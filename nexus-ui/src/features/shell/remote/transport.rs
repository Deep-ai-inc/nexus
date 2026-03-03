//! SSH transport management: spawning the SSH process, connecting to
//! the remote agent, and managing the byte stream.

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

/// Handle to the remote transport (SSH child process + codec).
pub(crate) struct TransportHandle {
    /// The SSH child process.
    child: Child,
    /// Current RTT in milliseconds (0 = not yet measured).
    pub rtt_ms: Arc<AtomicU64>,
    /// Last seen event sequence number from the agent.
    pub last_seen_seq: Arc<AtomicU64>,
    /// Receiver for non-event responses (ClassifyResult, CompleteResult, etc.)
    pub response_rx: mpsc::Receiver<Response>,
}

impl TransportHandle {
    /// Connect to a remote agent via the given transport.
    ///
    /// Dispatches to SSH, Docker, or kubectl based on the transport type.
    pub async fn connect(
        transport: &Transport,
        agent_path: &str,
        forwarded_env: HashMap<String, String>,
        kernel_tx: broadcast::Sender<ShellEvent>,
    ) -> Result<(Self, EnvInfo, [u8; 16], mpsc::UnboundedSender<super::RequestEnvelope>)> {
        let mut child = match transport {
            Transport::Ssh {
                destination,
                port,
                identity,
                extra_args,
            } => {
                let mut cmd = Command::new("ssh");
                cmd.arg("-o").arg("BatchMode=yes");
                cmd.arg("-o").arg("StrictHostKeyChecking=accept-new");
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
                cmd.stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                cmd.spawn()?
            }
        };

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

        let (env, session_token, _caps, request_tx, rtt_ms, last_seen_seq, response_rx) =
            Self::handshake(reader, writer, forwarded_env, kernel_tx).await?;

        Ok((
            Self {
                child,
                rtt_ms,
                last_seen_seq,
                response_rx,
            },
            env,
            session_token,
            request_tx,
        ))
    }

    /// Spawn an SSH process and connect to the remote agent (convenience wrapper).
    pub async fn connect_ssh(
        destination: &str,
        port: Option<u16>,
        identity: Option<&str>,
        extra_args: &[String],
        agent_path: &str,
        forwarded_env: HashMap<String, String>,
        kernel_tx: broadcast::Sender<ShellEvent>,
    ) -> Result<(Self, EnvInfo, [u8; 16], mpsc::UnboundedSender<super::RequestEnvelope>)> {
        let mut cmd = Command::new("ssh");

        // Basic SSH args
        cmd.arg("-o").arg("BatchMode=yes"); // Don't prompt for password
        cmd.arg("-o").arg("StrictHostKeyChecking=accept-new");

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
        cmd.arg(agent_path);

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("failed to take SSH stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("failed to take SSH stdout"))?;

        // Create codec over SSH stdin/stdout
        let codec = FrameCodec::new(stdout, stdin);
        let (reader, writer) = codec.into_parts();

        // Perform handshake
        let (env, session_token, _caps, request_tx, rtt_ms, last_seen_seq, response_rx) =
            Self::handshake(reader, writer, forwarded_env, kernel_tx).await?;

        Ok((
            Self {
                child,
                rtt_ms,
                last_seen_seq,
                response_rx,
            },
            env,
            session_token,
            request_tx,
        ))
    }

    /// Perform the Hello/HelloOk handshake and spawn event bridge tasks.
    async fn handshake<R, W>(
        mut reader: FrameReader<R>,
        mut writer: FrameWriter<W>,
        forwarded_env: HashMap<String, String>,
        kernel_tx: broadcast::Sender<ShellEvent>,
    ) -> Result<(
        EnvInfo,
        [u8; 16],
        AgentCaps,
        mpsc::UnboundedSender<super::RequestEnvelope>,
        Arc<AtomicU64>,
        Arc<AtomicU64>,
        mpsc::Receiver<Response>,
    )>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        // Send Hello
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

        // Read HelloOk
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

        // Shared state for RTT tracking
        let rtt_ms = Arc::new(AtomicU64::new(0));
        let last_seen_seq = Arc::new(AtomicU64::new(0));
        let ping_timestamps: Arc<Mutex<HashMap<u64, Instant>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Spawn request sender task
        let (request_tx, mut request_rx) =
            mpsc::unbounded_channel::<super::RequestEnvelope>();

        tokio::spawn(async move {
            while let Some(envelope) = request_rx.recv().await {
                let priority = envelope.request.priority();
                if writer.write(&envelope.request, priority).await.is_err() {
                    break;
                }
            }
        });

        // Non-event response channel (bounded for backpressure)
        let (response_tx, response_rx) = mpsc::channel::<Response>(64);

        // Spawn response reader task (event bridge)
        let bridge_rtt = rtt_ms.clone();
        let bridge_seq = last_seen_seq.clone();
        let bridge_timestamps = ping_timestamps.clone();
        let bridge_request_tx = request_tx.clone();
        tokio::spawn(async move {
            super::event_bridge::run(
                reader,
                kernel_tx,
                response_tx,
                bridge_request_tx,
                bridge_timestamps,
                bridge_rtt,
                bridge_seq,
            )
            .await;
        });

        // Send initial flow control credits
        let initial_grant = Request::GrantCredits {
            bytes: 256 * 1024,
        };
        let _ = request_tx.send(super::RequestEnvelope {
            request: initial_grant,
            response_tx: None,
        });

        // Spawn ping loop for RTT tracking
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
                if ping_tx
                    .send(super::RequestEnvelope {
                        request: Request::Ping { seq },
                        response_tx: None,
                    })
                    .is_err()
                {
                    break; // Channel closed — transport is gone
                }
            }
        });

        Ok((
            env,
            session_token,
            caps,
            request_tx,
            rtt_ms,
            last_seen_seq,
            response_rx,
        ))
    }

    /// Kill the SSH process.
    pub async fn kill(&mut self) -> Result<()> {
        self.child.kill().await?;
        Ok(())
    }

    /// Check if the SSH process is still running.
    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        Ok(self.child.try_wait()?)
    }
}
