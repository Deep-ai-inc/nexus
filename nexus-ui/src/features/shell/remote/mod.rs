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
    Nest(BlockId),
    PtySpawn(BlockId),
    FileRead {
        tx: oneshot::Sender<Vec<u8>>,
        buffer: Vec<u8>,
    },
    /// One entry per chunk — all share the same accumulator.
    /// The last chunk to complete fires the oneshot.
    FileWriteChunk(Arc<std::sync::Mutex<FileWriteAccum>>),
    ListDir(oneshot::Sender<Vec<nexus_api::FileEntry>>),
}

/// Shared accumulator for multi-chunk file writes.
///
/// Each `FileWriteOk` response adds to `total_written` and decrements
/// `chunks_remaining`. When it reaches zero, the oneshot fires.
struct FileWriteAccum {
    total_written: u64,
    chunks_remaining: usize,
    tx: Option<oneshot::Sender<u64>>,
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
    /// CWD changed (from NestOk, UnnestOk, or ChildLost).
    CwdChanged { cwd: std::path::PathBuf },
    /// Nest request failed — the caller should emit error + finish on the block.
    NestFailed { block_id: BlockId, message: String },
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
    /// Timestamp of the last received Pong (shared with event bridge).
    /// Used to detect stale connections where SSH is alive but data isn't flowing.
    pub last_pong_at: Arc<AtomicU64>,
    /// Receiver for non-event responses from the event bridge.
    pub(crate) response_rx: mpsc::UnboundedReceiver<Response>,
    /// The SSH/docker/kubectl child process (owned for lifecycle management).
    child: Option<tokio::process::Child>,
    /// Transport used to establish this connection (for reconnection).
    pub(crate) transport: Transport,
    /// Path to the agent binary on the remote host.
    pub(crate) agent_path: String,
    /// Session token from the last HelloOk (stored for future resume support).
    pub(crate) session_token: [u8; 16],
    /// Tracks the expected surviving agent's instance_id during an intentional Unnest.
    /// When ChildLost arrives and matches this, no error toast is shown.
    pending_unnest_target: Option<String>,
    /// Monotonically increasing echo epoch counter for local echo prediction.
    /// Incremented on every PtyInput; the agent reflects it back on StdoutChunk.
    echo_epoch: u64,
    /// PTY inputs buffered while disconnected. Flushed in order on reconnect.
    pending_inputs: Vec<QueuedInput>,
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

/// A PTY input queued while disconnected. Carries the pre-assigned epoch
/// so predictions stay consistent across reconnect.
#[derive(Debug, Clone)]
struct QueuedInput {
    block_id: BlockId,
    data: Vec<u8>,
    echo_epoch: u64,
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
        last_pong_at: Arc<AtomicU64>,
        last_seen_seq: Arc<AtomicU64>,
        response_rx: mpsc::UnboundedReceiver<Response>,
        child: Option<tokio::process::Child>,
        transport: Transport,
        agent_path: String,
        session_token: [u8; 16],
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
            last_pong_at,
            response_rx,
            child,
            transport,
            agent_path,
            session_token,
            pending_unnest_target: None,
            echo_epoch: 0,
            pending_inputs: Vec::new(),
        }
    }

    /// Read the current RTT in milliseconds (0 = not yet measured).
    pub fn current_rtt_ms(&self) -> u64 {
        self.rtt_ms.load(Ordering::Relaxed)
    }

    /// Check if the connection is stale (no Pong received recently).
    /// Returns true if data is still flowing, false if connection appears dead.
    pub fn is_data_flowing(&self) -> bool {
        let last = self.last_pong_at.load(Ordering::Relaxed);
        if last == 0 {
            return true; // No pongs yet — still in handshake
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        // If no Pong in 10 seconds (pings sent every 500ms), connection is dead
        now_ms.saturating_sub(last) < 10_000
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

    /// List a directory on the remote agent (async).
    ///
    /// Returns a oneshot receiver that resolves with the directory entries.
    /// The `block_id` and `path` are stored so the caller can match the
    /// response back to the tree node, but are not sent over the wire.
    pub fn list_dir(
        &mut self,
        path: std::path::PathBuf,
        _block_id: BlockId,
    ) -> Option<oneshot::Receiver<Vec<nexus_api::FileEntry>>> {
        if self.state != ConnectionState::Connected {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        let id = self.next_id();
        self.pending.insert(id, PendingRequest::ListDir(tx));
        self.send(Request::ListDir {
            id,
            path: path.to_string_lossy().into_owned(),
        });
        Some(rx)
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
        cwd: &str,
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
            cwd: cwd.to_string(),
        });
    }

    /// Send input to a remote PTY.
    /// Increments the echo epoch so the agent can reflect it back
    /// for local echo prediction confirmation. Returns the assigned epoch.
    ///
    /// When disconnected, the input is buffered locally and flushed on
    /// reconnect. The epoch is still assigned so predictions remain valid.
    pub fn pty_input(&mut self, block_id: BlockId, data: Vec<u8>) -> u64 {
        self.echo_epoch += 1;
        let epoch = self.echo_epoch;

        if self.state == ConnectionState::Connected {
            self.send(Request::PtyInput {
                block_id,
                data,
                echo_epoch: epoch,
            });
        } else {
            // Buffer for reconnect — predictions still render locally
            self.pending_inputs.push(QueuedInput {
                block_id,
                data,
                echo_epoch: epoch,
            });
        }

        epoch
    }

    /// Resize a remote PTY.
    pub fn pty_resize(&mut self, block_id: BlockId, cols: u16, rows: u16) {
        self.send(Request::PtyResize {
            block_id,
            cols,
            rows,
        });
    }

    /// Send a signal to a remote PTY (e.g. SIGKILL for force-kill).
    ///
    /// API surface for future "Force Kill" UI (e.g. double-Ctrl+C).
    /// The normal KillBlock path uses cancel_block() → SIGINT which is correct
    /// for graceful interrupt. This method is for escalation.
    #[allow(dead_code)]
    pub fn pty_kill(&mut self, block_id: BlockId, signal: i32) {
        self.send(Request::PtyKill { block_id, signal });
    }

    /// Close a remote PTY (hangup the master fd).
    ///
    /// API surface for future use — closes the PTY without signaling the process.
    #[allow(dead_code)]
    pub fn pty_close(&mut self, block_id: BlockId) {
        self.send(Request::PtyClose { block_id });
    }

    /// Read a file from the remote agent.
    ///
    /// Returns a oneshot receiver that resolves with the full file contents.
    /// Accumulates chunked `FileData` responses internally. Capped at 50 MB
    /// to prevent OOM — larger files will error on the receiver side.
    #[allow(dead_code)]
    pub fn file_read(&mut self, path: &str) -> oneshot::Receiver<Vec<u8>> {
        let (tx, rx) = oneshot::channel();
        let id = self.next_id();
        self.pending.insert(
            id,
            PendingRequest::FileRead {
                tx,
                buffer: Vec::new(),
            },
        );
        self.send(Request::FileRead {
            id,
            path: path.to_string(),
            offset: 0,
            len: None,
        });
        rx
    }

    /// Cancel an in-progress file read on the remote agent.
    ///
    /// Sends `CancelFileRead` so the agent stops sending chunks for this ID.
    #[allow(dead_code)]
    fn cancel_file_read(&mut self, id: u32) {
        self.send(Request::CancelFileRead { id });
    }

    /// Write a file to the remote agent.
    ///
    /// Splits data into 16 KB chunks (MAX_FRAME_PAYLOAD) and sends them via a
    /// background tokio task that yields between chunks to avoid blocking the
    /// UI thread or bloating the unbounded channel with megabytes at once.
    ///
    /// Every chunk's ID is tracked in `pending` with a shared `FileWriteAccum`.
    /// The agent replies `FileWriteOk` per chunk; the accumulator aggregates
    /// `bytes_written` and fires the oneshot when all chunks are acknowledged.
    ///
    // TODO: Large uploads spike local memory because `request_tx` is unbounded
    // and `yield_now()` doesn't provide real backpressure — the runtime may
    // poll the spawned task to completion before the transport drains the
    // channel. A bounded channel or explicit semaphore would be needed to
    // cap in-flight data.
    #[allow(dead_code)]
    pub fn file_write(&mut self, path: &str, data: &[u8]) -> oneshot::Receiver<u64> {
        let (tx, rx) = oneshot::channel();

        if data.is_empty() {
            let _ = tx.send(0);
            return rx;
        }

        const CHUNK_SIZE: usize = 16 * 1024; // MAX_FRAME_PAYLOAD
        let num_chunks = (data.len() + CHUNK_SIZE - 1) / CHUNK_SIZE;

        let accum = Arc::new(std::sync::Mutex::new(FileWriteAccum {
            total_written: 0,
            chunks_remaining: num_chunks,
            tx: Some(tx),
        }));

        // Pre-allocate all chunk IDs and build envelopes synchronously so IDs
        // are sequential and registered in `pending` before any response arrives.
        let mut envelopes = Vec::with_capacity(num_chunks);
        for (i, chunk) in data.chunks(CHUNK_SIZE).enumerate() {
            let id = self.next_id();
            self.pending
                .insert(id, PendingRequest::FileWriteChunk(accum.clone()));
            envelopes.push(RequestEnvelope {
                request: Request::FileWrite {
                    id,
                    path: path.to_string(),
                    offset: (i * CHUNK_SIZE) as u64,
                    data: chunk.to_vec(),
                },
            });
        }

        // Send the first chunk immediately so small writes don't wait a tick.
        if let Some(first) = envelopes.first() {
            if self
                .request_tx
                .send(RequestEnvelope {
                    request: first.request.clone(),
                })
                .is_err()
            {
                self.state = ConnectionState::Disconnected;
                return rx;
            }
        }

        // Remaining chunks are sent from a background task with yielding.
        if envelopes.len() > 1 {
            let sender = self.request_tx.clone();
            let remaining: Vec<RequestEnvelope> = envelopes.into_iter().skip(1).collect();
            tokio::spawn(async move {
                for envelope in remaining {
                    if sender.send(envelope).is_err() {
                        break; // Transport gone
                    }
                    tokio::task::yield_now().await;
                }
            });
        }

        rx
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
                if let Some(PendingRequest::Nest(_)) = self.pending.remove(&id) {
                    self.backend_stack.push(BackendEntry {
                        env: self.env.clone(),
                    });
                    let cwd = env.cwd.clone();
                    self.env = env;
                    return ResponseEffect::CwdChanged { cwd };
                }
                ResponseEffect::None
            }
            Response::UnnestOk { id, env } => {
                self.pending.remove(&id);
                self.backend_stack.pop();
                let cwd = env.cwd.clone();
                self.env = env;
                ResponseEffect::CwdChanged { cwd }
            }
            Response::ChildLost {
                reason,
                surviving_env,
            } => {
                // Pop entries ABOVE the surviving agent
                while let Some(top) = self.backend_stack.pop() {
                    if top.env.instance_id == surviving_env.instance_id {
                        break;
                    }
                }
                let cwd = surviving_env.cwd.clone();
                self.env = surviving_env;

                // Only show warning if this wasn't an intentional Unnest
                let is_clean = self
                    .pending_unnest_target
                    .take()
                    .map_or(false, |target| target == self.env.instance_id);
                if !is_clean {
                    tracing::warn!("nested connection lost: {reason}");
                }
                ResponseEffect::CwdChanged { cwd }
            }
            Response::Pong { seq: _ } => {
                // RTT tracking handled by event bridge
                ResponseEffect::None
            }
            Response::FileData { id, data, eof } => {
                // Accumulate chunks; only complete on eof.
                if let Some(pending) = self.pending.get_mut(&id) {
                    if let PendingRequest::FileRead { buffer, .. } = pending {
                        buffer.extend_from_slice(&data);
                        // OOM guard: cap at 50 MB — tell the agent to stop sending
                        if buffer.len() > 50 * 1024 * 1024 {
                            tracing::warn!("file read exceeded 50 MB cap, cancelling");
                            self.pending.remove(&id);
                            self.cancel_file_read(id);
                            return ResponseEffect::None;
                        }
                        if eof {
                            if let Some(PendingRequest::FileRead { tx, buffer }) =
                                self.pending.remove(&id)
                            {
                                let _ = tx.send(buffer);
                            }
                        }
                    }
                }
                ResponseEffect::None
            }
            Response::FileWriteOk { id, bytes_written } => {
                if let Some(PendingRequest::FileWriteChunk(accum)) = self.pending.remove(&id) {
                    let mut state = accum.lock().unwrap();
                    state.total_written += bytes_written;
                    state.chunks_remaining -= 1;
                    if state.chunks_remaining == 0 {
                        if let Some(tx) = state.tx.take() {
                            let _ = tx.send(state.total_written);
                        }
                    }
                }
                ResponseEffect::None
            }
            Response::ListDirResult { id, entries } => {
                if let Some(PendingRequest::ListDir(tx)) = self.pending.remove(&id) {
                    let _ = tx.send(entries);
                }
                ResponseEffect::None
            }
            Response::Error { id, message } => {
                tracing::error!("remote error (id={id}): {message}");
                match self.pending.remove(&id) {
                    Some(PendingRequest::PtySpawn(block_id)) => {
                        ResponseEffect::PtySpawnFailed { block_id, message }
                    }
                    Some(PendingRequest::Nest(block_id)) => {
                        ResponseEffect::NestFailed { block_id, message }
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
        // Flood buffered PTY inputs — predictions will reconcile
        // against the server's echoed epochs.
        let inputs = std::mem::take(&mut self.pending_inputs);
        for input in inputs {
            self.send(Request::PtyInput {
                block_id: input.block_id,
                data: input.data,
                echo_epoch: input.echo_epoch,
            });
        }
    }

    /// Nest into a deeper level.
    pub fn nest(&mut self, transport: Transport, block_id: BlockId) {
        let id = self.next_id();
        self.pending.insert(id, PendingRequest::Nest(block_id));
        self.send(Request::Nest {
            id,
            transport,
            force_redeploy: false,
        });
    }

    /// Unnest from the current level.
    pub fn unnest(&mut self) {
        // Track the expected surviving agent for ChildLost disambiguation
        self.pending_unnest_target = self
            .backend_stack
            .last()
            .map(|entry| entry.env.instance_id.clone());
        let id = self.next_id();
        self.send(Request::Unnest { id });
    }

    /// Swap the request channel to a new transport (after reconnection).
    pub fn swap_request_tx(&mut self, new_tx: mpsc::UnboundedSender<RequestEnvelope>) {
        self.request_tx = new_tx;
    }

    /// Replace the child process (after reconnection).
    pub fn set_child(&mut self, child: Option<tokio::process::Child>) {
        self.child = child;
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
