//! Thin test client that drives the nexus protocol directly.
//!
//! Wraps `TransportHandle` and provides a simple API for integration tests
//! to connect, execute commands, spawn PTYs, and resume sessions.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use nexus_api::ShellEvent;
use nexus_protocol::messages::{EnvInfo, Request, Response, Transport};
use tokio::sync::{broadcast, mpsc};

use crate::container::TestEnv;

/// A connected test client.
pub struct TestClient {
    pub env: EnvInfo,
    pub session_token: [u8; 16],
    pub last_seen_seq: Arc<AtomicU64>,
    pub rtt_ms: Arc<AtomicU64>,
    pub last_pong_at: Arc<AtomicU64>,

    request_tx: mpsc::UnboundedSender<RequestEnvelope>,
    response_rx: mpsc::UnboundedReceiver<Response>,
    event_rx: broadcast::Receiver<ShellEvent>,
    kernel_tx: broadcast::Sender<ShellEvent>,

    /// SSH child process (kill this to simulate client disconnect).
    child: Option<tokio::process::Child>,

    /// For resume
    transport: Transport,
    agent_path: String,

    next_request_id: u32,
}

/// Mirrors the agent's RequestEnvelope.
struct RequestEnvelope {
    request: Request,
}

impl TestClient {
    /// Connect to the agent via SSH (fresh Hello handshake).
    pub async fn connect(env: &TestEnv) -> Result<Self> {
        let transport = env.transport();
        let agent_path = env.agent_path().to_string();
        let (kernel_tx, _) = broadcast::channel::<ShellEvent>(256);

        // Use the transport module directly
        let (handle, env_info, session_token, request_tx) =
            connect_transport(&transport, &agent_path, kernel_tx.clone()).await?;

        let event_rx = kernel_tx.subscribe();

        Ok(Self {
            env: env_info,
            session_token,
            last_seen_seq: handle.last_seen_seq,
            rtt_ms: handle.rtt_ms,
            last_pong_at: handle.last_pong_at,
            request_tx: wrap_request_tx(request_tx),
            response_rx: handle.response_rx,
            event_rx,
            kernel_tx,
            child: Some(handle.child),
            transport,
            agent_path,
            next_request_id: 1,
        })
    }

    /// Resume an existing session (Resume handshake via --attach).
    pub async fn resume(
        env: &TestEnv,
        instance_id: &str,
        session_token: [u8; 16],
        last_seen_seq: u64,
    ) -> Result<Self> {
        let transport = env.transport();
        let agent_path = env.agent_path().to_string();
        let (kernel_tx, _) = broadcast::channel::<ShellEvent>(256);
        // Subscribe before resume so we capture replay events sent during handshake
        let event_rx = kernel_tx.subscribe();

        let (handle, env_info, request_tx) = resume_transport(
            &transport,
            &agent_path,
            instance_id,
            session_token,
            last_seen_seq,
            80,
            24,
            kernel_tx.clone(),
        )
        .await?;

        Ok(Self {
            env: env_info,
            session_token,
            last_seen_seq: handle.last_seen_seq,
            rtt_ms: handle.rtt_ms,
            last_pong_at: handle.last_pong_at,
            request_tx: wrap_request_tx(request_tx),
            response_rx: handle.response_rx,
            event_rx,
            kernel_tx,
            child: Some(handle.child),
            transport,
            agent_path,
            next_request_id: 1,
        })
    }

    /// Kill the SSH child process (simulates client-side disconnect).
    pub fn kill_ssh(&mut self) {
        if let Some(ref mut child) = self.child {
            let _ = child.start_kill();
        }
        self.child = None;
    }

    /// Get the agent's instance ID.
    pub fn instance_id(&self) -> &str {
        &self.env.instance_id
    }

    /// Get the last seen sequence number.
    pub fn last_seen_seq_value(&self) -> u64 {
        self.last_seen_seq.load(Ordering::Relaxed)
    }

    /// Check if pong data is flowing (mirrors RemoteBackend::is_data_flowing).
    pub fn is_data_flowing(&self) -> bool {
        let last = self.last_pong_at.load(Ordering::Relaxed);
        if last == 0 {
            return true;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now_ms.saturating_sub(last) < 10_000
    }

    fn next_id(&mut self) -> u32 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    /// Send a request to the agent.
    pub fn send(&mut self, request: Request) {
        let _ = self.request_tx.send(RequestEnvelope { request });
    }

    /// Execute a command and return the block ID.
    pub fn execute(&mut self, command: &str, block_id: nexus_api::BlockId) {
        let id = self.next_id();
        self.send(Request::Execute {
            id,
            command: command.to_string(),
            block_id,
        });
    }

    /// Spawn a PTY.
    pub fn spawn_pty(
        &mut self,
        command: &str,
        block_id: nexus_api::BlockId,
        cols: u16,
        rows: u16,
    ) {
        let id = self.next_id();
        self.send(Request::PtySpawn {
            id,
            command: command.to_string(),
            block_id,
            cols,
            rows,
            term: "xterm-256color".to_string(),
            cwd: "/home/testuser".to_string(),
        });
    }

    /// Wait for a specific event, with timeout.
    pub async fn wait_for_event<F>(
        &mut self,
        timeout: Duration,
        pred: F,
    ) -> Option<ShellEvent>
    where
        F: Fn(&ShellEvent) -> bool,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }

            match tokio::time::timeout(remaining, self.event_rx.recv()).await {
                Ok(Ok(event)) if pred(&event) => return Some(event),
                Ok(Ok(_)) => continue, // not the event we want
                Ok(Err(_)) => return None, // channel closed
                Err(_) => return None, // timeout
            }
        }
    }

    /// Collect all stdout output for a block until CommandFinished or timeout.
    pub async fn collect_output(
        &mut self,
        block_id: nexus_api::BlockId,
        timeout: Duration,
    ) -> (String, Option<i32>) {
        let mut output = Vec::new();
        let mut exit_code = None;
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            match tokio::time::timeout(remaining, self.event_rx.recv()).await {
                Ok(Ok(ShellEvent::StdoutChunk {
                    block_id: bid,
                    data,
                    ..
                })) if bid == block_id => {
                    output.extend_from_slice(&data);
                }
                Ok(Ok(ShellEvent::StderrChunk {
                    block_id: bid,
                    data,
                    ..
                })) if bid == block_id => {
                    output.extend_from_slice(&data);
                }
                Ok(Ok(ShellEvent::CommandOutput {
                    block_id: bid,
                    value,
                })) if bid == block_id => {
                    // Builtin commands emit structured Value, not raw bytes
                    output.extend_from_slice(format!("{value}").as_bytes());
                }
                Ok(Ok(ShellEvent::CommandFinished {
                    block_id: bid,
                    exit_code: code,
                    ..
                })) if bid == block_id => {
                    exit_code = Some(code);
                    break;
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(_) => break,
            }
        }

        (String::from_utf8_lossy(&output).to_string(), exit_code)
    }

    /// Wait for the next non-event response (ClassifyResult, Error, etc).
    pub async fn recv_response(&mut self, timeout: Duration) -> Option<Response> {
        tokio::time::timeout(timeout, self.response_rx.recv())
            .await
            .ok()
            .flatten()
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        self.kill_ssh();
    }
}

// ---------------------------------------------------------------------------
// Transport wrappers — thin layer over the real transport code
// ---------------------------------------------------------------------------

/// Minimal handle matching TransportHandle's public fields.
pub struct TransportHandleFields {
    pub child: tokio::process::Child,
    pub rtt_ms: Arc<AtomicU64>,
    pub last_pong_at: Arc<AtomicU64>,
    pub last_seen_seq: Arc<AtomicU64>,
    pub response_rx: mpsc::UnboundedReceiver<Response>,
}

/// Connect via the real transport, returning the raw handle fields.
async fn connect_transport(
    transport: &Transport,
    agent_path: &str,
    kernel_tx: broadcast::Sender<ShellEvent>,
) -> Result<(
    TransportHandleFields,
    EnvInfo,
    [u8; 16],
    mpsc::UnboundedSender<nexus_protocol::messages::Request>,
)> {
    use nexus_protocol::codec::FrameCodec;
    use nexus_protocol::messages::Request;
    use nexus_protocol::{ClientCaps, PROTOCOL_VERSION};

    let mut child = spawn_ssh_child(transport, agent_path, &[])?;

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let codec = FrameCodec::new(stdout, stdin);
    let (mut reader, mut writer) = codec.into_parts();

    // Hello handshake
    let hello = Request::Hello {
        protocol_version: PROTOCOL_VERSION,
        capabilities: ClientCaps {
            flow_control: true,
            resume: true,
            nesting: true,
            file_transfer: true,
        },
        forwarded_env: HashMap::new(),
    };
    writer.write(&hello, hello.priority()).await?;

    let response: Response = reader.read().await?;
    let (env, session_token, _caps) = match response {
        Response::HelloOk {
            env,
            session_token,
            capabilities,
            ..
        } => (env, session_token, capabilities),
        Response::Error { message, .. } => anyhow::bail!("Hello rejected: {message}"),
        other => anyhow::bail!("unexpected response to Hello: {other:?}"),
    };

    let (rtt_ms, last_pong_at, last_seen_seq, request_tx, response_rx) =
        setup_bridge(reader, writer, kernel_tx);

    Ok((
        TransportHandleFields {
            child,
            rtt_ms,
            last_pong_at,
            last_seen_seq,
            response_rx,
        },
        env,
        session_token,
        request_tx,
    ))
}

/// Resume via --attach + Resume handshake.
async fn resume_transport(
    transport: &Transport,
    agent_path: &str,
    instance_id: &str,
    session_token: [u8; 16],
    last_seen_seq: u64,
    cols: u16,
    rows: u16,
    kernel_tx: broadcast::Sender<ShellEvent>,
) -> Result<(
    TransportHandleFields,
    EnvInfo,
    mpsc::UnboundedSender<nexus_protocol::messages::Request>,
)> {
    use nexus_protocol::codec::FrameCodec;
    use nexus_protocol::messages::Request;

    let extra_args = ["--attach", instance_id];
    let mut child = spawn_ssh_child(transport, agent_path, &extra_args)?;

    let stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let codec = FrameCodec::new(stdout, stdin);
    let (mut reader, mut writer) = codec.into_parts();

    // Resume handshake
    let resume = Request::Resume {
        session_token,
        last_seen_seq,
        cols,
        rows,
    };
    writer.write(&resume, resume.priority()).await?;

    // The agent may send replay events (TerminalSnapshot, ring buffer) before
    // SessionState. Read until we find SessionState, forwarding events.
    let env = loop {
        let response: Response = reader.read().await?;
        match response {
            Response::SessionState { env, .. } => break env,
            Response::Event { event, .. } => {
                let _ = kernel_tx.send(event);
            }
            Response::Error { message, .. } => anyhow::bail!("Resume rejected: {message}"),
            _ => {} // skip other frames (e.g. Pong) during handshake
        }
    };

    let (rtt_ms, last_pong_at, last_seen_seq_arc, request_tx, response_rx) =
        setup_bridge(reader, writer, kernel_tx);

    Ok((
        TransportHandleFields {
            child,
            rtt_ms,
            last_pong_at,
            last_seen_seq: last_seen_seq_arc,
            response_rx,
        },
        env,
        request_tx,
    ))
}

/// Spawn the SSH child process.
fn spawn_ssh_child(
    transport: &Transport,
    agent_path: &str,
    extra_agent_args: &[&str],
) -> Result<tokio::process::Child> {
    let Transport::Ssh {
        destination,
        port,
        identity,
        extra_args,
    } = transport
    else {
        anyhow::bail!("only SSH transport supported in test harness");
    };

    let mut cmd = tokio::process::Command::new("ssh");
    cmd.arg("-o").arg("BatchMode=yes");
    cmd.arg("-o").arg("ConnectTimeout=10");
    if let Some(port) = port {
        cmd.arg("-p").arg(port.to_string());
    }
    if let Some(identity) = identity {
        cmd.arg("-i").arg(identity);
    }
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.arg(destination);

    // Route agent stderr to a log file for debugging
    let remote_cmd = if extra_agent_args.is_empty() {
        format!(
            "RUST_LOG=info {} 2>>/tmp/nexus-agent.log",
            agent_path
        )
    } else {
        format!(
            "RUST_LOG=info {} {} 2>>/tmp/nexus-agent.log",
            agent_path,
            extra_agent_args.join(" ")
        )
    };
    cmd.arg(remote_cmd);

    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    Ok(cmd.spawn()?)
}

/// Spawn event bridge, request sender, and ping loop.
/// Mirrors TransportHandle::setup_bridge but returns raw parts.
fn setup_bridge<R, W>(
    reader: nexus_protocol::codec::FrameReader<R>,
    writer: nexus_protocol::codec::FrameWriter<W>,
    kernel_tx: broadcast::Sender<ShellEvent>,
) -> (
    Arc<AtomicU64>,
    Arc<AtomicU64>,
    Arc<AtomicU64>,
    mpsc::UnboundedSender<Request>,
    mpsc::UnboundedReceiver<Response>,
)
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    use nexus_protocol::codec::encode_payload;
    use std::collections::HashMap;
    use std::time::Instant;
    use tokio::sync::Mutex;

    let rtt_ms = Arc::new(AtomicU64::new(0));
    let last_pong_at = Arc::new(AtomicU64::new(0));
    let last_seen_seq = Arc::new(AtomicU64::new(0));
    let ping_timestamps: Arc<Mutex<HashMap<u64, Instant>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // Request sender
    let (request_tx, mut request_rx) = mpsc::unbounded_channel::<Request>();
    let mut writer = writer;
    tokio::spawn(async move {
        while let Some(req) = request_rx.recv().await {
            let priority = req.priority();
            if writer.write(&req, priority).await.is_err() {
                break;
            }
        }
    });

    // Response reader (event bridge)
    let (response_tx, response_rx) = mpsc::unbounded_channel::<Response>();
    let bridge_rtt = rtt_ms.clone();
    let bridge_pong = last_pong_at.clone();
    let bridge_seq = last_seen_seq.clone();
    let bridge_timestamps = ping_timestamps.clone();
    let bridge_request_tx = request_tx.clone();

    tokio::spawn(async move {
        let mut bytes_since_grant: u64 = 0;
        let mut reader = reader;

        loop {
            let response: Response = match reader.read().await {
                Ok(resp) => resp,
                Err(_) => break,
            };

            let frame_size = encode_payload(&response)
                .map(|v| v.len() as u64)
                .unwrap_or(256);
            bytes_since_grant += frame_size;

            if bytes_since_grant >= 128 * 1024 {
                bytes_since_grant = 0;
                let _ = bridge_request_tx.send(Request::GrantCredits {
                    bytes: 256 * 1024,
                });
            }

            match response {
                Response::Event { seq, event } => {
                    let prev = bridge_seq.load(Ordering::Relaxed);
                    if prev > 0 && seq <= prev {
                        continue;
                    }
                    bridge_seq.store(seq, Ordering::Relaxed);
                    let _ = kernel_tx.send(event);
                }
                Response::Pong { seq } => {
                    let mut timestamps = bridge_timestamps.lock().await;
                    if let Some(sent_at) = timestamps.remove(&seq) {
                        let elapsed = sent_at.elapsed().as_millis() as u64;
                        bridge_rtt.store(elapsed, Ordering::Relaxed);
                    }
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;
                    bridge_pong.store(now_ms, Ordering::Relaxed);
                }
                other => {
                    if response_tx.send(other).is_err() {
                        break;
                    }
                }
            }
        }
    });

    // Initial flow control credits
    let _ = request_tx.send(Request::GrantCredits {
        bytes: 256 * 1024,
    });

    // Ping loop
    let ping_tx = request_tx.clone();
    let ping_timestamps_clone = ping_timestamps;
    tokio::spawn(async move {
        let mut seq = 0u64;
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            seq += 1;
            ping_timestamps_clone
                .lock()
                .await
                .insert(seq, Instant::now());
            if ping_tx.send(Request::Ping { seq }).is_err() {
                break;
            }
        }
    });

    (rtt_ms, last_pong_at, last_seen_seq, request_tx, response_rx)
}

/// Wrap the raw request sender to match our internal envelope type.
fn wrap_request_tx(
    raw_tx: mpsc::UnboundedSender<Request>,
) -> mpsc::UnboundedSender<RequestEnvelope> {
    let (tx, mut rx) = mpsc::unbounded_channel::<RequestEnvelope>();
    tokio::spawn(async move {
        while let Some(envelope) = rx.recv().await {
            if raw_tx.send(envelope.request).is_err() {
                break;
            }
        }
    });
    tx
}
