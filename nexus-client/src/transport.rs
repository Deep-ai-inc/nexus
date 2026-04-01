//! Transport management: spawning child processes (SSH/Docker/kubectl),
//! performing Hello/Resume handshakes, and setting up the event bridge.

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use nexus_protocol::codec::{FrameCodec, FrameReader, FrameWriter};
use nexus_protocol::messages::{EnvInfo, Request, Response, Transport};
use nexus_protocol::{AgentCaps, ClientCaps, PROTOCOL_VERSION};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, Mutex};

use nexus_api::ShellEvent;

use crate::RequestSender;

/// Handle to the remote transport (child process + codec).
pub struct TransportHandle {
    /// The child process (SSH/Docker/kubectl).
    pub child: Child,
    /// Current RTT in milliseconds (0 = not yet measured).
    pub rtt_ms: Arc<AtomicU64>,
    /// Timestamp (epoch ms) of last Pong received — for staleness detection.
    pub last_pong_at: Arc<AtomicU64>,
    /// Last seen event sequence number from the agent.
    pub last_seen_seq: Arc<AtomicU64>,
    /// Highest echo epoch confirmed by the agent (from StdoutChunk).
    pub last_confirmed_epoch: Arc<AtomicU64>,
    /// Receiver for non-event responses (ClassifyResult, CompleteResult, etc.)
    pub response_rx: mpsc::UnboundedReceiver<Response>,
}

impl TransportHandle {
    /// Connect to a remote agent via the given transport.
    pub async fn connect(
        transport: &Transport,
        agent_path: &str,
        forwarded_env: HashMap<String, String>,
        kernel_tx: broadcast::Sender<ShellEvent>,
    ) -> Result<(Self, EnvInfo, [u8; 16], RequestSender)> {
        let mut child = Self::spawn_child(transport, agent_path)?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("failed to take child stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("failed to take child stdout"))?;

        let codec = FrameCodec::new(stdout, stdin);
        let (reader, writer) = codec.into_parts();

        let (env, session_token, _caps, raw_tx, rtt_ms, last_pong_at, last_seen_seq, last_confirmed_epoch, response_rx) =
            Self::handshake(reader, writer, forwarded_env, kernel_tx).await?;

        Ok((
            Self {
                child,
                rtt_ms,
                last_pong_at,
                last_seen_seq,
                last_confirmed_epoch,
                response_rx,
            },
            env,
            session_token,
            RequestSender::new(raw_tx),
        ))
    }

    /// Resume an existing session with a remote agent.
    ///
    /// If `instance_id` is provided, spawns the agent in `--attach` mode which
    /// connects to the persisting agent's UDS socket.
    ///
    /// Sends `Request::Resume` and reads responses until `Response::SessionState`.
    /// Any replay events (ring buffer, terminal snapshots) received before
    /// SessionState are forwarded to `kernel_tx`.
    pub async fn resume(
        transport: &Transport,
        agent_path: &str,
        instance_id: Option<&str>,
        session_token: [u8; 16],
        last_seen_seq: u64,
        cols: u16,
        rows: u16,
        kernel_tx: broadcast::Sender<ShellEvent>,
    ) -> Result<(Self, EnvInfo, RequestSender)> {
        let mut child = Self::spawn_child_with_args(
            transport,
            agent_path,
            instance_id
                .map(|id| vec!["--attach".to_string(), id.to_string()])
                .unwrap_or_default(),
        )?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("failed to take child stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("failed to take child stdout"))?;

        let codec = FrameCodec::new(stdout, stdin);
        let (reader, writer) = codec.into_parts();

        let (env, raw_tx, rtt_ms, last_pong_at, last_seen_seq_arc, last_confirmed_epoch, response_rx) =
            Self::resume_handshake(
                reader,
                writer,
                session_token,
                last_seen_seq,
                cols,
                rows,
                kernel_tx,
            )
            .await?;

        Ok((
            Self {
                child,
                rtt_ms,
                last_pong_at,
                last_seen_seq: last_seen_seq_arc,
                last_confirmed_epoch,
                response_rx,
            },
            env,
            RequestSender::new(raw_tx),
        ))
    }

    /// Spawn the transport child process (SSH/Docker/kubectl/Command).
    fn spawn_child(transport: &Transport, agent_path: &str) -> Result<Child> {
        Self::spawn_child_with_args(transport, agent_path, Vec::new())
    }

    /// Spawn the transport child process with extra arguments after the agent path.
    fn spawn_child_with_args(
        transport: &Transport,
        agent_path: &str,
        extra_agent_args: Vec<String>,
    ) -> Result<Child> {
        let child = match transport {
            Transport::Ssh {
                destination,
                port,
                identity,
                extra_args,
            } => {
                let mut cmd = Command::new("ssh");
                cmd.arg("-o").arg("BatchMode=yes");
                cmd.arg("-o").arg("StrictHostKeyChecking=accept-new");
                cmd.arg("-o").arg("ConnectTimeout=10");
                cmd.arg("-o").arg("ServerAliveInterval=15");
                cmd.arg("-o").arg("ServerAliveCountMax=3");
                if let Some(port) = port {
                    cmd.arg("-p").arg(port.to_string());
                }
                if let Some(identity) = identity {
                    cmd.arg("-i").arg(identity.as_str());
                }
                for arg in extra_args {
                    cmd.arg(arg);
                }
                cmd.arg(destination);
                cmd.arg(agent_path);
                for arg in &extra_agent_args {
                    cmd.arg(arg);
                }
                cmd.stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                cmd.spawn()?
            }
            Transport::Docker { container, user } => {
                let mut cmd = Command::new("docker");
                cmd.arg("exec").arg("-i");
                if let Some(user) = user {
                    cmd.arg("-u").arg(user);
                }
                cmd.arg(container);
                cmd.arg(agent_path);
                for arg in &extra_agent_args {
                    cmd.arg(arg);
                }
                cmd.stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                cmd.spawn()?
            }
            Transport::Kubectl {
                pod,
                namespace,
                container,
            } => {
                let mut cmd = Command::new("kubectl");
                cmd.arg("exec").arg("-i");
                if let Some(ns) = namespace {
                    cmd.arg("-n").arg(ns);
                }
                if let Some(ctr) = container {
                    cmd.arg("-c").arg(ctr);
                }
                cmd.arg(pod).arg("--").arg(agent_path);
                for arg in &extra_agent_args {
                    cmd.arg(arg);
                }
                cmd.stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                cmd.spawn()?
            }
            Transport::Command { argv } => {
                if argv.is_empty() {
                    anyhow::bail!("empty command argv for transport");
                }
                let mut cmd = Command::new(&argv[0]);
                for arg in &argv[1..] {
                    cmd.arg(arg);
                }
                for arg in &extra_agent_args {
                    cmd.arg(arg);
                }
                cmd.stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                cmd.spawn()?
            }
        };
        Ok(child)
    }

    /// Perform Resume handshake and spawn event bridge tasks.
    ///
    /// The agent may send replay events (ring buffer, terminal snapshots)
    /// before SessionState. These are forwarded to `kernel_tx` as they arrive.
    async fn resume_handshake<R, W>(
        mut reader: FrameReader<R>,
        mut writer: FrameWriter<W>,
        session_token: [u8; 16],
        last_seen_seq: u64,
        cols: u16,
        rows: u16,
        kernel_tx: broadcast::Sender<ShellEvent>,
    ) -> Result<(
        EnvInfo,
        mpsc::UnboundedSender<Request>,
        Arc<AtomicU64>,
        Arc<AtomicU64>,
        Arc<AtomicU64>,
        Arc<AtomicU64>,
        mpsc::UnboundedReceiver<Response>,
    )>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let resume = Request::Resume {
            session_token,
            last_seen_seq,
            cols,
            rows,
        };
        writer
            .write(&resume, resume.priority())
            .await
            .map_err(|e| anyhow::anyhow!("failed to send Resume: {e}"))?;

        // Read until SessionState, forwarding any replay events to kernel_tx.
        // The agent sends: ring buffer replay → terminal snapshots → SessionState.
        let (env, events_lost) = loop {
            let response: Response = reader
                .read()
                .await
                .map_err(|e| anyhow::anyhow!("failed to read during resume: {e}"))?;

            match response {
                Response::SessionState {
                    env, events_lost, ..
                } => break (env, events_lost),
                Response::Event { event, .. } => {
                    let _ = kernel_tx.send(event);
                }
                Response::Error { message, .. } => {
                    anyhow::bail!("resume rejected: {message}");
                }
                _ => {} // skip Pong etc during handshake
            }
        };

        if events_lost {
            tracing::warn!(
                "resume: ring buffer overflowed, some output was lost during disconnection"
            );
        }

        let (request_tx, rtt_ms, last_pong_at, last_seen_seq_arc, last_confirmed_epoch, response_rx) =
            Self::setup_bridge(reader, writer, kernel_tx);

        Ok((
            env,
            request_tx,
            rtt_ms,
            last_pong_at,
            last_seen_seq_arc,
            last_confirmed_epoch,
            response_rx,
        ))
    }

    /// Perform Hello/HelloOk handshake and spawn event bridge tasks.
    async fn handshake<R, W>(
        mut reader: FrameReader<R>,
        mut writer: FrameWriter<W>,
        forwarded_env: HashMap<String, String>,
        kernel_tx: broadcast::Sender<ShellEvent>,
    ) -> Result<(
        EnvInfo,
        [u8; 16],
        AgentCaps,
        mpsc::UnboundedSender<Request>,
        Arc<AtomicU64>,
        Arc<AtomicU64>,
        Arc<AtomicU64>,
        Arc<AtomicU64>,
        mpsc::UnboundedReceiver<Response>,
    )>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let hello = Request::Hello {
            protocol_version: PROTOCOL_VERSION,
            capabilities: ClientCaps {
                flow_control: true,
                resume: true,
                nesting: true,
                file_transfer: true,
            },
            forwarded_env,
        };
        writer
            .write(&hello, hello.priority())
            .await
            .map_err(|e| anyhow::anyhow!("failed to send Hello: {e}"))?;

        let response: Response = reader
            .read()
            .await
            .map_err(|e| anyhow::anyhow!("failed to read HelloOk: {e}"))?;

        let (env, session_token, caps) = match response {
            Response::HelloOk {
                env,
                session_token,
                capabilities,
                ..
            } => (env, session_token, capabilities),
            Response::Error { message, .. } => {
                anyhow::bail!("agent rejected Hello: {message}");
            }
            other => {
                anyhow::bail!("unexpected response to Hello: {other:?}");
            }
        };

        let (request_tx, rtt_ms, last_pong_at, last_seen_seq, last_confirmed_epoch, response_rx) =
            Self::setup_bridge(reader, writer, kernel_tx);

        Ok((
            env,
            session_token,
            caps,
            request_tx,
            rtt_ms,
            last_pong_at,
            last_seen_seq,
            last_confirmed_epoch,
            response_rx,
        ))
    }

    /// Spawn the event bridge, request sender, ping loop, and initial credit grant.
    fn setup_bridge<R, W>(
        reader: FrameReader<R>,
        writer: FrameWriter<W>,
        kernel_tx: broadcast::Sender<ShellEvent>,
    ) -> (
        mpsc::UnboundedSender<Request>,
        Arc<AtomicU64>,
        Arc<AtomicU64>,
        Arc<AtomicU64>,
        Arc<AtomicU64>,
        mpsc::UnboundedReceiver<Response>,
    )
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let rtt_ms = Arc::new(AtomicU64::new(0));
        let last_pong_at = Arc::new(AtomicU64::new(0));
        let last_seen_seq = Arc::new(AtomicU64::new(0));
        let last_confirmed_epoch = Arc::new(AtomicU64::new(0));
        let ping_timestamps: Arc<Mutex<HashMap<u64, Instant>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Spawn request sender task
        let (request_tx, mut request_rx) = mpsc::unbounded_channel::<Request>();

        let mut writer = writer;
        tokio::spawn(async move {
            while let Some(request) = request_rx.recv().await {
                let priority = request.priority();
                if writer.write(&request, priority).await.is_err() {
                    break;
                }
            }
        });

        // Non-event response channel (unbounded to prevent deadlock with flow control)
        let (response_tx, response_rx) = mpsc::unbounded_channel::<Response>();

        // Spawn event bridge
        let bridge_rtt = rtt_ms.clone();
        let bridge_pong = last_pong_at.clone();
        let bridge_seq = last_seen_seq.clone();
        let bridge_epoch = last_confirmed_epoch.clone();
        let bridge_timestamps = ping_timestamps.clone();
        let bridge_request_tx = request_tx.clone();
        tokio::spawn(async move {
            crate::event_bridge::run(
                reader,
                kernel_tx,
                response_tx,
                bridge_request_tx,
                bridge_timestamps,
                bridge_rtt,
                bridge_pong,
                bridge_seq,
                bridge_epoch,
            )
            .await;
        });

        // Send initial flow control credits
        let _ = request_tx.send(Request::GrantCredits {
            bytes: 256 * 1024,
        });

        // Spawn ping loop
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

        (request_tx, rtt_ms, last_pong_at, last_seen_seq, last_confirmed_epoch, response_rx)
    }
}
