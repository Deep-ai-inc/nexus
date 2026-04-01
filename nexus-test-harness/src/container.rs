//! Docker container lifecycle for integration tests.
//!
//! `TestEnv` manages a Docker container running sshd, deploys the agent
//! binary, and provides SSH connection details. Ephemeral port binding
//! ensures parallel test execution.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::network::NetworkControl;

const IMAGE_NAME: &str = "nexus-test-sshd";
const CONTAINER_PREFIX: &str = "nexus-test-";

/// A running test environment: Docker container with sshd + deployed agent.
pub struct TestEnv {
    pub container_id: String,
    pub ssh_port: u16,
    pub ssh_key: PathBuf,
    pub network: NetworkControl,
    agent_logs: Option<String>,
}

impl TestEnv {
    /// Start a new test environment.
    ///
    /// Builds the Docker image (cached), starts a container with an ephemeral
    /// SSH port, waits for sshd to be ready, and deploys the agent binary.
    pub async fn start() -> Result<Self> {
        Self::ensure_image_built()?;

        // Start container with ephemeral port and NET_ADMIN for iptables/tc
        let container_id = run_cmd(
            "docker",
            &[
                "run", "-d",
                "--cap-add", "NET_ADMIN",
                "-p", "0:22", // ephemeral host port
                "--name", &format!("{}{}", CONTAINER_PREFIX, uuid_short()),
                IMAGE_NAME,
            ],
        )?;
        let container_id = container_id.trim().to_string();

        // Resolve the ephemeral port
        let port_str = run_cmd(
            "docker",
            &["port", &container_id, "22"],
        )?;
        let ssh_port = parse_docker_port(&port_str)
            .context("failed to parse Docker port mapping")?;

        let ssh_key = docker_dir().join("id_test");

        let env = Self {
            network: NetworkControl::new(container_id.clone()),
            container_id,
            ssh_port,
            ssh_key,
            agent_logs: None,
        };

        // Wait for sshd to accept connections
        env.wait_for_ssh(Duration::from_secs(10)).await?;

        // Deploy agent binary
        env.deploy_agent().await?;

        // Shrink TCP timeouts inside container for fast failure detection
        env.network.shrink_tcp_timeouts().await;

        Ok(env)
    }

    /// Wait until we can establish an SSH connection.
    async fn wait_for_ssh(&self, timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if tokio::time::Instant::now() > deadline {
                anyhow::bail!(
                    "sshd not ready after {}s on port {}",
                    timeout.as_secs(),
                    self.ssh_port
                );
            }

            let result = tokio::net::TcpStream::connect(
                format!("127.0.0.1:{}", self.ssh_port),
            )
            .await;

            if result.is_ok() {
                return Ok(());
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Deploy the agent binary into the container.
    async fn deploy_agent(&self) -> Result<()> {
        let agent_bin = find_agent_binary()?;

        // Copy into the container
        run_cmd(
            "docker",
            &[
                "cp",
                agent_bin.to_str().unwrap(),
                &format!("{}:/home/testuser/.nexus/nexus-agent", self.container_id),
            ],
        )?;

        // Make executable and set ownership
        self.docker_exec("chmod +x /home/testuser/.nexus/nexus-agent").await?;
        self.docker_exec("chown testuser:testuser /home/testuser/.nexus/nexus-agent").await?;

        Ok(())
    }

    /// Run a command inside the container.
    pub async fn docker_exec(&self, cmd: &str) -> Result<String> {
        let output = tokio::process::Command::new("docker")
            .args(["exec", &self.container_id, "sh", "-c", cmd])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("docker exec failed: {stderr}");
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Set an environment variable in the container's testuser profile.
    /// Takes effect on the next SSH session (agent startup).
    pub async fn set_agent_env(&self, key: &str, value: &str) -> Result<()> {
        self.docker_exec(&format!(
            "echo 'export {key}={value}' >> /home/testuser/.bashrc"
        ))
        .await?;
        Ok(())
    }

    /// The remote path to the agent binary.
    pub fn agent_path(&self) -> &str {
        "/home/testuser/.nexus/nexus-agent"
    }

    /// SSH destination string.
    pub fn destination(&self) -> &str {
        "testuser@127.0.0.1"
    }

    /// Build the transport description for the protocol.
    pub fn transport(&self) -> nexus_protocol::Transport {
        nexus_protocol::Transport::Ssh {
            destination: self.destination().to_string(),
            port: Some(self.ssh_port),
            identity: Some(self.ssh_key.to_string_lossy().to_string()),
            extra_args: vec![
                "-o".to_string(), "StrictHostKeyChecking=no".to_string(),
                "-o".to_string(), "UserKnownHostsFile=/dev/null".to_string(),
                "-o".to_string(), "LogLevel=ERROR".to_string(),
            ],
        }
    }

    /// Capture and return agent logs from the container (for test failure debugging).
    pub async fn capture_agent_logs(&mut self) -> String {
        let logs = self
            .docker_exec("cat /tmp/nexus-agent.log 2>/dev/null || echo '(no agent log)'")
            .await
            .unwrap_or_else(|e| format!("(failed to capture logs: {e})"));
        self.agent_logs = Some(logs.clone());
        logs
    }

    /// Dump diagnostic info on test failure.
    pub async fn dump_diagnostics(&mut self) {
        eprintln!("=== Test Failure Diagnostics ===");

        let agent_logs = self.capture_agent_logs().await;
        eprintln!("--- Agent Logs (last 50 lines) ---");
        for line in agent_logs.lines().rev().take(50).collect::<Vec<_>>().into_iter().rev() {
            eprintln!("  {line}");
        }

        if let Ok(ps) = self.docker_exec("ps aux").await {
            eprintln!("--- Container Processes ---");
            eprintln!("{ps}");
        }

        if let Ok(sockets) = self.docker_exec("ls -la /home/testuser/.nexus/agent-*.sock 2>/dev/null || echo '(none)'").await {
            eprintln!("--- Agent UDS Sockets ---");
            eprintln!("  {sockets}");
        }

        eprintln!("================================");
    }

    fn ensure_image_built() -> Result<()> {
        let docker_dir = docker_dir();

        // Check if image exists
        let check = Command::new("docker")
            .args(["image", "inspect", IMAGE_NAME])
            .output()?;

        if check.status.success() {
            return Ok(());
        }

        // Build it
        eprintln!("Building test Docker image '{IMAGE_NAME}'...");
        let output = Command::new("docker")
            .args(["build", "-t", IMAGE_NAME, "."])
            .current_dir(&docker_dir)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("docker build failed: {stderr}");
        }

        Ok(())
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        // Force-remove container on cleanup
        let _ = Command::new("docker")
            .args(["rm", "-f", &self.container_id])
            .output();
    }
}

/// Path to the docker/ directory within the harness crate.
fn docker_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("docker")
}

/// Find the agent binary for the container's architecture.
fn find_agent_binary() -> Result<PathBuf> {
    // For Docker on macOS (typically linux/amd64 via Docker Desktop)
    // Check target/agents first, then ~/.nexus/agents
    let candidates = [
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/agents/nexus-agent-linux-x86_64"),
        dirs::home_dir()
            .unwrap_or_default()
            .join(".nexus/agents/nexus-agent-x86_64-unknown-linux-musl"),
    ];

    for path in &candidates {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    anyhow::bail!(
        "agent binary not found. Run ./scripts/build-agent.sh x86_64 first.\n\
         Searched: {:?}",
        candidates,
    )
}

/// Run a command and return stdout.
fn run_cmd(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{program} failed: {stderr}");
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Parse "0.0.0.0:12345" or ":::12345" from `docker port` output.
fn parse_docker_port(output: &str) -> Option<u16> {
    // docker port outputs lines like "0.0.0.0:49153" or "[::]:49153"
    output
        .lines()
        .next()?
        .rsplit(':')
        .next()?
        .trim()
        .parse()
        .ok()
}

/// Generate a short random ID for container names.
fn uuid_short() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Mix time with random state to avoid collisions in parallel tests
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(t);
    format!("{:08x}", hasher.finish() as u32)
}
