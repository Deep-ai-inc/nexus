//! Core agent loop: owns a Kernel, reads Requests, writes Responses.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use nexus_api::{BlockId, ShellEvent};
use nexus_kernel::Kernel;
use nexus_protocol::codec::{decode_payload, FrameCodec, FrameWriter};
use nexus_protocol::messages::*;
use nexus_protocol::priority;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{broadcast, Mutex, Semaphore};

use crate::pty::PtyManager;
use crate::session::RingBuffer;

/// The remote agent. Wraps a `Kernel` and speaks the Nexus wire protocol.
pub struct Agent {
    kernel: Arc<Mutex<Kernel>>,
    kernel_rx: broadcast::Receiver<ShellEvent>,
    idle_timeout_secs: u64,
    /// Monotonically increasing sequence number for outbound events.
    next_seq: u64,
    /// Ring buffer for session resume.
    ring_buffer: RingBuffer,
    /// Next request ID tracker (for generating unique IDs internally).
    next_id: u32,
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
}

impl Agent {
    pub fn new(idle_timeout_secs: u64) -> Result<Self> {
        let (kernel, kernel_rx) = Kernel::new()?;

        Ok(Self {
            kernel: Arc::new(Mutex::new(kernel)),
            kernel_rx,
            idle_timeout_secs,
            next_seq: 0,
            ring_buffer: RingBuffer::new(1024 * 1024), // 1 MB default
            next_id: 0,
            viewport_cols: 80,
            viewport_rows: 24,
            pty_manager: PtyManager::new(),
            session_token: None,
            credits: Arc::new(Semaphore::new(0)),
        })
    }

    /// Main loop: read requests from `input`, write responses to `output`.
    pub async fn run<R, W>(&mut self, input: R, output: W) -> Result<()>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let codec = FrameCodec::new(input, output);
        let (mut reader, writer) = codec.into_parts();
        let writer = Arc::new(tokio::sync::Mutex::new(writer));

        // Spawn kernel event forwarder
        let event_writer = writer.clone();
        let kernel_tx = self.kernel.lock().await.event_sender().clone();
        let mut event_rx = kernel_tx.subscribe();
        let ring_buffer = Arc::new(tokio::sync::Mutex::new(std::mem::replace(
            &mut self.ring_buffer,
            RingBuffer::new(0),
        )));

        let next_seq = Arc::new(std::sync::atomic::AtomicU64::new(self.next_seq));
        let next_seq_clone = next_seq.clone();
        let ring_clone = ring_buffer.clone();
        let credits = self.credits.clone();

        let event_task = tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        let seq =
                            next_seq_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let resp = Response::Event {
                            seq,
                            event: event.clone(),
                        };

                        // Buffer for resume
                        {
                            let mut rb = ring_clone.lock().await;
                            rb.push(&resp);
                        }

                        // Acquire flow control credits for data payloads
                        let frame_size = nexus_protocol::codec::encode_payload(&resp)
                            .map(|v| v.len())
                            .unwrap_or(256);
                        // Best-effort: try to acquire credits, but don't block
                        // indefinitely if the client is gone. The writer lock
                        // error will break us out.
                        let _ = credits.acquire_many(frame_size as u32).await;

                        // Send to client
                        let mut w = event_writer.lock().await;
                        if w.write(&resp, resp.priority()).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("event forwarder lagged by {n} events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });

        // Main request loop
        loop {
            let request: Request = match reader.read().await {
                Ok(req) => req,
                Err(nexus_protocol::codec::CodecError::ConnectionClosed) => {
                    tracing::info!("client disconnected");
                    break;
                }
                Err(e) => {
                    tracing::error!("read error: {e}");
                    break;
                }
            };

            match request {
                Request::Hello {
                    protocol_version: _,
                    capabilities: _,
                    forwarded_env,
                } => {
                    self.handle_hello(forwarded_env, &writer).await?;
                }

                Request::Execute {
                    id,
                    command,
                    block_id,
                } => {
                    self.handle_execute(id, command, block_id, &writer).await;
                }

                Request::Classify { id, command } => {
                    self.handle_classify(id, &command, &writer).await?;
                }

                Request::Complete { id, input, cursor } => {
                    self.handle_complete(id, &input, cursor, &writer).await?;
                }

                Request::SearchHistory { id, query, limit } => {
                    self.handle_search_history(id, &query, limit, &writer)
                        .await?;
                }

                Request::CancelBlock { id: _, block_id } => {
                    // Cancel via kernel's cancel mechanism
                    nexus_kernel::commands::cancel_block(block_id);
                    // Also kill PTY process group if it's a PTY session
                    let _ = self.pty_manager.kill(block_id, libc::SIGINT);
                }

                Request::PtySpawn {
                    id,
                    command,
                    block_id,
                    cols,
                    rows,
                    term,
                } => {
                    if let Err(e) = self
                        .pty_manager
                        .spawn(&command, block_id, cols, rows, &term, &kernel_tx)
                        .await
                    {
                        let resp = Response::Error {
                            id,
                            message: format!("PTY spawn failed: {e}"),
                        };
                        let mut w = writer.lock().await;
                        w.write(&resp, resp.priority()).await?;
                    }
                }

                Request::PtyInput { block_id, data } => {
                    if let Err(e) = self.pty_manager.input(block_id, &data).await {
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
                    // Update COLUMNS/LINES env vars so native commands can query them
                    let mut kernel = self.kernel.lock().await;
                    kernel
                        .state_mut()
                        .set_env("COLUMNS".to_string(), cols.to_string());
                    kernel
                        .state_mut()
                        .set_env("LINES".to_string(), rows.to_string());
                }

                Request::FileRead {
                    id,
                    path,
                    offset,
                    len,
                } => {
                    self.handle_file_read(id, &path, offset, len, &writer)
                        .await?;
                }

                Request::FileWrite {
                    id,
                    path,
                    offset,
                    data,
                } => {
                    self.handle_file_write(id, &path, offset, &data, &writer)
                        .await?;
                }

                Request::Nest {
                    id,
                    transport: _,
                    force_redeploy: _,
                } => {
                    // TODO: Implement nesting
                    let resp = Response::Error {
                        id,
                        message: "Nesting not yet implemented".into(),
                    };
                    let mut w = writer.lock().await;
                    w.write(&resp, resp.priority()).await?;
                }

                Request::Unnest { id } => {
                    // TODO: Implement unnesting
                    let resp = Response::Error {
                        id,
                        message: "Not in nested mode".into(),
                    };
                    let mut w = writer.lock().await;
                    w.write(&resp, resp.priority()).await?;
                }

                Request::GrantCredits { bytes } => {
                    self.credits.add_permits(bytes as usize);
                }

                Request::Ping { seq } => {
                    let resp = Response::Pong { seq };
                    let mut w = writer.lock().await;
                    w.write(&resp, priority::CONTROL).await?;
                }

                Request::Resume {
                    session_token,
                    last_seen_seq,
                } => {
                    self.handle_resume(session_token, last_seen_seq, &writer, &ring_buffer)
                        .await?;
                }

                Request::Shutdown => {
                    tracing::info!("received shutdown request");
                    self.pty_manager.shutdown_all();
                    break;
                }
            }
        }

        event_task.abort();
        self.pty_manager.shutdown_all();
        Ok(())
    }

    async fn handle_hello<W: AsyncWrite + Unpin + Send>(
        &mut self,
        forwarded_env: HashMap<String, String>,
        writer: &Arc<tokio::sync::Mutex<FrameWriter<W>>>,
    ) -> Result<()> {
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
                nesting: false,
                file_transfer: true,
                pty: true,
            },
            session_token,
        };

        let mut w = writer.lock().await;
        w.write(&resp, resp.priority()).await?;
        Ok(())
    }

    async fn handle_resume<W: AsyncWrite + Unpin + Send>(
        &mut self,
        session_token: [u8; 16],
        last_seen_seq: u64,
        writer: &Arc<tokio::sync::Mutex<FrameWriter<W>>>,
        ring_buffer: &Arc<tokio::sync::Mutex<RingBuffer>>,
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

        // Replay buffered events since last_seen_seq
        {
            let rb = ring_buffer.lock().await;
            let frames = rb.replay_since(last_seen_seq);
            let mut w = writer.lock().await;
            for payload in frames {
                if let Ok(resp) = decode_payload::<Response>(payload) {
                    let _ = w.write(&resp, resp.priority()).await;
                }
            }
        }

        // Reset flow control credits (stale credits from pre-disconnect)
        // The client will send a fresh GrantCredits after resume.
        self.credits = Arc::new(Semaphore::new(0));

        // Send session state
        let env = self.collect_env_info().await;
        let active_blocks = self.pty_manager.active_block_ids();
        let resp = Response::SessionState {
            token: session_token,
            env,
            active_blocks,
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

        // Execute on a blocking thread (same pattern as the UI)
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
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
                // The kernel's event_tx handles emitting CommandFinished on error
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

    async fn handle_file_read<W: AsyncWrite + Unpin + Send>(
        &self,
        id: u32,
        path: &str,
        offset: u64,
        len: Option<u64>,
        writer: &Arc<tokio::sync::Mutex<FrameWriter<W>>>,
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
            user,
            hostname,
            cwd,
            os,
            arch,
        }
    }
}
