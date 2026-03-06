//! Remote PTY allocation and I/O streaming.
//!
//! Maps `PtySpawn`, `PtyInput`, `PtyResize`, `PtyKill`, `PtyClose` requests
//! to actual PTY operations on the remote machine.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Instant;

use anyhow::Result;
use nexus_api::{BlockId, ShellEvent};
use tokio::io::unix::AsyncFd;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf, WriteHalf};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

/// Manages active PTY sessions on the agent.
pub(crate) struct PtyManager {
    /// Active PTY sessions indexed by block ID.
    sessions: HashMap<BlockId, PtySession>,
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
        }
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

        // 6. Spawn reader task: read output and send StdoutChunk events
        let tx = kernel_tx.clone();
        let reader_block_id = block_id;
        let reader_handle = tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut reader = read_half;
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break, // EOF — slave closed
                    Ok(n) => {
                        let _ = tx.send(ShellEvent::StdoutChunk {
                            block_id: reader_block_id,
                            data: buf[..n].to_vec(),
                        });
                    }
                    Err(e) => {
                        // EIO is normal when the slave side closes
                        if e.raw_os_error() != Some(libc::EIO) {
                            tracing::debug!("pty read error: {e}");
                        }
                        break;
                    }
                }
            }
        });

        // 7. Spawn waiter task: wait for child exit and send CommandFinished
        let tx = kernel_tx.clone();
        let waiter_block_id = block_id;
        let waiter_handle = tokio::spawn(async move {
            let status = child.wait().await;
            let (exit_code, duration_ms) = match status {
                Ok(s) => {
                    let code = s.code().unwrap_or(-1);
                    let dur = start_time.elapsed().as_millis() as u64;
                    (code, dur)
                }
                Err(e) => {
                    tracing::warn!("wait error: {e}");
                    (-1, start_time.elapsed().as_millis() as u64)
                }
            };
            let _ = tx.send(ShellEvent::CommandFinished {
                block_id: waiter_block_id,
                exit_code,
                duration_ms,
            });
        });

        // 8. Store session
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
            },
        );

        Ok(())
    }

    /// Write input to a PTY session.
    pub async fn input(&mut self, block_id: BlockId, data: &[u8]) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        if let Some(session) = self.sessions.get_mut(&block_id) {
            session.write_half.write_all(data).await?;
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
}
