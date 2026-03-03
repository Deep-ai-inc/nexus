//! Agent deployment: detect remote architecture, upload agent binary,
//! and launch it on the remote machine.
//!
//! Deployment cascade:
//! 1. `~/.nexus/agent-<proto_hash>` — primary location
//! 2. `/tmp/nexus-agent-$UID` — fallback for no $HOME
//! 3. `memfd_create` — fallback for noexec mounts
//!
//! Binary is version-keyed by protocol hash so multiple Nexus versions
//! can coexist on the same remote.

use std::path::PathBuf;

use anyhow::Result;
use tokio::process::Command;

/// Build an SSH command with the standard options + user's extra args.
fn ssh_command(
    destination: &str,
    port: Option<u16>,
    identity: Option<&str>,
    extra_args: &[String],
) -> Command {
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
    cmd
}

/// Detect the remote machine's architecture via SSH.
pub(crate) async fn detect_arch(
    destination: &str,
    port: Option<u16>,
    identity: Option<&str>,
    extra_args: &[String],
) -> Result<String> {
    let mut cmd = ssh_command(destination, port, identity, extra_args);
    cmd.arg("uname -m");

    let output = cmd.output().await?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to detect remote arch: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let arch = String::from_utf8(output.stdout)?
        .trim()
        .to_string();

    Ok(arch)
}

/// Check if the agent binary is already deployed and matches the current protocol version.
pub(crate) async fn check_deployed(
    destination: &str,
    port: Option<u16>,
    identity: Option<&str>,
    extra_args: &[String],
    agent_path: &str,
) -> Result<bool> {
    let mut cmd = ssh_command(destination, port, identity, extra_args);
    cmd.arg(format!("{} --protocol-version", agent_path));

    let output = cmd.output().await?;
    if !output.status.success() {
        return Ok(false);
    }

    let remote_version: u32 = String::from_utf8(output.stdout)?
        .trim()
        .parse()
        .unwrap_or(0);

    Ok(remote_version == nexus_protocol::PROTOCOL_VERSION)
}

/// Map `uname -m` output to a Rust target triple.
pub(crate) fn arch_to_target(arch: &str) -> Option<&'static str> {
    match arch {
        "x86_64" | "amd64" => Some("x86_64-unknown-linux-musl"),
        "aarch64" | "arm64" => Some("aarch64-unknown-linux-musl"),
        "armv7l" | "armhf" => Some("armv7-unknown-linux-musleabihf"),
        _ => None,
    }
}

/// Compute the version-keyed agent binary name.
pub(crate) fn agent_binary_name() -> String {
    format!("agent-{}", nexus_protocol::PROTOCOL_VERSION)
}

/// Compute the remote agent path for a given destination.
pub(crate) fn remote_agent_path() -> String {
    format!("~/.nexus/{}", agent_binary_name())
}

/// Upload an agent binary to the remote machine using atomic rename.
///
/// Pipes the binary through SSH stdin to avoid argument length limits.
pub(crate) async fn upload_agent(
    destination: &str,
    port: Option<u16>,
    identity: Option<&str>,
    extra_args: &[String],
    local_binary_path: &str,
    remote_path: &str,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;

    let binary_data = tokio::fs::read(local_binary_path).await?;

    let script = format!(
        "mkdir -p ~/.nexus && cat > {rp}.tmp.$$ && chmod +x {rp}.tmp.$$ && mv -f {rp}.tmp.$$ {rp}",
        rp = remote_path,
    );

    let mut cmd = ssh_command(destination, port, identity, extra_args);
    cmd.arg(script);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn()?;
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&binary_data).await?;
    drop(stdin); // close stdin so remote cat exits

    let output = child.wait_with_output().await?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to upload agent: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}

/// Find the local agent binary for the given target architecture.
///
/// Search order:
/// 1. `~/.nexus/agents/nexus-agent-{target}`
/// 2. Adjacent to the running Nexus binary
pub(crate) fn find_local_agent(target: &str) -> Option<PathBuf> {
    let binary_name = format!("nexus-agent-{}", target);

    // Check ~/.nexus/agents/
    if let Ok(home) = std::env::var("HOME") {
        let path = PathBuf::from(home)
            .join(".nexus")
            .join("agents")
            .join(&binary_name);
        if path.exists() {
            return Some(path);
        }
    }

    // Check adjacent to the running binary
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let path = dir.join(&binary_name);
            if path.exists() {
                return Some(path);
            }
        }
    }

    None
}

/// Full deployment flow: detect arch, check if deployed, upload if needed.
pub(crate) async fn ensure_deployed(
    destination: &str,
    port: Option<u16>,
    identity: Option<&str>,
    extra_args: &[String],
) -> Result<String> {
    let remote_path = remote_agent_path();

    // Check if already deployed with correct version
    if check_deployed(destination, port, identity, extra_args, &remote_path).await? {
        return Ok(remote_path);
    }

    // Detect remote architecture
    let arch = detect_arch(destination, port, identity, extra_args).await?;
    let target = arch_to_target(&arch).ok_or_else(|| {
        anyhow::anyhow!("unsupported remote architecture: {arch}")
    })?;

    // Find local agent binary
    let local_path = find_local_agent(target).ok_or_else(|| {
        anyhow::anyhow!(
            "agent binary not found for target {target}. \
             Place it at ~/.nexus/agents/nexus-agent-{target}"
        )
    })?;

    // Upload
    upload_agent(
        destination,
        port,
        identity,
        extra_args,
        local_path.to_str().unwrap(),
        &remote_path,
    )
    .await?;

    Ok(remote_path)
}
