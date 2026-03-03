//! Nest/Unnest relay mode for agent chaining.
//!
//! When the agent receives a `Nest` request, it:
//! 1. Deploys a child agent via the specified transport
//! 2. Enters relay mode (transparent byte forwarding parent ↔ child)
//! 3. Intercepts only `Unnest` to tear down the child
//!
//! On child pipe EOF (container killed, SSH dropped):
//! - Exits relay mode
//! - Sends `Response::ChildLost { reason }` to parent
//! - Agent becomes active again at this level

use std::process::Stdio;

use anyhow::Result;
use nexus_protocol::codec::{FrameCodec, FrameReader, FrameWriter};
use nexus_protocol::messages::{EnvInfo, Request, Response, Transport};
use nexus_protocol::{AgentCaps, ClientCaps, PROTOCOL_VERSION};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::process::{Child, Command};

/// State of the relay when active.
pub(crate) struct RelayState {
    /// The child transport process (SSH, docker, etc.)
    child: Child,
    /// Child environment info.
    pub env: EnvInfo,
}

impl RelayState {
    /// Spawn a child agent via the given transport and perform handshake.
    pub async fn spawn(
        transport: &Transport,
        forwarded_env: std::collections::HashMap<String, String>,
    ) -> Result<Self> {
        let mut child = match transport {
            Transport::Ssh {
                destination,
                port,
                identity,
                extra_args,
            } => {
                let agent_path = detect_and_deploy_agent_ssh(destination, port.as_ref(), identity.as_deref()).await?;
                let mut cmd = Command::new("ssh");
                cmd.arg("-o").arg("BatchMode=yes");
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
                // Self-replicate: copy agent binary into container first
                // For now, assume agent is already deployed
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

        // Get stdin/stdout for the child
        let stdin = child.stdin.take().ok_or_else(|| anyhow::anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("no stdout"))?;

        // Handshake with child agent
        let codec = FrameCodec::new(stdout, stdin);
        let (mut reader, mut writer) = codec.into_parts();

        let hello = Request::Hello {
            protocol_version: PROTOCOL_VERSION,
            capabilities: ClientCaps {
                flow_control: false,
                resume: false,
                nesting: true,
                file_transfer: true,
            },
            forwarded_env,
        };
        writer.write(&hello, hello.priority()).await?;

        let response: Response = reader.read().await
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

        // Put stdin/stdout back into the child process
        // (We consumed them for handshake — in production we'd keep the reader/writer
        // and relay bytes. For now, store the child handle for cleanup.)

        Ok(Self { child, env })
    }

    /// Kill the child process.
    pub async fn kill(&mut self) -> Result<()> {
        self.child.kill().await?;
        Ok(())
    }
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
) -> Result<String> {
    // Detect arch
    let mut cmd = Command::new("ssh");
    cmd.arg("-o").arg("BatchMode=yes");
    if let Some(port) = port {
        cmd.arg("-p").arg(port.to_string());
    }
    if let Some(identity) = identity {
        cmd.arg("-i").arg(identity);
    }
    cmd.arg(destination).arg("uname -m");
    let output = cmd.output().await?;
    let remote_uname = String::from_utf8(output.stdout)?.trim().to_string();

    // Check if agent already deployed
    let agent_path = format!("~/.nexus/agent-{}", PROTOCOL_VERSION);
    let mut check_cmd = Command::new("ssh");
    check_cmd.arg("-o").arg("BatchMode=yes");
    if let Some(port) = port {
        check_cmd.arg("-p").arg(port.to_string());
    }
    if let Some(identity) = identity {
        check_cmd.arg("-i").arg(identity);
    }
    check_cmd.arg(destination)
        .arg(format!("{} --protocol-version 2>/dev/null", agent_path));
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

    // Verify architecture compatibility before self-replicating.
    // If the remote arch doesn't match our compile target, we can't
    // just copy /proc/self/exe — it would be the wrong binary format.
    let remote_arch = uname_to_target_arch(&remote_uname);
    let my_arch = std::env::consts::ARCH; // e.g. "x86_64", "aarch64"
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
        upload_binary_ssh(destination, port, identity, &self_exe, &agent_path).await?;
        return Ok(agent_path);
    }

    #[cfg(not(target_os = "linux"))]
    {
        let exe_path = std::env::current_exe()?;
        let self_exe = tokio::fs::read(&exe_path).await?;
        upload_binary_ssh(destination, port, identity, &self_exe, &agent_path).await?;
        Ok(agent_path)
    }
}

/// Upload a binary to a remote host via SSH with atomic rename.
async fn upload_binary_ssh(
    destination: &str,
    port: Option<&u16>,
    identity: Option<&str>,
    binary: &[u8],
    remote_path: &str,
) -> Result<()> {
    let mut cmd = Command::new("ssh");
    cmd.arg("-o").arg("BatchMode=yes");
    if let Some(port) = port {
        cmd.arg("-p").arg(port.to_string());
    }
    if let Some(identity) = identity {
        cmd.arg("-i").arg(identity);
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
