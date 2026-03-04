//! Remote backend: manages SSH transport to a remote `nexus-agent`.
//!
//! The `RemoteBackend` sends `Request` messages over the wire and receives
//! `Response` messages. `Response::Event` variants are injected into the same
//! `broadcast::Sender<ShellEvent>` that the local kernel uses, so the rest
//! of the UI is completely unchanged.

pub(crate) mod deploy;
pub(crate) mod event_bridge;
pub(crate) mod queue;
pub(crate) mod transport;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use nexus_api::BlockId;
use nexus_protocol::messages::{
    CompletionItem, EnvInfo, HistoryEntry, Request, Response, Transport,
};
use tokio::sync::{mpsc, oneshot};

/// A pending request awaiting a response from the remote agent.
enum PendingRequest {
    Complete(oneshot::Sender<(Vec<CompletionItem>, usize)>),
    SearchHistory(oneshot::Sender<Vec<HistoryEntry>>),
    Nest,
    PtySpawn(BlockId),
}

/// An entry in the backend connection stack.
/// Each level represents one hop (local → remote1 → remote2 → ...).
#[derive(Debug, Clone)]
pub(crate) struct BackendEntry {
    pub env: EnvInfo,
}

/// Side effect from processing a response — the caller must act on these.
#[derive(Debug)]
pub(crate) enum ResponseEffect {
    /// No side effect.
    None,
    /// A remote PTY spawn failed — the caller should emit synthetic error + finish events.
    PtySpawnFailed { block_id: BlockId, message: String },
}

/// Connection state for the remote backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionState {
    Connected,
    Reconnecting,
    Disconnected,
}

/// The remote backend. Manages the SSH child process and wire protocol.
///
/// Not `Clone` because it contains oneshot channels for pending requests.
pub(crate) struct RemoteBackend {
    /// Channel to send requests to the transport task.
    request_tx: mpsc::UnboundedSender<RequestEnvelope>,
    /// Remote environment info from the last HelloOk.
    pub env: EnvInfo,
    /// Last seen sequence number (for resume), shared with event bridge.
    pub(crate) last_seen_seq: Arc<AtomicU64>,
    /// Next request ID.
    next_id: u32,
    /// Pending request/response pairs.
    pending: HashMap<u32, PendingRequest>,
    /// Current connection state.
    pub state: ConnectionState,
    /// Backend connection stack (for nesting).
    pub backend_stack: Vec<BackendEntry>,
    /// Queued commands while disconnected.
    pub pending_queue: Vec<QueuedCommand>,
    /// Current RTT in milliseconds (shared with event bridge).
    pub rtt_ms: Arc<AtomicU64>,
    /// Receiver for non-event responses from the event bridge.
    pub(crate) response_rx: mpsc::UnboundedReceiver<Response>,
    /// The SSH/docker/kubectl child process (owned for lifecycle management).
    child: Option<tokio::process::Child>,
}

impl std::fmt::Debug for RemoteBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteBackend")
            .field("env", &self.env)
            .field("state", &self.state)
            .field("pending_count", &self.pending.len())
            .field("queue_len", &self.pending_queue.len())
            .finish()
    }
}

/// A command queued while disconnected.
#[derive(Debug, Clone)]
pub(crate) struct QueuedCommand {
    pub command: String,
    pub block_id: BlockId,
}

/// Envelope wrapping a request for the transport task.
pub(crate) struct RequestEnvelope {
    pub request: Request,
}

impl RemoteBackend {
    /// Create a new remote backend from an established transport.
    pub fn new(
        env: EnvInfo,
        request_tx: mpsc::UnboundedSender<RequestEnvelope>,
        rtt_ms: Arc<AtomicU64>,
        last_seen_seq: Arc<AtomicU64>,
        response_rx: mpsc::UnboundedReceiver<Response>,
        child: Option<tokio::process::Child>,
    ) -> Self {
        Self {
            request_tx,
            env,
            last_seen_seq,
            next_id: 1,
            pending: HashMap::new(),
            state: ConnectionState::Connected,
            backend_stack: Vec::new(),
            pending_queue: Vec::new(),
            rtt_ms,
            response_rx,
            child,
        }
    }

    /// Read the current RTT in milliseconds (0 = not yet measured).
    pub fn current_rtt_ms(&self) -> u64 {
        self.rtt_ms.load(Ordering::Relaxed)
    }

    /// Poll and process any pending non-event responses from the event bridge.
    /// Should be called during UI update cycles to deliver ClassifyResult,
    /// CompleteResult, HistoryResult, etc. to pending request handlers.
    /// Returns any side effects that the caller must act on.
    pub fn poll_responses(&mut self) -> Vec<ResponseEffect> {
        let mut effects = Vec::new();
        loop {
            match self.response_rx.try_recv() {
                Ok(response) => {
                    let effect = self.handle_response(response);
                    if !matches!(effect, ResponseEffect::None) {
                        effects.push(effect);
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    // Event bridge is gone — connection lost
                    if self.state == ConnectionState::Connected {
                        self.state = ConnectionState::Disconnected;
                    }
                    break;
                }
            }
        }
        effects
    }

    /// Allocate the next request ID.
    fn next_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    /// Send a request to the remote agent (fire-and-forget).
    ///
    /// If the transport channel is closed, transitions to `Disconnected`.
    pub fn send(&mut self, request: Request) {
        if self.request_tx.send(RequestEnvelope { request }).is_err() {
            self.state = ConnectionState::Disconnected;
        }
    }

    /// Execute a command on the remote agent.
    pub fn execute(&mut self, command: String, block_id: BlockId) {
        if self.state != ConnectionState::Connected {
            self.pending_queue.push(QueuedCommand { command, block_id });
            return;
        }

        let id = self.next_id();
        self.send(Request::Execute {
            id,
            command,
            block_id,
        });
    }

    /// Request tab completions from the remote agent (async).
    pub fn complete(
        &mut self,
        input: &str,
        cursor: usize,
    ) -> oneshot::Receiver<(Vec<CompletionItem>, usize)> {
        let (tx, rx) = oneshot::channel();
        let id = self.next_id();
        self.pending.insert(id, PendingRequest::Complete(tx));
        self.send(Request::Complete {
            id,
            input: input.to_string(),
            cursor,
        });
        rx
    }

    /// Search history on the remote agent (async).
    pub fn search_history(
        &mut self,
        query: &str,
        limit: u32,
    ) -> oneshot::Receiver<Vec<HistoryEntry>> {
        let (tx, rx) = oneshot::channel();
        let id = self.next_id();
        self.pending.insert(id, PendingRequest::SearchHistory(tx));
        self.send(Request::SearchHistory {
            id,
            query: query.to_string(),
            limit,
        });
        rx
    }

    /// Send a terminal resize to the remote agent.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.send(Request::TerminalResize { cols, rows });
    }

    /// Request graceful shutdown.
    pub fn shutdown(&mut self) {
        self.send(Request::Shutdown);
    }

    /// Send interrupt to a block.
    pub fn cancel_block(&mut self, block_id: BlockId) {
        let id = self.next_id();
        self.send(Request::CancelBlock { id, block_id });
    }

    /// Spawn a PTY on the remote agent.
    pub fn pty_spawn(
        &mut self,
        command: &str,
        block_id: BlockId,
        cols: u16,
        rows: u16,
    ) {
        let id = self.next_id();
        self.pending.insert(id, PendingRequest::PtySpawn(block_id));
        self.send(Request::PtySpawn {
            id,
            command: command.to_string(),
            block_id,
            cols,
            rows,
            term: "xterm-256color".to_string(),
        });
    }

    /// Send input to a remote PTY.
    pub fn pty_input(&mut self, block_id: BlockId, data: Vec<u8>) {
        self.send(Request::PtyInput { block_id, data });
    }

    /// Resize a remote PTY.
    pub fn pty_resize(&mut self, block_id: BlockId, cols: u16, rows: u16) {
        self.send(Request::PtyResize {
            block_id,
            cols,
            rows,
        });
    }

    /// Handle a response from the remote agent.
    /// Returns a `ResponseEffect` that the caller may need to act on.
    pub fn handle_response(&mut self, response: Response) -> ResponseEffect {
        match response {
            Response::CompleteResult {
                id,
                completions,
                start,
            } => {
                if let Some(PendingRequest::Complete(tx)) = self.pending.remove(&id) {
                    let _ = tx.send((completions, start));
                }
                ResponseEffect::None
            }
            Response::HistoryResult { id, entries } => {
                if let Some(PendingRequest::SearchHistory(tx)) = self.pending.remove(&id) {
                    let _ = tx.send(entries);
                }
                ResponseEffect::None
            }
            Response::NestOk { id, env } => {
                if let Some(PendingRequest::Nest) = self.pending.remove(&id) {
                    self.backend_stack.push(BackendEntry {
                        env: self.env.clone(),
                    });
                    self.env = env;
                }
                ResponseEffect::None
            }
            Response::UnnestOk { id, env } => {
                self.pending.remove(&id);
                self.backend_stack.pop();
                self.env = env;
                ResponseEffect::None
            }
            Response::ChildLost { reason } => {
                tracing::warn!("remote child lost: {reason}");
                if let Some(previous) = self.backend_stack.pop() {
                    self.env = previous.env;
                }
                ResponseEffect::None
            }
            Response::Pong { seq: _ } => {
                // RTT tracking handled by event bridge
                ResponseEffect::None
            }
            Response::Error { id, message } => {
                tracing::error!("remote error (id={id}): {message}");
                match self.pending.remove(&id) {
                    Some(PendingRequest::PtySpawn(block_id)) => {
                        ResponseEffect::PtySpawnFailed { block_id, message }
                    }
                    _ => ResponseEffect::None,
                }
            }
            _ => ResponseEffect::None,
        }
    }

    /// Flush the offline command queue after reconnection.
    pub fn flush_queue(&mut self) {
        let queue = std::mem::take(&mut self.pending_queue);
        for cmd in queue {
            self.execute(cmd.command, cmd.block_id);
        }
    }

    /// Nest into a deeper level.
    pub fn nest(&mut self, transport: Transport) {
        let id = self.next_id();
        self.pending.insert(id, PendingRequest::Nest);
        self.send(Request::Nest {
            id,
            transport,
            force_redeploy: false,
        });
    }

    /// Unnest from the current level.
    pub fn unnest(&mut self) {
        let id = self.next_id();
        self.send(Request::Unnest { id });
    }

    /// Swap the request channel to a new transport (after reconnection).
    pub fn swap_request_tx(&mut self, new_tx: mpsc::UnboundedSender<RequestEnvelope>) {
        self.request_tx = new_tx;
    }

    /// Kill the SSH/docker/kubectl child process (sync — sends SIGKILL without awaiting).
    pub fn kill_child_sync(&mut self) {
        if let Some(ref mut child) = self.child {
            let _ = child.start_kill();
        }
        self.child = None;
    }

    /// Check if the child process is still alive.
    /// Returns `true` if alive (or if there's no child to check).
    pub fn check_child_alive(&mut self) -> bool {
        self.child
            .as_mut()
            .map_or(true, |c| c.try_wait().ok().flatten().is_none())
    }
}
