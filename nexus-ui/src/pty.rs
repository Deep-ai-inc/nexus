//! PTY handling for spawning and communicating with shell processes.

use std::io::Read;
use std::thread;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tokio::sync::mpsc;

use nexus_api::BlockId;
use crate::app::PtyEvent;

/// Handle to a running PTY process.
pub struct PtyHandle {
    pub block_id: BlockId,
    // The actual PTY is managed by the reader thread
}

impl PtyHandle {
    /// Spawn a new PTY running the given command.
    pub fn spawn(
        command: &str,
        cwd: &str,
        block_id: BlockId,
        tx: mpsc::UnboundedSender<(BlockId, PtyEvent)>,
    ) -> anyhow::Result<Self> {
        let pty_system = native_pty_system();

        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        // Build the command
        let mut cmd = CommandBuilder::new("sh");
        cmd.arg("-c");
        cmd.arg(command);
        cmd.cwd(cwd);

        // Spawn the child process
        let mut child = pair.slave.spawn_command(cmd)?;

        // Get reader for the master side
        let mut reader = pair.master.try_clone_reader()?;

        // Spawn reader thread
        let tx_clone = tx.clone();
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
        });

        Ok(Self { block_id })
    }
}
