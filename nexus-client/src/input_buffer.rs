//! Request sender with PTY input buffering for reconnection resilience.
//!
//! When the transport dies (WiFi drop, SSH kill), inputs that were sent to
//! the old request channel but never written to the wire are lost. The
//! `RequestSender` keeps a copy of every PtyInput until the agent confirms
//! receipt via the echo epoch on StdoutChunk. On reconnect, unconfirmed
//! inputs are replayed automatically.

use nexus_protocol::messages::Request;
use tokio::sync::mpsc;

/// A buffered PTY input awaiting echo epoch confirmation.
#[derive(Debug, Clone)]
struct BufferedInput {
    block_id: nexus_api::BlockId,
    data: Vec<u8>,
    echo_epoch: u64,
}

/// Wraps `mpsc::UnboundedSender<Request>` with PTY input buffering.
///
/// Use this instead of a raw sender. PTY inputs are buffered until confirmed
/// by the agent's echo epoch. On transport swap (reconnection), unconfirmed
/// inputs are replayed on the new channel.
#[derive(Debug)]
pub struct RequestSender {
    tx: mpsc::UnboundedSender<Request>,
    buffer: Vec<BufferedInput>,
}

impl RequestSender {
    /// Create a new sender wrapping the given channel.
    pub fn new(tx: mpsc::UnboundedSender<Request>) -> Self {
        Self {
            tx,
            buffer: Vec::new(),
        }
    }

    /// Send a request to the agent. Non-PtyInput requests are sent directly.
    /// PtyInput requests are also buffered for reconnection replay.
    ///
    /// Returns `true` if the send succeeded, `false` if the channel is closed.
    pub fn send(&mut self, request: Request) -> bool {
        if let Request::PtyInput {
            block_id,
            ref data,
            echo_epoch,
        } = request
        {
            self.buffer.push(BufferedInput {
                block_id,
                data: data.clone(),
                echo_epoch,
            });
        }
        self.tx.send(request).is_ok()
    }

    /// Confirm that the agent has processed inputs up to the given echo epoch.
    /// Removes all buffered inputs with epoch <= the confirmed epoch.
    pub fn confirm_echo_epoch(&mut self, epoch: u64) {
        if epoch > 0 {
            self.buffer.retain(|i| i.echo_epoch > epoch);
        }
    }

    /// Swap the underlying transport channel (after reconnection).
    /// Prunes confirmed inputs, then replays unconfirmed ones on the new channel.
    pub fn swap_transport(&mut self, new_tx: mpsc::UnboundedSender<Request>, confirmed_epoch: u64) {
        self.tx = new_tx;
        // Prune inputs the agent already processed
        self.confirm_echo_epoch(confirmed_epoch);
        // Replay unconfirmed inputs on the new channel
        for input in &self.buffer {
            let _ = self.tx.send(Request::PtyInput {
                block_id: input.block_id,
                data: input.data.clone(),
                echo_epoch: input.echo_epoch,
            });
        }
    }

    /// Get a reference to the underlying sender (for cloning, etc).
    pub fn inner(&self) -> &mpsc::UnboundedSender<Request> {
        &self.tx
    }

    /// Consume the sender, returning the underlying raw channel.
    pub fn into_inner(self) -> mpsc::UnboundedSender<Request> {
        self.tx
    }

    /// Number of unconfirmed PTY inputs in the buffer.
    pub fn pending_input_count(&self) -> usize {
        self.buffer.len()
    }
}
