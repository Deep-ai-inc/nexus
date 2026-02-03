//! Process management - PTY allocation, job control, signals.

mod pty;
pub mod job;

pub use job::{Job, JobState};
pub use pty::PtyHandle;

use std::collections::HashMap;
use std::ffi::CString;
use std::path::Path;
use std::io::Read;
use std::time::Instant;

use tokio::sync::broadcast::Sender;
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::{close, dup2, execvp, fork, ForkResult, Pid};
use nix::fcntl::{open, OFlag};
use nix::sys::stat::Mode;
use nexus_api::{BlockId, ShellEvent};

use crate::parser::{Command, Redirect, RedirectOp};
use crate::ShellState;

/// Handle for a spawned process.
pub struct ProcessHandle {
    pub pid: Pid,
    pub pty: Option<PtyHandle>,
}

/// Spawn a process with the given arguments.
pub fn spawn(
    argv: &[String],
    cwd: &Path,
    env: &HashMap<String, String>,
    env_overrides: &[(String, String)],
    redirects: &[Redirect],
) -> anyhow::Result<ProcessHandle> {
    // Create a PTY for interactive processes
    let pty = pty::open_pty()?;

    match unsafe { fork() }? {
        ForkResult::Child => {
            // Child process
            drop(pty.master);

            // Set up the slave as stdin/stdout/stderr
            pty::setup_slave(&pty.slave)?;

            // Change to the working directory
            std::env::set_current_dir(cwd)?;

            // Set up environment
            // Safety: We're in a forked child process, before exec
            unsafe {
                for (key, value) in env {
                    std::env::set_var(key, value);
                }
                for (key, value) in env_overrides {
                    std::env::set_var(key, value);
                }
            }

            // Apply redirections
            if let Err(e) = apply_redirects(redirects) {
                eprintln!("redirect error: {}", e);
                std::process::exit(1);
            }

            // Convert argv to CStrings
            let argv_cstr: Vec<CString> = argv
                .iter()
                .map(|s| CString::new(s.as_str()).unwrap())
                .collect();

            // Execute
            execvp(&argv_cstr[0], &argv_cstr)?;
            unreachable!()
        }
        ForkResult::Parent { child } => {
            drop(pty.slave);

            Ok(ProcessHandle {
                pid: child,
                pty: Some(PtyHandle {
                    master: pty.master,
                    pid: child,
                }),
            })
        }
    }
}

/// Apply file redirections to the current process.
/// This should be called in the child process after fork, before exec.
fn apply_redirects(redirects: &[Redirect]) -> anyhow::Result<()> {
    for redirect in redirects {
        match redirect.op {
            RedirectOp::Write => {
                // fd > file - open file for writing (truncate)
                let flags = OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_TRUNC;
                let mode = Mode::from_bits_truncate(0o644);
                let fd = open(redirect.target.as_str(), flags, mode)?;
                dup2(fd, redirect.fd)?;
                close(fd)?;
            }
            RedirectOp::Append => {
                // fd >> file - open file for appending
                let flags = OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_APPEND;
                let mode = Mode::from_bits_truncate(0o644);
                let fd = open(redirect.target.as_str(), flags, mode)?;
                dup2(fd, redirect.fd)?;
                close(fd)?;
            }
            RedirectOp::Read => {
                // fd < file - open file for reading
                let flags = OFlag::O_RDONLY;
                let mode = Mode::empty();
                let fd = open(redirect.target.as_str(), flags, mode)?;
                dup2(fd, redirect.fd)?;
                close(fd)?;
            }
            RedirectOp::DupWrite => {
                // fd>&target - duplicate target fd to fd
                // e.g., 2>&1 means dup2(1, 2) - duplicate fd 1 to fd 2
                let target_fd: i32 = redirect.target.parse().unwrap_or(-1);
                if target_fd == -1 {
                    // Special case: >&- means close
                    if redirect.target == "-" {
                        close(redirect.fd)?;
                    } else {
                        anyhow::bail!("invalid fd for >&: {}", redirect.target);
                    }
                } else {
                    dup2(target_fd, redirect.fd)?;
                }
            }
            RedirectOp::DupRead => {
                // fd<&target - duplicate target fd to fd for reading
                let target_fd: i32 = redirect.target.parse().unwrap_or(-1);
                if target_fd == -1 {
                    if redirect.target == "-" {
                        close(redirect.fd)?;
                    } else {
                        anyhow::bail!("invalid fd for <&: {}", redirect.target);
                    }
                } else {
                    dup2(target_fd, redirect.fd)?;
                }
            }
        }
    }
    Ok(())
}

/// Wait for a process to complete, emitting events for output.
pub fn wait_with_events(
    handle: ProcessHandle,
    block_id: BlockId,
    events: &Sender<ShellEvent>,
) -> anyhow::Result<i32> {
    let start = Instant::now();

    if let Some(mut pty) = handle.pty {
        // Read from PTY and emit events
        let mut buffer = [0u8; 4096];

        // Set non-blocking mode
        pty::set_nonblocking(&pty.master, true)?;

        loop {
            // Check if process has exited
            match waitpid(handle.pid, Some(WaitPidFlag::WNOHANG))? {
                WaitStatus::Exited(_, code) => {
                    // Read any remaining output
                    while let Ok(n) = pty.master.read(&mut buffer) {
                        if n == 0 {
                            break;
                        }
                        let _ = events.send(ShellEvent::StdoutChunk {
                            block_id,
                            data: buffer[..n].to_vec(),
                        });
                    }

                    let _ = events.send(ShellEvent::CommandFinished {
                        block_id,
                        exit_code: code,
                        duration_ms: start.elapsed().as_millis() as u64,
                    });

                    return Ok(code);
                }
                WaitStatus::Signaled(_, signal, _) => {
                    let _ = events.send(ShellEvent::CommandFinished {
                        block_id,
                        exit_code: 128 + signal as i32,
                        duration_ms: start.elapsed().as_millis() as u64,
                    });

                    return Ok(128 + signal as i32);
                }
                WaitStatus::StillAlive => {
                    // Process still running, read available output
                    match pty.master.read(&mut buffer) {
                        Ok(0) => {
                            // EOF - process closed its end
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Ok(n) => {
                            let _ = events.send(ShellEvent::StdoutChunk {
                                block_id,
                                data: buffer[..n].to_vec(),
                            });
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                        Err(e) => {
                            tracing::debug!("PTY read error: {}", e);
                            std::thread::sleep(std::time::Duration::from_millis(10));
                        }
                    }
                }
                _ => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    } else {
        // No PTY - just wait for the process
        let status = waitpid(handle.pid, None)?;
        let code = match status {
            WaitStatus::Exited(_, code) => code,
            WaitStatus::Signaled(_, signal, _) => 128 + signal as i32,
            _ => 1,
        };

        let _ = events.send(ShellEvent::CommandFinished {
            block_id,
            exit_code: code,
            duration_ms: start.elapsed().as_millis() as u64,
        });

        Ok(code)
    }
}

/// Spawn a pipeline of external commands connected by real pipes.
///
/// Creates pipe pairs between adjacent processes:
///   process[0].stdout → pipe → process[1].stdin → pipe → process[2].stdin ...
///
/// The last process gets a PTY for terminal output. All processes share a
/// process group (pgid = first child's PID) so Ctrl+C kills them all.
pub fn spawn_pipeline(
    state: &ShellState,
    commands: &[Command],
) -> anyhow::Result<Vec<ProcessHandle>> {
    use nix::unistd::{pipe, setpgid, Pid};
    use std::os::fd::AsRawFd;

    let n = commands.len();
    if n == 0 {
        return Ok(vec![]);
    }
    if n == 1 {
        // Single command — use regular spawn with PTY
        if let Command::Simple(simple) = &commands[0] {
            let argv: Vec<String> = std::iter::once(simple.name.clone())
                .chain(simple.args.iter().filter_map(|w| w.as_literal().map(String::from)))
                .collect();
            let handle = spawn(&argv, &state.cwd, &state.env, &[], &simple.redirects)?;
            return Ok(vec![handle]);
        }
        return Ok(vec![]);
    }

    // Create pipes between adjacent processes, set CLOEXEC to prevent FD leaks
    let mut pipes = Vec::with_capacity(n - 1);
    for _ in 0..n - 1 {
        let (read_fd, write_fd) = pipe()?;
        set_cloexec(read_fd.as_raw_fd())?;
        set_cloexec(write_fd.as_raw_fd())?;
        pipes.push((read_fd, write_fd));
    }

    // PTY for the last process's output (so terminal escapes work and parent can read it)
    let pty = pty::open_pty()?;
    let pty_master_fd = pty.master.as_raw_fd();
    let pty_slave_fd = pty.slave.as_raw_fd();

    // Set CLOEXEC on PTY FDs so children that don't dup2 them won't leak them
    set_cloexec(pty_master_fd)?;
    set_cloexec(pty_slave_fd)?;

    let mut handles = Vec::with_capacity(n);
    let mut pgid = Pid::from_raw(0);

    for (i, cmd) in commands.iter().enumerate() {
        let Command::Simple(simple) = cmd else { continue };

        let argv: Vec<String> = std::iter::once(simple.name.clone())
            .chain(simple.args.iter().filter_map(|w| w.as_literal().map(String::from)))
            .collect();

        match unsafe { fork() }? {
            ForkResult::Child => {
                // Join the pipeline's process group (pgid=0 means "use my own PID" for the first child)
                let _ = setpgid(Pid::from_raw(0), pgid);

                // stdin: first process inherits parent's stdin; others read from previous pipe
                if i > 0 {
                    dup2(pipes[i - 1].0.as_raw_fd(), 0)?;
                }

                // stdout: last process writes to PTY; others write to next pipe
                if i < n - 1 {
                    dup2(pipes[i].1.as_raw_fd(), 1)?;
                } else {
                    dup2(pty_slave_fd, 1)?;
                }

                // stderr: always to PTY (visible in terminal)
                dup2(pty_slave_fd, 2)?;

                // All other FDs (pipes, PTY master/slave originals) have CLOEXEC
                // and will be closed on exec. Set cwd and env.
                let _ = std::env::set_current_dir(&state.cwd);
                unsafe {
                    for (key, value) in &state.env {
                        std::env::set_var(key, value);
                    }
                }

                if let Err(e) = apply_redirects(&simple.redirects) {
                    eprintln!("redirect error: {}", e);
                    std::process::exit(1);
                }

                let argv_cstr: Vec<CString> = argv
                    .iter()
                    .map(|s| CString::new(s.as_str()).unwrap())
                    .collect();
                let _ = execvp(&argv_cstr[0], &argv_cstr);
                // exec failed
                eprintln!("{}: command not found", argv[0]);
                std::process::exit(127);
            }
            ForkResult::Parent { child } => {
                if i == 0 {
                    pgid = child;
                }
                // Also set pgid in parent (prevents race where child execs before setpgid)
                let _ = setpgid(child, pgid);

                handles.push(ProcessHandle { pid: child, pty: None });
            }
        }
    }

    // Parent: close all pipe FDs (children own them via dup2; CLOEXEC closed the originals on exec)
    drop(pipes);
    // Close PTY slave — children have it via dup2
    drop(pty.slave);

    // Give the last handle the PTY master so wait_pipeline can read output
    if let Some(last) = handles.last_mut() {
        last.pty = Some(PtyHandle {
            master: pty.master,
            pid: last.pid,
        });
    } else {
        drop(pty.master);
    }

    Ok(handles)
}

/// Set FD_CLOEXEC on a file descriptor.
fn set_cloexec(fd: i32) -> anyhow::Result<()> {
    use nix::fcntl::{fcntl, FcntlArg, FdFlag};
    let flags = fcntl(fd, FcntlArg::F_GETFD)?;
    let mut flags = FdFlag::from_bits_truncate(flags);
    flags.insert(FdFlag::FD_CLOEXEC);
    fcntl(fd, FcntlArg::F_SETFD(flags))?;
    Ok(())
}

/// Wait for all processes in a pipeline.
///
/// Waits for the LAST process first (the one with the PTY, producing output).
/// This prevents deadlocks: if we waited for process[0] first, it might block
/// writing to a full pipe because process[1] hasn't started reading yet.
/// By waiting for the last process, data flows through the pipeline naturally.
/// After the last process exits, upstream processes get SIGPIPE and die.
pub fn wait_pipeline(
    mut handles: Vec<ProcessHandle>,
    block_id: BlockId,
    events: &Sender<ShellEvent>,
) -> anyhow::Result<i32> {
    if handles.is_empty() {
        return Ok(0);
    }

    // Pop the last handle (has the PTY) and wait for it while streaming output
    let last = handles.pop().unwrap();
    let last_exit = wait_with_events(last, block_id, events)?;

    // Reap remaining processes (they should be done or dying from SIGPIPE)
    for handle in handles {
        match waitpid(handle.pid, Some(WaitPidFlag::WNOHANG)) {
            Ok(WaitStatus::StillAlive) => {
                // Still running — give it a moment then block-wait
                let _ = waitpid(handle.pid, None);
            }
            Ok(_) => {} // Already exited
            Err(_) => {} // Already reaped
        }
    }

    Ok(last_exit)
}

/// Spawn an external command with optional stdin input (for native→external piping).
///
/// This function:
/// 1. Spawns the process using pipes (not PTY) for stdin/stdout
/// 2. Writes stdin_text to the process's stdin if provided
/// 3. Reads stdout and emits events
/// 4. Returns the exit code
pub fn spawn_with_stdin(
    name: &str,
    args: &[String],
    stdin_text: Option<String>,
    state: &ShellState,
    block_id: BlockId,
    events: &Sender<ShellEvent>,
) -> anyhow::Result<i32> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let start = Instant::now();

    // Build the command
    let mut cmd = Command::new(name);
    cmd.args(args)
        .current_dir(&state.cwd)
        .stdin(if stdin_text.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Set environment
    for (key, value) in &state.env {
        cmd.env(key, value);
    }

    // Spawn the process
    let mut child = cmd.spawn()?;

    // Write stdin if provided
    if let Some(text) = stdin_text {
        if let Some(mut stdin) = child.stdin.take() {
            // Write in a thread to avoid deadlock with stdout reading
            std::thread::spawn(move || {
                let _ = stdin.write_all(text.as_bytes());
                // stdin is dropped here, closing the pipe
            });
        }
    }

    // Read stdout in chunks and emit events
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Read stdout
    if let Some(mut stdout) = stdout {
        let events_clone = events.clone();
        let block_id_clone = block_id;
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut stdout, &mut buffer) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let _ = events_clone.send(ShellEvent::StdoutChunk {
                            block_id: block_id_clone,
                            data: buffer[..n].to_vec(),
                        });
                    }
                    Err(_) => break,
                }
            }
        });
    }

    // Read stderr
    if let Some(mut stderr) = stderr {
        let events_clone = events.clone();
        let block_id_clone = block_id;
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                match std::io::Read::read(&mut stderr, &mut buffer) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let _ = events_clone.send(ShellEvent::StderrChunk {
                            block_id: block_id_clone,
                            data: buffer[..n].to_vec(),
                        });
                    }
                    Err(_) => break,
                }
            }
        });
    }

    // Wait for process to complete
    let status = child.wait()?;
    let exit_code = status.code().unwrap_or(1);

    // Note: CommandFinished is emitted by the pipeline executor, not here
    let _ = start.elapsed(); // Silence unused warning

    Ok(exit_code)
}
