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
use nix::unistd::{execvp, fork, ForkResult, Pid};
use nexus_api::{BlockId, ShellEvent};

use crate::parser::{Command, Redirect};
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
    _redirects: &[Redirect],
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
            // TODO: Implement redirect handling

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

/// Spawn a pipeline of commands.
pub fn spawn_pipeline(
    state: &ShellState,
    commands: &[Command],
) -> anyhow::Result<Vec<ProcessHandle>> {
    // TODO: Implement proper pipeline with pipes between processes
    // For now, this is a placeholder

    let mut handles = Vec::new();

    for cmd in commands {
        if let Command::Simple(simple) = cmd {
            let argv: Vec<String> = std::iter::once(simple.name.clone())
                .chain(simple.args.iter().filter_map(|w| w.as_literal().map(String::from)))
                .collect();

            let handle = spawn(&argv, &state.cwd, &state.env, &[], &simple.redirects)?;
            handles.push(handle);
        }
    }

    Ok(handles)
}

/// Wait for all processes in a pipeline.
pub fn wait_pipeline(
    handles: Vec<ProcessHandle>,
    block_id: BlockId,
    events: &Sender<ShellEvent>,
) -> anyhow::Result<i32> {
    let mut last_exit = 0;

    for handle in handles {
        last_exit = wait_with_events(handle, block_id, events)?;
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
