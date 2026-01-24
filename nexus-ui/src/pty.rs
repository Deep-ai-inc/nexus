//! PTY handling for spawning and communicating with shell processes.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::thread;

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc;

use nexus_api::BlockId;
use crate::app::PtyEvent;

/// Handle to a running PTY process.
pub struct PtyHandle {
    pub block_id: BlockId,
    /// Writer to send input to the PTY.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Master PTY for resize operations.
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Child process for signal handling.
    #[allow(dead_code)]
    child: Arc<Mutex<Option<Box<dyn Child + Send + Sync>>>>,
}

impl PtyHandle {
    /// Spawn a new PTY running the given command with default size.
    #[allow(dead_code)]
    pub fn spawn(
        command: &str,
        cwd: &str,
        block_id: BlockId,
        tx: mpsc::UnboundedSender<(BlockId, PtyEvent)>,
    ) -> anyhow::Result<Self> {
        Self::spawn_with_size(command, cwd, block_id, tx, 120, 24)
    }

    /// Spawn a new PTY running the given command with specified size.
    pub fn spawn_with_size(
        command: &str,
        cwd: &str,
        block_id: BlockId,
        tx: mpsc::UnboundedSender<(BlockId, PtyEvent)>,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();

        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Build the command
        let mut cmd = CommandBuilder::new("sh");
        cmd.arg("-c");
        cmd.arg(command);
        cmd.cwd(cwd);

        // Spawn the child process
        let child = pair.slave.spawn_command(cmd)?;
        let child: Arc<Mutex<Option<Box<dyn Child + Send + Sync>>>> =
            Arc::new(Mutex::new(Some(child)));

        // Get reader and writer for the master side
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        let master: Arc<Mutex<Box<dyn MasterPty + Send>>> = Arc::new(Mutex::new(pair.master));

        // Spawn reader thread
        let tx_clone = tx.clone();
        let child_clone = child.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        if tx_clone.send((block_id, PtyEvent::Output(data))).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::debug!("PTY read error: {}", e);
                        break;
                    }
                }
            }

            // Wait for child and send exit status
            if let Some(mut child) = child_clone.lock().unwrap().take() {
                match child.wait() {
                    Ok(status) => {
                        let code = status.exit_code() as i32;
                        let _ = tx_clone.send((block_id, PtyEvent::Exited(code)));
                    }
                    Err(e) => {
                        tracing::error!("Failed to wait for child: {}", e);
                        let _ = tx_clone.send((block_id, PtyEvent::Exited(1)));
                    }
                }
            }
        });

        Ok(Self {
            block_id,
            writer,
            master,
            child,
        })
    }

    /// Write input to the PTY.
    pub fn write(&self, data: &[u8]) -> std::io::Result<()> {
        let mut writer = self.writer.lock().unwrap();
        writer.write_all(data)?;
        writer.flush()
    }

    /// Write a string to the PTY.
    #[allow(dead_code)]
    pub fn write_str(&self, s: &str) -> std::io::Result<()> {
        self.write(s.as_bytes())
    }

    /// Resize the PTY.
    pub fn resize(&self, cols: u16, rows: u16) -> anyhow::Result<()> {
        let master = self.master.lock().unwrap();
        master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        Ok(())
    }

    /// Send Ctrl+C (SIGINT) to the process.
    pub fn send_interrupt(&self) -> std::io::Result<()> {
        // Send ETX (Ctrl+C) character
        self.write(&[0x03])
    }

    /// Send Ctrl+D (EOF) to the process.
    pub fn send_eof(&self) -> std::io::Result<()> {
        // Send EOT (Ctrl+D) character
        self.write(&[0x04])
    }

    /// Send Ctrl+Z (SIGTSTP) to the process.
    pub fn send_suspend(&self) -> std::io::Result<()> {
        // Send SUB (Ctrl+Z) character
        self.write(&[0x1a])
    }

    /// Kill the process.
    #[allow(dead_code)]
    pub fn kill(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}
