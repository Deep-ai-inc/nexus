//! Core agent loop: owns a Kernel, reads Requests, writes Responses.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Result;
use nexus_api::{BlockId, ShellEvent};
use nexus_kernel::Kernel;
use nexus_protocol::codec::{decode_payload, encode_payload, FrameCodec, FrameReader, FrameWriter, FLAG_EVENT};
use nexus_protocol::messages::*;
use nexus_protocol::priority;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{broadcast, mpsc, Mutex, Semaphore};

use crate::pty::PtyManager;
use crate::relay::{self, ActiveRelay};
use crate::session::RingBuffer;

/// Exit reason from the relay loop.
enum RelayExit {
    /// Child agent died or its pipe closed.
    ChildLost(String),
    /// Parent disconnected (reader returned ConnectionClosed).
    ParentDisconnected,
    /// Client sent Shutdown (clean exit).
    Shutdown,
}

/// The remote agent. Wraps a `Kernel` and speaks the Nexus wire protocol.
pub struct Agent {
    /// Unique identifier for this agent session (UUID v4).
    instance_id: String,
    kernel: Arc<Mutex<Kernel>>,
    kernel_rx: broadcast::Receiver<ShellEvent>,
    idle_timeout_secs: u64,
    /// Read timeout for client connections (seconds). Detects dead TCP.
    read_timeout_secs: u64,
    /// Monotonically increasing sequence number for outbound events.
    next_seq: Arc<AtomicU64>,
    /// Ring buffer for session resume (always shared — events are collected
    /// even between client connections so nothing is lost on disconnect).
    ring_buffer: Arc<tokio::sync::Mutex<RingBuffer>>,
    /// Shared sender for the bg collector to forward encoded events to the wire sender.
    /// Wrapped in Arc so the bg_collector and Agent can both reference it.
    /// Swapped to a new channel each run() so the sender task gets a fresh receiver.
    bg_wire_tx: Arc<std::sync::Mutex<mpsc::UnboundedSender<(u64, Vec<u8>)>>>,
    /// Receiver end, moved into each run() call's sender task.
    wire_rx: Option<mpsc::UnboundedReceiver<(u64, Vec<u8>)>>,
    /// Handle to the persistent background collector task.
    _bg_collector: tokio::task::JoinHandle<()>,
    /// Terminal viewport dimensions (for native commands like `ls`).
    viewport_cols: u16,
    viewport_rows: u16,
    /// PTY session manager.
    pty_manager: PtyManager,
    /// Session token for resume validation.
    session_token: Option<[u8; 16]>,
    /// Credit-based flow control semaphore.
    /// Permits represent bytes the client has granted for sending.
    credits: Arc<Semaphore>,
    /// IDs of file reads that have been cancelled by the client.
    /// Checked by spawned file-read tasks on each chunk iteration.
    cancelled_reads: Arc<std::sync::Mutex<std::collections::HashSet<u32>>>,
    /// Active relay to a nested child agent (persists across disconnects).
    active_relay: Option<ActiveRelay>,
    /// Environment variables forwarded from the client (for nesting).
    forwarded_env: HashMap<String, String>,
}

impl Agent {
    pub fn new(idle_timeout_secs: u64, read_timeout_secs: u64) -> Result<Self> {
        let (kernel, kernel_rx) = Kernel::new()?;
        let kernel = Arc::new(Mutex::new(kernel));

        let next_seq = Arc::new(AtomicU64::new(1));
        let ring_buffer = Arc::new(tokio::sync::Mutex::new(
            RingBuffer::new(4 * 1024 * 1024), // 4 MB
        ));

        // Persistent background collector: subscribes to kernel events and
        // pushes them to the ring buffer with assigned seq numbers. Runs for
        // the entire lifetime of the agent, including between client connections.
        let bg_next_seq = next_seq.clone();
        let bg_ring = ring_buffer.clone();
        let bg_kernel_tx = {
            kernel.try_lock().unwrap().event_sender().clone()
        };
        let mut bg_event_rx = bg_kernel_tx.subscribe();
        let (wire_tx, wire_rx) = mpsc::unbounded_channel::<(u64, Vec<u8>)>();
        let bg_wire_tx = Arc::new(std::sync::Mutex::new(wire_tx));
        let bg_wire_tx_clone = bg_wire_tx.clone();
        let bg_collector = tokio::spawn(async move {
            loop {
                match bg_event_rx.recv().await {
                    Ok(event) => {
                        let seq = bg_next_seq.fetch_add(1, Ordering::Relaxed);
                        let resp = Response::Event { seq, event };
                        match encode_payload(&resp) {
                            Ok(payload) => {
                                bg_ring.lock().await.push_raw(seq, payload.clone());
                                // Best-effort forward to wire sender (fails if no sender active)
                                let _ = bg_wire_tx_clone.lock().unwrap().send((seq, payload));
                            }
                            Err(e) => {
                                tracing::warn!("bg collector: failed to encode event: {e}");
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("bg collector lagged by {n} events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        Ok(Self {
            instance_id: uuid::Uuid::new_v4().to_string(),
            kernel,
            kernel_rx,
            idle_timeout_secs,
            read_timeout_secs,
            next_seq,
            ring_buffer,
            bg_wire_tx,
            wire_rx: Some(wire_rx),
            _bg_collector: bg_collector,
            viewport_cols: 80,
            viewport_rows: 24,
            pty_manager: PtyManager::new(),
            session_token: None,
            credits: Arc::new(Semaphore::new(0)),
            cancelled_reads: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            active_relay: None,
            forwarded_env: HashMap::new(),
        })
    }

    /// Returns the unique instance ID for this agent session.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Returns the shared set of PIDs managed by Tokio's child waiter.
    /// The zombie reaper uses this to avoid stealing exit statuses.
    pub fn tokio_pids(&self) -> Arc<std::sync::Mutex<std::collections::HashSet<u32>>> {
        self.pty_manager.tokio_pids()
    }

    /// Exit statuses saved by the zombie reaper (for ECHILD recovery).
    pub fn reaped_statuses(&self) -> Arc<std::sync::Mutex<std::collections::HashMap<u32, i32>>> {
        self.pty_manager.reaped_statuses()
    }

    /// Returns true if the agent should persist after a parent disconnect.
    /// Persists if there's an active relay, running PTY sessions, or we've
    /// had at least one successful client handshake (so reconnect can find us).
    pub fn should_persist(&self) -> bool {
        self.active_relay.is_some()
            || self.pty_manager.has_active_sessions()
            || self.session_token.is_some()
    }

    /// Main loop: read requests from `input`, write responses to `output`.
    pub async fn run<R, W>(&mut self, input: R, output: W) -> Result<()>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        self.run_cancellable(input, output, tokio_util::sync::CancellationToken::new()).await
    }

    /// Main loop with external cancellation support.
    /// When `cancel` fires, the agent breaks out immediately (used for UDS takeover).
    pub async fn run_cancellable<R, W>(
        &mut self,
        input: R,
        output: W,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<()>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        // Drain stale credits from a previous connection
        let stale = self.credits.available_permits();
        if stale > 0 {
            let _ = self.credits.acquire_many(stale as u32).await;
            // permits are dropped immediately, resetting to 0
        }

        let codec = FrameCodec::new(input, output);
        let (mut reader, writer) = codec.into_parts();
        let writer = Arc::new(tokio::sync::Mutex::new(writer));

        let ring_buffer = self.ring_buffer.clone();
        let next_seq = self.next_seq.clone();
        let credits = self.credits.clone();
        let kernel_tx = self.kernel.lock().await.event_sender().clone();

        // =====================================================================
        // Wire sender: reads pre-encoded events from bg_collector via wire_rx,
        // acquires flow-control credits, and writes to the transport.
        // The bg_collector (persistent) handles seq assignment + ring buffer.
        // =====================================================================

        let event_writer = writer.clone();
        let sender_credits = credits.clone();
        // Take wire_rx for this run; it will be recreated if needed
        let mut wire_rx = self.wire_rx.take()
            .expect("wire_rx should be available at start of run()");

        let sender_task = tokio::spawn(async move {
            while let Some((_seq, payload)) = wire_rx.recv().await {
                // Acquire credits — wait briefly, then skip wire write if none available
                let size_est = payload.len();
                match sender_credits.try_acquire_many(size_est.max(1) as u32) {
                    Ok(permit) => permit.forget(),
                    Err(tokio::sync::TryAcquireError::Closed) => return,
                    Err(tokio::sync::TryAcquireError::NoPermits) => {
                        match tokio::time::timeout(
                            Duration::from_millis(500),
                            sender_credits.acquire_many(size_est.max(1) as u32),
                        ).await {
                            Ok(Ok(permit)) => permit.forget(),
                            Ok(Err(_)) => return,
                            Err(_) => continue, // skip this frame on wire, it's in ring buffer
                        }
                    }
                }
                let mut w = event_writer.lock().await;
                if w.write_raw_flagged(&payload, priority::INTERACTIVE, FLAG_EVENT)
                    .await
                    .is_err()
                {
                    return; // Connection lost
                }
            }
        });

        // =====================================================================
        // Main request loop
        // =====================================================================
        loop {
            // If we have an active relay, enter relay mode
            if let Some(mut active_relay) = self.active_relay.take() {
                let exit = Self::relay_loop(
                    &mut reader,
                    &mut active_relay.child_writer,
                    &mut active_relay.child_lost_rx,
                    &writer,
                    &self.credits,
                    &mut self.viewport_cols,
                    &mut self.viewport_rows,
                    &self.kernel,
                    &self.instance_id,
                    &cancel,
                    self.read_timeout_secs,
                )
                .await;

                match exit {
                    RelayExit::ChildLost(reason) => {
                        tracing::info!("child lost: {reason}");
                        active_relay.cleanup(&self.pty_manager.tokio_pids()).await;
                        // Fall through to normal dispatch
                    }
                    RelayExit::ParentDisconnected => {
                        // Relay stays alive for reconnection
                        self.active_relay = Some(active_relay);
                        break;
                    }
                    RelayExit::Shutdown => {
                        active_relay.cleanup(&self.pty_manager.tokio_pids()).await;
                        break;
                    }
                }
                continue;
            }

            // Read with timeout and cancellation support.
            // - read timeout: detects dead TCP without waiting for kernel keepalive
            // - cancel token: allows UDS takeover to interrupt immediately
            let request: Request = match tokio::select! {
                result = tokio::time::timeout(Duration::from_secs(self.read_timeout_secs), reader.read()) => result,
                _ = cancel.cancelled() => {
                    tracing::info!("connection cancelled (UDS takeover)");
                    break;
                }
            } {
                Ok(Ok(req)) => req,
                Ok(Err(nexus_protocol::codec::CodecError::ConnectionClosed)) => {
                    tracing::info!("client disconnected");
                    break;
                }
                Ok(Err(e)) => {
                    tracing::error!("read error: {e}");
                    break;
                }
                Err(_) => {
                    tracing::info!("read timeout ({}s), assuming dead connection", self.read_timeout_secs);
                    break;
                }
            };

            // Macro to handle write results: treat errors as connection loss → break
            macro_rules! try_write {
                ($expr:expr) => {
                    if $expr.await.is_err() {
                        tracing::info!("write failed, client disconnected");
                        break;
                    }
                };
            }

            match request {
                Request::Hello {
                    protocol_version: _,
                    capabilities: _,
                    forwarded_env,
                } => {
                    if self.handle_hello(forwarded_env, &writer).await.is_err() {
                        break;
                    }
                }

                Request::Execute {
                    id,
                    command,
                    block_id,
                } => {
                    self.handle_execute(id, command, block_id, &writer).await;
                }

                Request::Classify { id, command } => {
                    if self.handle_classify(id, &command, &writer).await.is_err() {
                        break;
                    }
                }

                Request::Complete { id, input, cursor } => {
                    if self.handle_complete(id, &input, cursor, &writer).await.is_err() {
                        break;
                    }
                }

                Request::SearchHistory { id, query, limit } => {
                    if self.handle_search_history(id, &query, limit, &writer).await.is_err() {
                        break;
                    }
                }

                Request::CancelBlock { id: _, block_id } => {
                    nexus_kernel::commands::cancel_block(block_id);
                    let _ = self.pty_manager.kill(block_id, libc::SIGINT);
                }

                Request::PtySpawn {
                    id,
                    command,
                    block_id,
                    cols,
                    rows,
                    term,
                    cwd,
                } => {
                    if let Err(e) = self
                        .pty_manager
                        .spawn(&command, block_id, cols, rows, &term, &cwd, &kernel_tx)
                        .await
                    {
                        let resp = Response::Error {
                            id,
                            message: format!("PTY spawn failed: {e}"),
                        };
                        let mut w = writer.lock().await;
                        try_write!(w.write(&resp, resp.priority()));
                    }
                }

                Request::PtyInput { block_id, data, echo_epoch } => {
                    if let Err(e) = self.pty_manager.input(block_id, &data, echo_epoch).await {
                        tracing::warn!("pty input error for {:?}: {e}", block_id);
                    }
                }

                Request::PtyResize {
                    block_id,
                    cols,
                    rows,
                } => {
                    if let Err(e) = self.pty_manager.resize(block_id, cols, rows) {
                        tracing::warn!("pty resize error for {:?}: {e}", block_id);
                    }
                }

                Request::PtyKill { block_id, signal } => {
                    if let Err(e) = self.pty_manager.kill(block_id, signal) {
                        tracing::warn!("pty kill error for {:?}: {e}", block_id);
                    }
                }

                Request::PtyClose { block_id } => {
                    if let Err(e) = self.pty_manager.close(block_id) {
                        tracing::warn!("pty close error for {:?}: {e}", block_id);
                    }
                }

                Request::TerminalResize { cols, rows } => {
                    self.viewport_cols = cols;
                    self.viewport_rows = rows;
                    let mut kernel = self.kernel.lock().await;
                    kernel
                        .state_mut()
                        .set_env("COLUMNS".to_string(), cols.to_string());
                    kernel
                        .state_mut()
                        .set_env("LINES".to_string(), rows.to_string());
                }

                Request::CancelFileRead { id } => {
                    self.cancelled_reads.lock().unwrap().insert(id);
                }

                Request::FileRead {
                    id,
                    path,
                    offset,
                    len,
                } => {
                    let writer = writer.clone();
                    let cancelled = self.cancelled_reads.clone();
                    tokio::spawn(async move {
                        if let Err(e) =
                            Self::handle_file_read_task(id, &path, offset, len, &writer, &cancelled)
                                .await
                        {
                            tracing::error!("file read error (id={id}): {e}");
                        }
                        cancelled.lock().unwrap().remove(&id);
                    });
                }

                Request::ListDir { id, path } => {
                    let writer = writer.clone();
                    tokio::spawn(async move {
                        // Do blocking fs work on a blocking thread, then send response async.
                        let result = tokio::task::spawn_blocking(move || {
                            match std::fs::read_dir(&path) {
                                Ok(read_dir) => {
                                    let mut entries = Vec::new();
                                    for entry in read_dir.flatten() {
                                        if let Ok(fe) = nexus_api::FileEntry::from_path(entry.path()) {
                                            entries.push(fe);
                                        }
                                    }
                                    entries.sort_by(|a, b| {
                                        a.name.to_lowercase().cmp(&b.name.to_lowercase())
                                    });
                                    Ok(entries)
                                }
                                Err(e) => Err(format!("failed to read directory: {e}")),
                            }
                        }).await;

                        let resp = match result {
                            Ok(Ok(entries)) => Response::ListDirResult { id, entries },
                            Ok(Err(msg)) => Response::Error { id, message: msg },
                            Err(e) => Response::Error { id, message: format!("task failed: {e}") },
                        };
                        let mut w = writer.lock().await;
                        let _ = w.write(&resp, resp.priority()).await;
                    });
                }

                Request::FileWrite {
                    id,
                    path,
                    offset,
                    data,
                } => {
                    if self.handle_file_write(id, &path, offset, &data, &writer).await.is_err() {
                        break;
                    }
                }

                Request::Nest {
                    id,
                    transport,
                    force_redeploy: _,
                } => {
                    match relay::spawn_and_handshake(&transport, self.forwarded_env.clone()).await {
                        Ok((child_reader, child_writer, child, env)) => {
                            // Register relay child PID as Tokio-managed
                            if let Some(pid) = child.id() {
                                self.pty_manager.tokio_pids().lock().unwrap().insert(pid);
                            }
                            let (reader_task, child_lost_rx) = relay::start_relay_reader(
                                child_reader,
                                writer.clone(),
                                credits.clone(),
                                next_seq.clone(),
                                ring_buffer.clone(),
                            );
                            self.active_relay = Some(ActiveRelay {
                                child,
                                child_writer,
                                reader_task,
                                child_lost_rx,
                            });
                            let resp = Response::NestOk { id, env };
                            let mut w = writer.lock().await;
                            try_write!(w.write(&resp, resp.priority()));
                            // Next iteration enters relay mode
                        }
                        Err(e) => {
                            let resp = Response::Error {
                                id,
                                message: format!("Nest failed: {e}"),
                            };
                            let mut w = writer.lock().await;
                            try_write!(w.write(&resp, resp.priority()));
                        }
                    }
                }

                Request::Unnest { id: _ } => {
                    // Leaf node: shut down cleanly.
                    // Parent detects child pipe EOF → sends ChildLost to its parent.
                    self.pty_manager.shutdown_all();
                    self.session_token = None;
                    break;
                }

                Request::GrantCredits { bytes } => {
                    self.credits.add_permits(bytes as usize);
                }

                Request::Ping { seq } => {
                    let resp = Response::Pong { seq };
                    let mut w = writer.lock().await;
                    try_write!(w.write(&resp, priority::CONTROL));
                }

                Request::Resume {
                    session_token,
                    last_seen_seq,
                    cols,
                    rows,
                } => {
                    if self.handle_resume(session_token, last_seen_seq, cols, rows, &writer, &ring_buffer, &next_seq)
                        .await.is_err() {
                        break;
                    }
                }

                Request::Shutdown => {
                    tracing::info!("received shutdown request");
                    self.pty_manager.shutdown_all();
                    // Clear session token so we don't persist after clean shutdown
                    self.session_token = None;
                    break;
                }
            }
        }

        // Abort the sender task (it only writes to the wire)
        sender_task.abort();
        let _ = tokio::time::timeout(Duration::from_secs(2), sender_task).await;

        // Recreate the wire channel for the next run() call.
        // Swap the sender in the bg_collector's shared Arc so new events
        // go to the new receiver.
        let (new_tx, new_rx) = mpsc::unbounded_channel();
        *self.bg_wire_tx.lock().unwrap() = new_tx;
        self.wire_rx = Some(new_rx);

        Ok(())
    }

    /// Relay loop: forward requests between parent and child.
    ///
    /// Intercepts GrantCredits, Ping, TerminalResize locally.
    /// Forwards everything else as raw bytes to child.
    async fn relay_loop<R, W>(
        reader: &mut FrameReader<R>,
        child_writer: &mut FrameWriter<tokio::process::ChildStdin>,
        child_lost_rx: &mut tokio::sync::oneshot::Receiver<String>,
        parent_writer: &Arc<Mutex<FrameWriter<W>>>,
        credits: &Arc<Semaphore>,
        viewport_cols: &mut u16,
        viewport_rows: &mut u16,
        kernel: &Arc<Mutex<Kernel>>,
        instance_id: &str,
        cancel: &tokio_util::sync::CancellationToken,
        read_timeout_secs: u64,
    ) -> RelayExit
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin + Send,
    {
        let mut buf = Vec::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!("relay cancelled (UDS takeover)");
                    return RelayExit::ParentDisconnected;
                }
                result = tokio::time::timeout(Duration::from_secs(read_timeout_secs), reader.read_raw(&mut buf)) => {
                    let (req_priority, _flags) = match result {
                        Ok(Ok(pf)) => pf,
                        Ok(Err(nexus_protocol::codec::CodecError::ConnectionClosed)) => {
                            tracing::info!("parent disconnected during relay");
                            return RelayExit::ParentDisconnected;
                        }
                        Ok(Err(e)) => {
                            tracing::error!("relay read error: {e}");
                            return RelayExit::ParentDisconnected;
                        }
                        Err(_) => {
                            tracing::info!("relay read timeout ({read_timeout_secs}s), assuming dead connection");
                            return RelayExit::ParentDisconnected;
                        }
                    };

                    // Decode to decide routing
                    let request: Request = match decode_payload(&buf) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::warn!("relay: failed to decode request: {e}");
                            continue;
                        }
                    };

                    match request {
                        Request::GrantCredits { bytes } => {
                            credits.add_permits(bytes as usize);
                        }
                        Request::Ping { seq } => {
                            let resp = Response::Pong { seq };
                            let mut w = parent_writer.lock().await;
                            let _ = w.write(&resp, priority::CONTROL).await;
                        }
                        Request::TerminalResize { cols, rows } => {
                            // Apply locally (correct dimensions when child disconnects)
                            *viewport_cols = cols;
                            *viewport_rows = rows;
                            {
                                let mut k = kernel.lock().await;
                                k.state_mut().set_env("COLUMNS".to_string(), cols.to_string());
                                k.state_mut().set_env("LINES".to_string(), rows.to_string());
                            }
                            // Forward to child
                            if child_writer.write_raw(&buf, req_priority).await.is_err() {
                                // Child write failed — will be caught by child_lost_rx
                                continue;
                            }
                        }
                        Request::Shutdown => {
                            // Forward to child, then exit
                            let _ = child_writer.write_raw(&buf, req_priority).await;
                            return RelayExit::Shutdown;
                        }
                        _ => {
                            // Forward raw to child (Execute, PtyInput, Unnest, CancelBlock, etc.)
                            if child_writer.write_raw(&buf, req_priority).await.is_err() {
                                continue;
                            }
                        }
                    }
                }

                reason = &mut *child_lost_rx => {
                    let reason = reason.unwrap_or_else(|_| "child_lost channel dropped".to_string());

                    // Send ChildLost with our own identity to parent
                    let surviving_env = Self::collect_env_info_static(instance_id).await;
                    let resp = Response::ChildLost {
                        reason: reason.clone(),
                        surviving_env,
                    };
                    let mut w = parent_writer.lock().await;
                    let _ = w.write(&resp, resp.priority()).await;

                    return RelayExit::ChildLost(reason);
                }
            }
        }
    }

    /// Returns Err only on write failure (connection lost).
    async fn handle_hello<W: AsyncWrite + Unpin + Send>(
        &mut self,
        forwarded_env: HashMap<String, String>,
        writer: &Arc<tokio::sync::Mutex<FrameWriter<W>>>,
    ) -> Result<()> {
        // Store forwarded env for nesting
        self.forwarded_env = forwarded_env.clone();

        // Merge forwarded environment variables into kernel state
        {
            let mut kernel = self.kernel.lock().await;
            for (key, value) in &forwarded_env {
                kernel.state_mut().set_env(key.clone(), value.clone());
            }
        }

        let env = self.collect_env_info().await;
        let session_token: [u8; 16] = rand::random();
        self.session_token = Some(session_token);

        let resp = Response::HelloOk {
            agent_version: env!("CARGO_PKG_VERSION").to_string(),
            env,
            capabilities: nexus_protocol::AgentCaps {
                flow_control: true,
                resume: true,
                nesting: true,
                file_transfer: true,
                pty: true,
            },
            session_token,
        };

        let mut w = writer.lock().await;
        w.write(&resp, resp.priority()).await.map_err(|e| anyhow::anyhow!(e))
    }

    async fn handle_resume<W: AsyncWrite + Unpin + Send>(
        &mut self,
        session_token: [u8; 16],
        last_seen_seq: u64,
        cols: u16,
        rows: u16,
        writer: &Arc<tokio::sync::Mutex<FrameWriter<W>>>,
        ring_buffer: &Arc<tokio::sync::Mutex<RingBuffer>>,
        next_seq: &Arc<AtomicU64>,
    ) -> Result<()> {
        // Validate session token
        if self.session_token != Some(session_token) {
            let resp = Response::Error {
                id: 0,
                message: "Invalid session token".into(),
            };
            let mut w = writer.lock().await;
            w.write(&resp, resp.priority()).await?;
            return Ok(());
        }

        // Resize all active PTYs to the client's current terminal dimensions.
        // The window may have been resized while disconnected — this triggers
        // SIGWINCH so remote processes (vim, top, etc.) adapt immediately.
        if cols > 0 && rows > 0 {
            self.viewport_cols = cols;
            self.viewport_rows = rows;
            for &block_id in &self.pty_manager.active_block_ids() {
                let _ = self.pty_manager.resize(block_id, cols, rows);
            }
        }

        // Replay buffered events since last_seen_seq using raw writes.
        // Detect if the ring buffer has overflowed (events were lost).
        let events_lost;
        {
            let rb = ring_buffer.lock().await;
            let oldest = rb.oldest_seq();
            events_lost = last_seen_seq > 0 && oldest > 0 && last_seen_seq < oldest;
            if events_lost {
                tracing::warn!(
                    "ring buffer overrun: client at seq {last_seen_seq}, oldest buffered is {oldest}"
                );
            }
            let frames = rb.replay_since(last_seen_seq);
            tracing::debug!(
                "resume replay: last_seen_seq={last_seen_seq}, oldest={}, frames={}",
                rb.oldest_seq(),
                frames.len()
            );
            let mut w = writer.lock().await;
            for payload in frames {
                let _ = w
                    .write_raw_flagged(payload, priority::INTERACTIVE, FLAG_EVENT)
                    .await;
            }
        }

        // Send terminal grid snapshots for all active PTY sessions.
        // These arrive AFTER the ring buffer replay, so the UI can use them
        // as the definitive screen state (correcting any gaps from evicted
        // ring buffer frames).
        let active_blocks = self.pty_manager.active_block_ids();
        {
            let mut w = writer.lock().await;
            for &block_id in &active_blocks {
                if let Some(snap) = self.pty_manager.snapshot(block_id).await {
                    // Send viewport snapshot
                    let seq = next_seq.fetch_add(1, Ordering::Relaxed);
                    let snapshot_event = ShellEvent::TerminalSnapshot {
                        block_id,
                        grid: snap.grid,
                        alt_screen: snap.alt_screen,
                        app_cursor: snap.app_cursor,
                        bracketed_paste: snap.bracketed_paste,
                    };
                    let resp = Response::Event {
                        seq,
                        event: snapshot_event,
                    };
                    let _ = w.write(&resp, priority::INTERACTIVE).await;

                    // Send scrollback history if available
                    if !snap.scrollback.is_empty() {
                        let seq = next_seq.fetch_add(1, Ordering::Relaxed);
                        let scrollback_event = ShellEvent::ScrollbackHistory {
                            block_id,
                            cells: snap.scrollback,
                            cols: snap.scrollback_cols,
                        };
                        let resp = Response::Event {
                            seq,
                            event: scrollback_event,
                        };
                        let _ = w.write(&resp, priority::INTERACTIVE).await;
                    }
                }
            }
        }

        // Send session state (with gap warning if ring buffer overflowed)
        let env = self.collect_env_info().await;
        let resp = Response::SessionState {
            token: session_token,
            env,
            active_blocks,
            events_lost,
        };
        let mut w = writer.lock().await;
        w.write(&resp, resp.priority()).await?;
        Ok(())
    }

    async fn handle_execute<W: AsyncWrite + Unpin + Send>(
        &self,
        _id: u32,
        command: String,
        block_id: BlockId,
        _writer: &Arc<tokio::sync::Mutex<FrameWriter<W>>>,
    ) {
        let kernel = self.kernel.clone();

        tokio::task::spawn_blocking(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let handle = tokio::runtime::Handle::current();
                handle.block_on(async {
                    let mut kernel = kernel.lock().await;
                    let _ = kernel.execute_with_block_id(&command, Some(block_id));
                });
            }));

            if let Err(panic_info) = result {
                let error_msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                    format!("Command panicked: {}", s)
                } else if let Some(s) = panic_info.downcast_ref::<String>() {
                    format!("Command panicked: {}", s)
                } else {
                    "Command panicked (unknown error)".to_string()
                };
                tracing::error!("{error_msg}");
            }
        });
    }

    async fn handle_classify<W: AsyncWrite + Unpin + Send>(
        &self,
        id: u32,
        command: &str,
        writer: &Arc<tokio::sync::Mutex<FrameWriter<W>>>,
    ) -> Result<()> {
        let kernel = self.kernel.lock().await;
        let classification = kernel.classify_command(command);
        let wire_class = match classification {
            nexus_kernel::CommandClassification::Kernel => CommandClassification::Kernel,
            nexus_kernel::CommandClassification::Pty => CommandClassification::Pty,
            nexus_kernel::CommandClassification::RemoteTransport => {
                CommandClassification::RemoteTransport
            }
        };

        let resp = Response::ClassifyResult {
            id,
            classification: wire_class,
        };
        let mut w = writer.lock().await;
        w.write(&resp, resp.priority()).await?;
        Ok(())
    }

    async fn handle_complete<W: AsyncWrite + Unpin + Send>(
        &self,
        id: u32,
        input: &str,
        cursor: usize,
        writer: &Arc<tokio::sync::Mutex<FrameWriter<W>>>,
    ) -> Result<()> {
        let kernel = self.kernel.lock().await;
        let (completions, start) = kernel.complete(input, cursor);

        let items: Vec<CompletionItem> = completions
            .into_iter()
            .map(|c| CompletionItem {
                text: c.text,
                display: c.display,
                kind: match c.kind {
                    nexus_kernel::CompletionKind::File => CompletionKind::File,
                    nexus_kernel::CompletionKind::Directory => CompletionKind::Directory,
                    nexus_kernel::CompletionKind::Executable => CompletionKind::Executable,
                    nexus_kernel::CompletionKind::Builtin => CompletionKind::Builtin,
                    nexus_kernel::CompletionKind::NativeCommand => CompletionKind::NativeCommand,
                    nexus_kernel::CompletionKind::Function => CompletionKind::Function,
                    nexus_kernel::CompletionKind::Alias => CompletionKind::Alias,
                    nexus_kernel::CompletionKind::Variable => CompletionKind::Variable,
                    nexus_kernel::CompletionKind::GitBranch => CompletionKind::GitBranch,
                    nexus_kernel::CompletionKind::Flag => CompletionKind::Flag,
                },
                score: c.score,
            })
            .collect();

        let resp = Response::CompleteResult {
            id,
            completions: items,
            start,
        };
        let mut w = writer.lock().await;
        w.write(&resp, resp.priority()).await?;
        Ok(())
    }

    async fn handle_search_history<W: AsyncWrite + Unpin + Send>(
        &self,
        id: u32,
        query: &str,
        limit: u32,
        writer: &Arc<tokio::sync::Mutex<FrameWriter<W>>>,
    ) -> Result<()> {
        let kernel = self.kernel.lock().await;
        let entries = kernel.search_history(query, limit as usize);

        let items: Vec<HistoryEntry> = entries
            .into_iter()
            .map(|e| HistoryEntry {
                command: e.command,
                timestamp: e.timestamp,
            })
            .collect();

        let resp = Response::HistoryResult { id, entries: items };
        let mut w = writer.lock().await;
        w.write(&resp, resp.priority()).await?;
        Ok(())
    }

    /// Spawnable file-read task that checks `cancelled_reads` on each chunk.
    async fn handle_file_read_task<W: AsyncWrite + Unpin + Send + 'static>(
        id: u32,
        path: &str,
        offset: u64,
        len: Option<u64>,
        writer: &Arc<tokio::sync::Mutex<FrameWriter<W>>>,
        cancelled: &Arc<std::sync::Mutex<std::collections::HashSet<u32>>>,
    ) -> Result<()> {
        use tokio::io::AsyncReadExt;

        match tokio::fs::File::open(path).await {
            Ok(mut file) => {
                if offset > 0 {
                    use tokio::io::AsyncSeekExt;
                    file.seek(std::io::SeekFrom::Start(offset)).await?;
                }

                let chunk_size = nexus_protocol::MAX_FRAME_PAYLOAD;
                let mut remaining = len;
                loop {
                    if cancelled.lock().unwrap().contains(&id) {
                        let resp = Response::FileData {
                            id,
                            data: Vec::new(),
                            eof: true,
                        };
                        let mut w = writer.lock().await;
                        let _ = w.write(&resp, nexus_protocol::priority::BULK).await;
                        break;
                    }

                    let read_size = match remaining {
                        Some(r) => chunk_size.min(r as usize),
                        None => chunk_size,
                    };
                    let mut buf = vec![0u8; read_size];
                    let n = file.read(&mut buf).await?;
                    buf.truncate(n);
                    let eof = n == 0 || remaining.map_or(false, |r| r <= n as u64);

                    let resp = Response::FileData {
                        id,
                        data: buf,
                        eof,
                    };
                    let mut w = writer.lock().await;
                    w.write(&resp, nexus_protocol::priority::BULK).await?;

                    if eof {
                        break;
                    }
                    if let Some(ref mut r) = remaining {
                        *r -= n as u64;
                    }
                }
            }
            Err(e) => {
                let resp = Response::Error {
                    id,
                    message: format!("failed to open file: {e}"),
                };
                let mut w = writer.lock().await;
                w.write(&resp, resp.priority()).await?;
            }
        }
        Ok(())
    }

    async fn handle_file_write<W: AsyncWrite + Unpin + Send>(
        &self,
        id: u32,
        path: &str,
        offset: u64,
        data: &[u8],
        writer: &Arc<tokio::sync::Mutex<FrameWriter<W>>>,
    ) -> Result<()> {
        use tokio::io::AsyncWriteExt;

        let result = async {
            let mut file = if offset == 0 {
                tokio::fs::File::create(path).await?
            } else {
                let f = tokio::fs::OpenOptions::new()
                    .write(true)
                    .open(path)
                    .await?;
                use tokio::io::AsyncSeekExt;
                let mut f = f;
                f.seek(std::io::SeekFrom::Start(offset)).await?;
                f
            };
            file.write_all(data).await?;
            file.flush().await?;
            Ok::<_, std::io::Error>(data.len() as u64)
        }
        .await;

        let resp = match result {
            Ok(bytes_written) => Response::FileWriteOk { id, bytes_written },
            Err(e) => Response::Error {
                id,
                message: format!("file write failed: {e}"),
            },
        };

        let mut w = writer.lock().await;
        w.write(&resp, resp.priority()).await?;
        Ok(())
    }

    async fn collect_env_info(&self) -> EnvInfo {
        let mut info = Self::collect_env_info_static(&self.instance_id).await;
        // Use the kernel's tracked CWD (updated by `cd`) instead of the
        // agent process's CWD (which never changes after launch).
        let kernel = self.kernel.lock().await;
        info.cwd = kernel.state().cwd.clone();
        info
    }

    /// Collect environment info without needing `&self` (for use in relay loop).
    async fn collect_env_info_static(instance_id: &str) -> EnvInfo {
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "unknown".into());

        let hostname = gethostname::gethostname()
            .to_string_lossy()
            .into_owned();

        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));

        let os = std::env::consts::OS.to_string();
        let arch = std::env::consts::ARCH.to_string();

        EnvInfo {
            instance_id: instance_id.to_string(),
            user,
            hostname,
            cwd,
            os,
            arch,
        }
    }
}
