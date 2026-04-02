//! Nest/Unnest relay mode for agent chaining.
//!
//! When the agent receives a `Nest` request, it:
//! 1. Deploys a child agent via the specified transport
//! 2. Enters relay mode (transparent byte forwarding parent ↔ child)
//! 3. Intercepts only control messages (GrantCredits, Ping, TerminalResize)
//!
//! On child pipe EOF (container killed, SSH dropped):
//! - Exits relay mode
//! - Sends `Response::ChildLost { reason, surviving_env }` to parent
//! - Agent becomes active again at this level

use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::Result;
use nexus_protocol::codec::{
    decode_payload, encode_payload, FrameCodec, FrameReader, FrameWriter, FLAG_EVENT,
};
use nexus_protocol::messages::{EnvInfo, Request, Response, Transport};
use nexus_protocol::{priority, ClientCaps, PROTOCOL_VERSION};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};
use tokio::task::JoinHandle;

use crate::session::RingBuffer;

/// A frame to be forwarded from relay child to parent.
pub(crate) struct RelayFrame {
    pub data: Vec<u8>,
    pub priority: u8,
    pub flags: u8,
}

/// Swappable sender for child→parent frames.
/// Set to None when parent disconnects; replaced with a new sender on reconnect.
pub(crate) type ParentFrameSender = Arc<std::sync::Mutex<Option<mpsc::UnboundedSender<RelayFrame>>>>;

/// Active relay state. Stored in Agent struct, survives parent disconnects.
pub(crate) struct ActiveRelay {
    /// The child transport process (SSH, docker, etc.)
    pub child: Child,
    /// Writer to the child's stdin (for forwarding requests).
    pub child_writer: FrameWriter<ChildStdin>,
    /// Background task reading child stdout and forwarding via parent_sender.
    pub reader_task: JoinHandle<()>,
    /// Fires when the child dies or its pipe closes.
    pub child_lost_rx: oneshot::Receiver<String>,
    /// Swappable sender for routing child frames to the current parent writer.
    /// On parent disconnect, set to None. On reconnect, set to a new sender
    /// whose receiver is consumed by a forwarding task writing to the new parent.
    pub parent_sender: ParentFrameSender,
    /// Handle to the current forwarding task (writes relay frames to parent).
    /// Aborted and replaced on reconnect.
    pub forwarder_task: Option<JoinHandle<()>>,
}

impl ActiveRelay {
    /// Clean up the relay: kill the child and abort tasks.
    /// Unregisters the child PID from the Tokio-managed set.
    pub async fn cleanup(mut self, tokio_pids: &std::sync::Mutex<std::collections::HashSet<u32>>) {
        if let Some(pid) = self.child.id() {
            tokio_pids.lock().unwrap().remove(&pid);
        }
        self.reader_task.abort();
        if let Some(fwd) = self.forwarder_task.take() {
            fwd.abort();
        }
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }

    /// Disconnect from parent: stop forwarding but keep child alive.
    pub fn disconnect_parent(&mut self) {
        *self.parent_sender.lock().unwrap() = None;
        if let Some(fwd) = self.forwarder_task.take() {
            fwd.abort();
        }
    }

    /// Reconnect to a new parent writer: create a new forwarding channel and task.
    pub fn reconnect_parent<W>(&mut self, parent_writer: Arc<Mutex<FrameWriter<W>>>)
    where
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (tx, mut rx) = mpsc::unbounded_channel::<RelayFrame>();
        *self.parent_sender.lock().unwrap() = Some(tx);

        let fwd_writer = parent_writer;
        self.forwarder_task = Some(tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                let mut w = fwd_writer.lock().await;
                if w.write_raw_flagged(&frame.data, frame.priority, frame.flags)
                    .await
                    .is_err()
                {
                    break;
                }
            }
        }));
    }
}

/// Spawn a child agent via the given transport and perform handshake.
///
/// Returns the codec halves, child process, and child's EnvInfo.
/// The caller uses these to set up `ActiveRelay` and `start_relay_reader()`.
pub(crate) async fn spawn_and_handshake(
    transport: &Transport,
    forwarded_env: HashMap<String, String>,
) -> Result<(FrameReader<ChildStdout>, FrameWriter<ChildStdin>, Child, EnvInfo)> {
    let mut child = spawn_child(transport).await?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("no child stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("no child stdout"))?;

    let codec = FrameCodec::new(stdout, stdin);
    let (mut reader, mut writer) = codec.into_parts();

    // Hello handshake with child
    let hello = Request::Hello {
        protocol_version: PROTOCOL_VERSION,
        capabilities: ClientCaps {
            flow_control: true,
            resume: false,
            nesting: true,
            file_transfer: true,
        },
        forwarded_env,
    };
    writer
        .write(&hello, hello.priority())
        .await
        .map_err(|e| anyhow::anyhow!("child handshake failed: {e}"))?;

    let response: Response = reader
        .read()
        .await
        .map_err(|e| anyhow::anyhow!("child handshake failed: {e}"))?;

    let env = match response {
        Response::HelloOk { env, .. } => env,
        Response::Error { message, .. } => {
            anyhow::bail!("child agent rejected Hello: {message}");
        }
        other => {
            anyhow::bail!("unexpected child response: {other:?}");
        }
    };

    // Grant huge credits to child so it never blocks on our side
    let grant = Request::GrantCredits {
        bytes: 1_000_000_000,
    };
    writer
        .write(&grant, grant.priority())
        .await
        .map_err(|e| anyhow::anyhow!("failed to send initial credits to child: {e}"))?;

    Ok((reader, writer, child, env))
}

/// Spawn the relay reader task.
///
/// Reads frames from the child, rewrites Event seq numbers, pushes to ring
/// buffer, and forwards to the parent via the swappable `parent_sender`.
///
/// The reader task survives parent disconnections — when the parent_sender
/// is None, events are still pushed to the ring buffer (for resume replay)
/// but not forwarded. On reconnect, the caller sets a new sender.
///
/// Returns the task handle, child_lost receiver, and the swappable sender.
pub(crate) fn start_relay_reader(
    mut child_reader: FrameReader<ChildStdout>,
    parent_sender: ParentFrameSender,
    credits: Arc<Semaphore>,
    next_seq: Arc<AtomicU64>,
    ring_buffer: Arc<tokio::sync::Mutex<RingBuffer>>,
) -> (JoinHandle<()>, oneshot::Receiver<String>)
{
    let (child_lost_tx, child_lost_rx) = oneshot::channel();

    let task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let reason = loop {
            match child_reader.read_raw(&mut buf).await {
                Ok((child_priority, flags)) => {
                    // Credit-gate non-control frames
                    if child_priority > priority::CONTROL {
                        let size = buf.len().max(1) as u32;
                        match credits.acquire_many(size).await {
                            Ok(permit) => permit.forget(),
                            Err(_) => break "credit semaphore closed".to_string(),
                        }
                    }

                    if flags & FLAG_EVENT != 0 {
                        // Decode child event, rewrite seq, re-encode, push to ring buffer
                        let child_resp: Response = match decode_payload(&buf) {
                            Ok(r) => r,
                            Err(e) => {
                                tracing::warn!("relay: failed to decode event: {e}");
                                continue;
                            }
                        };

                        let event = match child_resp {
                            Response::Event { event, .. } => event,
                            _ => {
                                // FLAG_EVENT set but not an Event? Forward as-is.
                                let tx = parent_sender.lock().unwrap().clone();
                                if let Some(tx) = tx {
                                    let _ = tx.send(RelayFrame {
                                        data: buf.clone(),
                                        priority: child_priority,
                                        flags,
                                    });
                                }
                                continue;
                            }
                        };

                        let new_seq = next_seq.fetch_add(1, Ordering::Relaxed);
                        let rewritten = Response::Event {
                            seq: new_seq,
                            event,
                        };
                        let encoded = match encode_payload(&rewritten) {
                            Ok(bytes) => bytes,
                            Err(e) => {
                                tracing::warn!("relay: failed to encode event: {e}");
                                continue;
                            }
                        };

                        // Always push to ring buffer (survives parent disconnect)
                        ring_buffer
                            .lock()
                            .await
                            .push_raw(new_seq, encoded.clone());

                        // Forward to parent if connected
                        let tx = parent_sender.lock().unwrap().clone();
                        if let Some(tx) = tx {
                            let _ = tx.send(RelayFrame {
                                data: encoded,
                                priority: priority::INTERACTIVE,
                                flags: FLAG_EVENT,
                            });
                        }
                    } else {
                        // Non-event: forward raw without decode
                        let tx = parent_sender.lock().unwrap().clone();
                        if let Some(tx) = tx {
                            let _ = tx.send(RelayFrame {
                                data: buf.clone(),
                                priority: child_priority,
                                flags,
                            });
                        }
                    }
                }
                Err(nexus_protocol::codec::CodecError::ConnectionClosed) => {
                    break "child connection closed".to_string();
                }
                Err(e) => {
                    break format!("child read error: {e}");
                }
            }
        };

        let _ = child_lost_tx.send(reason);
    });

    (task, child_lost_rx)
}

// =========================================================================
// Transport spawning & deployment helpers
// =========================================================================

/// Spawn the transport child process (SSH/Docker/kubectl/Command).
async fn spawn_child(transport: &Transport) -> Result<Child> {
    let child = match transport {
        Transport::Ssh {
            destination,
            port,
            identity,
            extra_args,
        } => {
            let agent_path = detect_and_deploy_agent_ssh(
                destination,
                port.as_ref(),
                identity.as_deref(),
                extra_args,
            )
            .await?;
            let mut cmd = Command::new("ssh");
            cmd.arg("-o").arg("BatchMode=yes");
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
            cmd.arg(&agent_path);
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
            cmd.arg("/tmp/nexus-agent");
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
            cmd.arg(pod).arg("--").arg("/tmp/nexus-agent");
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
    Ok(child)
}

/// Map `uname -m` output to the Rust target architecture string.
fn uname_to_target_arch(uname: &str) -> &str {
    match uname {
        "x86_64" | "amd64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        "armv7l" | "armhf" => "arm",
        other => other,
    }
}

/// Detect remote architecture and deploy agent via SSH.
/// Returns the path to the agent binary on the remote.
async fn detect_and_deploy_agent_ssh(
    destination: &str,
    port: Option<&u16>,
    identity: Option<&str>,
    extra_args: &[String],
) -> Result<String> {
    // Detect arch
    let mut cmd = Command::new("ssh");
    cmd.arg("-o").arg("BatchMode=yes");
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
    cmd.arg(destination).arg("uname -m");
    let output = cmd.output().await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("failed to detect remote architecture: {stderr}");
    }
    let remote_uname = String::from_utf8(output.stdout)?.trim().to_string();
    if remote_uname.is_empty() {
        anyhow::bail!("failed to detect remote architecture: uname returned empty output");
    }

    // Check if agent already deployed
    let agent_path = format!("~/.nexus/agent-{}", PROTOCOL_VERSION);
    let mut check_cmd = Command::new("ssh");
    check_cmd.arg("-o").arg("BatchMode=yes");
    check_cmd.arg("-o").arg("StrictHostKeyChecking=accept-new");
    if let Some(port) = port {
        check_cmd.arg("-p").arg(port.to_string());
    }
    if let Some(identity) = identity {
        check_cmd.arg("-i").arg(identity);
    }
    for arg in extra_args {
        check_cmd.arg(arg);
    }
    check_cmd
        .arg(destination)
        .arg(format!(
            "{} --protocol-version 2>/dev/null",
            agent_path
        ));
    let output = check_cmd.output().await?;
    if output.status.success() {
        let remote_version: u32 = String::from_utf8(output.stdout)?
            .trim()
            .parse()
            .unwrap_or(0);
        if remote_version == PROTOCOL_VERSION {
            return Ok(agent_path);
        }
    }

    // Verify architecture compatibility
    let remote_arch = uname_to_target_arch(&remote_uname);
    let my_arch = std::env::consts::ARCH;
    if remote_arch != my_arch {
        anyhow::bail!(
            "architecture mismatch: this agent is {my_arch} but remote is \
             {remote_uname} ({remote_arch}). Cannot self-replicate across \
             architectures. Deploy the correct agent binary manually or \
             via the Nexus UI."
        );
    }

    // Self-replicate: read our own binary and upload
    #[cfg(target_os = "linux")]
    {
        let self_exe = tokio::fs::read("/proc/self/exe").await?;
        upload_binary_ssh(destination, port, identity, extra_args, &self_exe, &agent_path).await?;
        return Ok(agent_path);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let exe_path = std::env::current_exe()?;
        let self_exe = tokio::fs::read(&exe_path).await?;
        upload_binary_ssh(destination, port, identity, extra_args, &self_exe, &agent_path).await?;
        Ok(agent_path)
    }
}

/// Upload a binary to a remote host via SSH with atomic rename.
async fn upload_binary_ssh(
    destination: &str,
    port: Option<&u16>,
    identity: Option<&str>,
    extra_args: &[String],
    binary: &[u8],
    remote_path: &str,
) -> Result<()> {
    let mut cmd = Command::new("ssh");
    cmd.arg("-o").arg("BatchMode=yes");
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
    cmd.arg(format!(
        "mkdir -p ~/.nexus && cat > {}.tmp.$$ && chmod +x {}.tmp.$$ && mv -f {}.tmp.$$ {}",
        remote_path, remote_path, remote_path, remote_path
    ));
    cmd.stdin(Stdio::piped());

    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(binary).await?;
        // Drop stdin to signal EOF
    }
    let status = child.wait().await?;
    if !status.success() {
        anyhow::bail!("failed to upload agent binary");
    }
    Ok(())
}
