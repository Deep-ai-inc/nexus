//! Remote PTY allocation and I/O streaming.
//!
//! Maps `PtySpawn`, `PtyInput`, `PtyResize`, `PtyKill`, `PtyClose` requests
//! to actual PTY operations on the remote machine.
//!
//! Each PTY session runs a shadow terminal emulator (`ShadowParser`) inside
//! its reader task. The shadow parser is owned exclusively by the reader —
//! no shared mutex on the hot path. Snapshot requests are served via a
//! oneshot channel: the caller sends a oneshot::Sender, the reader task
//! extracts the grid and responds.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use anyhow::Result;
use nexus_api::{BlockId, ShellEvent, TerminalModes};
use nexus_term::ShadowParser;
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, WriteHalf};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

/// Manages active PTY sessions on the agent.
pub(crate) struct PtyManager {
    /// Active PTY sessions indexed by block ID.
    sessions: HashMap<BlockId, PtySession>,
    /// PIDs being waited on by Tokio. Shared with the zombie reaper so it
    /// knows which PIDs to skip (reaping them would steal the exit status).
    tokio_pids: Arc<std::sync::Mutex<std::collections::HashSet<u32>>>,
    /// Exit statuses reaped by the zombie reaper before Tokio's waiter could
    /// collect them. When Tokio gets ECHILD, it looks here for the real code.
    reaped_statuses: Arc<std::sync::Mutex<HashMap<u32, i32>>>,
}

/// Request types the reader task services between reads.
enum ReaderRequest {
    /// Extract a full snapshot (viewport + scrollback + modes).
    Snapshot(oneshot::Sender<SnapshotResponse>),
    /// Resize the shadow parser (sent alongside the ioctl resize).
    Resize { cols: u16, rows: u16 },
}

/// Response from a snapshot request.
pub(crate) struct SnapshotResponse {
    pub grid: nexus_term::TerminalGrid,
    pub scrollback: Vec<nexus_term::Cell>,
    pub scrollback_cols: u16,
    pub alt_screen: bool,
    pub app_cursor: bool,
    pub bracketed_paste: bool,
}

/// A single PTY session.
struct PtySession {
    /// Write half of the PTY master (for input).
    write_half: WriteHalf<PtyMaster>,
    /// Child process PID (and process group ID after setpgid).
    child_pid: u32,
    /// Process group ID (== child_pid after setpgid(0,0) in pre_exec).
    pgid: i32,
    /// Start time for duration tracking.
    start_time: Instant,
    /// Background output reader task.
    reader_handle: JoinHandle<()>,
    /// Background child waiter task.
    waiter_handle: JoinHandle<()>,
    /// Raw fd for ioctl operations (resize). Kept alive via the AsyncFd
    /// inside the split halves, but we need the numeric fd for ioctl.
    master_raw_fd: i32,
    /// Channel to send requests to the reader task (snapshot, resize).
    /// Lock-free: the reader task owns the ShadowParser exclusively.
    reader_tx: mpsc::UnboundedSender<ReaderRequest>,
    /// Highest echo epoch written to the PTY master.
    /// Written by the input path (after write_all), read by the reader task
    /// to stamp StdoutChunk events. Lock-free coordination.
    last_written_epoch: Arc<AtomicU64>,
}

/// Newtype for AsyncRead + AsyncWrite over a PTY master fd.
///
/// Wraps an `AsyncFd<OwnedFd>` and implements tokio's async I/O traits
/// so we can `tokio::io::split()` it into read/write halves backed by
/// a single fd and single reactor registration.
struct PtyMaster(AsyncFd<OwnedFd>);

impl PtyMaster {
    fn new(fd: OwnedFd) -> io::Result<Self> {
        // Set non-blocking before wrapping in AsyncFd
        let raw = fd.as_raw_fd();
        let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
        if flags == -1 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(AsyncFd::new(fd)?))
    }

    fn as_raw_fd(&self) -> i32 {
        self.0.as_raw_fd()
    }
}

impl AsyncRead for PtyMaster {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            let mut guard = match self.0.poll_read_ready(cx) {
                Poll::Ready(Ok(g)) => g,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };

            let fd = self.0.as_raw_fd();
            let unfilled = buf.initialize_unfilled();
            let ret =
                unsafe { libc::read(fd, unfilled.as_mut_ptr() as *mut libc::c_void, unfilled.len()) };

            if ret >= 0 {
                buf.advance(ret as usize);
                return Poll::Ready(Ok(()));
            }

            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                guard.clear_ready();
                continue;
            }
            return Poll::Ready(Err(err));
        }
    }
}

impl AsyncWrite for PtyMaster {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            let mut guard = match self.0.poll_write_ready(cx) {
                Poll::Ready(Ok(g)) => g,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Pending => return Poll::Pending,
            };

            let fd = self.0.as_raw_fd();
            let ret =
                unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };

            if ret >= 0 {
                return Poll::Ready(Ok(ret as usize));
            }

            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                guard.clear_ready();
                continue;
            }
            return Poll::Ready(Err(err));
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            tokio_pids: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            reaped_statuses: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    /// Returns a handle to the set of PIDs managed by Tokio's child waiter.
    /// Shared with the zombie reaper to avoid stealing exit statuses.
    pub fn tokio_pids(&self) -> Arc<std::sync::Mutex<std::collections::HashSet<u32>>> {
        self.tokio_pids.clone()
    }

    /// Exit statuses captured by the zombie reaper. When Tokio's waiter gets
    /// ECHILD (because the reaper already reaped the PID), it checks here.
    pub fn reaped_statuses(&self) -> Arc<std::sync::Mutex<HashMap<u32, i32>>> {
        self.reaped_statuses.clone()
    }

    /// Spawn a new PTY session.
    ///
    /// The PTY output is forwarded as `ShellEvent::StdoutChunk` via the event channel.
    /// Child exit is reported as `ShellEvent::CommandFinished`.
    pub async fn spawn(
        &mut self,
        command: &str,
        block_id: BlockId,
        cols: u16,
        rows: u16,
        term: &str,
        cwd: &str,
        kernel_tx: &broadcast::Sender<ShellEvent>,
    ) -> Result<()> {
        // 1. Allocate PTY pair
        let pty = nix::pty::openpty(None, None)?;
        let master_fd: OwnedFd = pty.master;
        let slave_fd: OwnedFd = pty.slave;

        // 2. Set initial window size on master
        let master_raw = master_fd.as_raw_fd();
        let ws = libc::winsize {
            ws_row: rows,
            ws_col: cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        unsafe {
            libc::ioctl(master_raw, libc::TIOCSWINSZ, &ws);
        }

        // 3. Spawn child process with slave as stdin/stdout/stderr
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let term = term.to_string();
        let slave_raw = slave_fd.as_raw_fd();

        let mut cmd = tokio::process::Command::new(&shell);
        cmd.arg("-c").arg(command);
        cmd.env("TERM", &term);
        cmd.env("COLUMNS", cols.to_string());
        cmd.env("LINES", rows.to_string());
        if !cwd.is_empty() {
            cmd.current_dir(cwd);
        }

        // Use the slave fd for child stdio
        unsafe {
            use std::os::fd::BorrowedFd;
            let stdin_fd = BorrowedFd::borrow_raw(slave_raw);
            let stdout_fd = BorrowedFd::borrow_raw(slave_raw);
            let stderr_fd = BorrowedFd::borrow_raw(slave_raw);
            cmd.stdin(std::process::Stdio::from(OwnedFd::from(stdin_fd.try_clone_to_owned()?)));
            cmd.stdout(std::process::Stdio::from(OwnedFd::from(stdout_fd.try_clone_to_owned()?)));
            cmd.stderr(std::process::Stdio::from(OwnedFd::from(stderr_fd.try_clone_to_owned()?)));
        }

        // 4. pre_exec: setsid, TIOCSCTTY, setpgid (async-signal-safe only)
        unsafe {
            cmd.pre_exec(move || {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(slave_raw, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                // Own process group (ok if fails)
                libc::setpgid(0, 0);
                Ok(())
            });
        }

        let mut child = cmd.spawn()?;
        let child_pid = child.id().ok_or_else(|| anyhow::anyhow!("child has no pid"))?;
        let pgid = child_pid as i32;

        // Drop the slave fd in the parent (child has its own copy)
        drop(slave_fd);

        let start_time = Instant::now();

        // 5. Wrap master fd in async I/O and split
        let pty_master = PtyMaster::new(master_fd)?;
        let raw_fd = pty_master.as_raw_fd();
        let (read_half, write_half) = tokio::io::split(pty_master);

        // 6. Create channel for reader requests (snapshot, resize)
        let (reader_tx, reader_rx) = mpsc::unbounded_channel::<ReaderRequest>();

        // 7. Shared epoch counter: input path writes, reader task reads
        let last_written_epoch = Arc::new(AtomicU64::new(0));

        // 8. Spawn reader task: owns ShadowParser exclusively (no shared mutex)
        let tx = kernel_tx.clone();
        let reader_block_id = block_id;
        let reader_raw_fd = raw_fd;
        let reader_epoch = last_written_epoch.clone();
        let reader_handle = tokio::spawn(reader_task(
            read_half,
            reader_rx,
            tx,
            reader_block_id,
            reader_raw_fd,
            cols,
            rows,
            reader_epoch,
        ));

        // 9. Register PID as Tokio-managed (zombie reaper must not touch it)
        self.tokio_pids.lock().unwrap().insert(child_pid);

        // 10. Spawn waiter task: wait for child exit and send CommandFinished
        let tx = kernel_tx.clone();
        let waiter_block_id = block_id;
        let waiter_tokio_pids = self.tokio_pids.clone();
        let waiter_reaped = self.reaped_statuses.clone();
        let waiter_child_pid = child_pid;
        let waiter_handle = tokio::spawn(async move {
            let status = child.wait().await;
            // Unregister — zombie reaper can now safely reap this PID if needed
            waiter_tokio_pids.lock().unwrap().remove(&waiter_child_pid);
            let (exit_code, duration_ms) = match status {
                Ok(s) => {
                    let code = s.code().unwrap_or(-1);
                    let dur = start_time.elapsed().as_millis() as u64;
                    (code, dur)
                }
                Err(e) => {
                    // ECHILD means the zombie reaper already reaped this PID.
                    // Check the saved exit status rather than defaulting to -1.
                    let code = waiter_reaped.lock().unwrap()
                        .remove(&waiter_child_pid)
                        .unwrap_or_else(|| {
                            tracing::warn!("wait error for pid {waiter_child_pid}: {e}");
                            -1
                        });
                    (code, start_time.elapsed().as_millis() as u64)
                }
            };
            let _ = tx.send(ShellEvent::CommandFinished {
                block_id: waiter_block_id,
                exit_code,
                duration_ms,
            });
        });

        // 10. Store session
        self.sessions.insert(
            block_id,
            PtySession {
                write_half,
                child_pid,
                pgid,
                start_time,
                reader_handle,
                waiter_handle,
                master_raw_fd: raw_fd,
                reader_tx,
                last_written_epoch,
            },
        );

        Ok(())
    }

    /// Write input to a PTY session and update the echo epoch.
    ///
    /// The epoch is stored AFTER the write completes, ensuring the reader
    /// task only sees the epoch once the bytes are in the PTY master's
    /// kernel buffer. This guarantees: any StdoutChunk stamped with
    /// epoch N reflects output produced after the input for epoch N
    /// was written.
    pub async fn input(&mut self, block_id: BlockId, data: &[u8], echo_epoch: u64) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        if let Some(session) = self.sessions.get_mut(&block_id) {
            session.write_half.write_all(data).await?;
            // Store epoch AFTER write — reader task will see it on next read
            session.last_written_epoch.store(echo_epoch, Ordering::Release);
        }
        Ok(())
    }

    /// Resize a PTY session (TIOCSWINSZ ioctl).
    pub fn resize(&mut self, block_id: BlockId, cols: u16, rows: u16) -> Result<()> {
        if let Some(session) = self.sessions.get(&block_id) {
            let ws = libc::winsize {
                ws_row: rows,
                ws_col: cols,
                ws_xpixel: 0,
                ws_ypixel: 0,
            };
            let ret = unsafe { libc::ioctl(session.master_raw_fd, libc::TIOCSWINSZ, &ws) };
            if ret == -1 {
                return Err(io::Error::last_os_error().into());
            }
            // Tell reader task to resize its shadow parser (lock-free)
            let _ = session.reader_tx.send(ReaderRequest::Resize { cols, rows });
        }
        Ok(())
    }

    /// Kill a PTY session's process group.
    pub fn kill(&mut self, block_id: BlockId, signal: i32) -> Result<()> {
        if let Some(session) = self.sessions.get(&block_id) {
            let ret = unsafe { libc::killpg(session.pgid, signal) };
            if ret == -1 {
                let err = io::Error::last_os_error();
                // ESRCH = process group already exited — not an error
                if err.raw_os_error() != Some(libc::ESRCH) {
                    return Err(err.into());
                }
            }
        }
        Ok(())
    }

    /// Close a PTY session's master fd (sends EOF to child).
    ///
    /// SAFETY: `sessions.remove()` MUST happen before dropping I/O halves.
    /// This ensures that a concurrent `resize()` call sees `None` in the
    /// HashMap lookup (no-op) rather than using a dangling `master_raw_fd`
    /// that the OS may have reassigned to another file.
    pub fn close(&mut self, block_id: BlockId) -> Result<()> {
        if let Some(session) = self.sessions.remove(&block_id) {
            // Drop write_half (releases one Arc ref from split)
            drop(session.write_half);
            // Abort reader (releases the other Arc ref → fd closes → EOF to slave)
            session.reader_handle.abort();
            // Abort waiter (or it completes naturally)
            session.waiter_handle.abort();
            // Safety: SIGHUP the process group in case background jobs hold the terminal
            unsafe {
                libc::killpg(session.pgid, libc::SIGHUP);
            }
        }
        Ok(())
    }

    /// Shut down all active PTY sessions. Called on agent exit.
    pub fn shutdown_all(&mut self) {
        let block_ids: Vec<BlockId> = self.sessions.keys().copied().collect();
        for block_id in block_ids {
            let _ = self.close(block_id);
        }
    }

    /// Return the block IDs of all active PTY sessions.
    pub fn active_block_ids(&self) -> Vec<BlockId> {
        self.sessions.keys().copied().collect()
    }

    /// Returns true if there are any active PTY sessions.
    pub fn has_active_sessions(&self) -> bool {
        !self.sessions.is_empty()
    }


    /// Request a terminal snapshot from the reader task (lock-free).
    /// Returns None if the session doesn't exist or the reader task has exited.
    pub async fn snapshot(&self, block_id: BlockId) -> Option<SnapshotResponse> {
        let session = self.sessions.get(&block_id)?;
        let (tx, rx) = oneshot::channel();
        session.reader_tx.send(ReaderRequest::Snapshot(tx)).ok()?;
        rx.await.ok()
    }
}

/// The reader task: owns a ShadowParser exclusively. Reads PTY output,
/// feeds the shadow parser, emits events, and services snapshot requests.
/// No shared mutexes — all communication is via channels.
async fn reader_task(
    read_half: tokio::io::ReadHalf<PtyMaster>,
    mut request_rx: mpsc::UnboundedReceiver<ReaderRequest>,
    tx: broadcast::Sender<ShellEvent>,
    block_id: BlockId,
    master_raw_fd: i32,
    cols: u16,
    rows: u16,
    last_written_epoch: Arc<AtomicU64>,
) {
    use tokio::io::AsyncReadExt;
    let mut reader = read_half;
    let mut shadow = ShadowParser::new(cols, rows);
    let mut last_modes: Option<TerminalModes> = None;
    let mut buf = [0u8; 4096];

    loop {
        tokio::select! {
            // Hot path: read PTY output
            result = reader.read(&mut buf) => {
                match result {
                    Ok(0) => break, // EOF — slave closed
                    Ok(n) => {
                        let data = &buf[..n];

                        // Feed shadow parser (owned, no lock)
                        shadow.feed(data);

                        // Check terminal mode changes
                        let (echo, icanon) = read_termios_flags(master_raw_fd);
                        let current_modes = TerminalModes {
                            echo,
                            icanon,
                            alt_screen: shadow.is_alternate_screen(),
                            app_cursor: shadow.app_cursor(),
                            bracketed_paste: shadow.bracketed_paste(),
                        };

                        if last_modes.as_ref() != Some(&current_modes) {
                            let _ = tx.send(ShellEvent::TerminalModeChanged {
                                block_id,
                                modes: current_modes.clone(),
                            });
                            last_modes = Some(current_modes);
                        }

                        let epoch = last_written_epoch.load(Ordering::Acquire);
                        let _ = tx.send(ShellEvent::StdoutChunk {
                            block_id,
                            data: data.to_vec(),
                            last_echo_epoch: epoch,
                        });
                    }
                    Err(e) => {
                        if e.raw_os_error() != Some(libc::EIO) {
                            tracing::debug!("pty read error: {e}");
                        }
                        break;
                    }
                }
            }

            // Service requests between reads (snapshot, resize)
            Some(req) = request_rx.recv() => {
                match req {
                    ReaderRequest::Snapshot(reply_tx) => {
                        let grid = shadow.extract_grid();
                        let (scrollback, scrollback_cols) = shadow.extract_scrollback();
                        let resp = SnapshotResponse {
                            grid,
                            scrollback,
                            scrollback_cols,
                            alt_screen: shadow.is_alternate_screen(),
                            app_cursor: shadow.app_cursor(),
                            bracketed_paste: shadow.bracketed_paste(),
                        };
                        let _ = reply_tx.send(resp);
                    }
                    ReaderRequest::Resize { cols, rows } => {
                        shadow.resize(cols, rows);
                    }
                }
            }
        }
    }
}

/// Read ECHO and ICANON flags from the PTY slave's termios (via the master fd).
fn read_termios_flags(master_fd: i32) -> (bool, bool) {
    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    let ret = unsafe { libc::tcgetattr(master_fd, termios.as_mut_ptr()) };
    if ret == -1 {
        return (true, true); // Default: assume echo + canonical
    }
    let termios = unsafe { termios.assume_init() };
    let echo = (termios.c_lflag & libc::ECHO) != 0;
    let icanon = (termios.c_lflag & libc::ICANON) != 0;
    (echo, icanon)
}
