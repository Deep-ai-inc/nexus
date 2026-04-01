//! Network manipulation inside the Docker container.
//!
//! Uses `docker exec` to run `iptables` and `tc` commands that simulate
//! network failures: blackholes, latency, packet loss, and connection freezes.

use anyhow::Result;

/// Control network conditions inside a Docker container.
#[derive(Clone)]
pub struct NetworkControl {
    container_id: String,
}

impl NetworkControl {
    pub fn new(container_id: String) -> Self {
        Self { container_id }
    }

    /// Shrink kernel TCP timeouts for fast failure detection.
    ///
    /// By default Linux retries TCP for ~15 minutes. With `tcp_retries2=3`,
    /// a dead connection is detected in ~2 seconds.
    pub async fn shrink_tcp_timeouts(&self) {
        // These may fail in unprivileged containers — best effort
        let _ = self.exec("sysctl -w net.ipv4.tcp_retries2=3 2>/dev/null").await;
        let _ = self.exec("sysctl -w net.ipv4.tcp_keepalive_time=1 2>/dev/null").await;
        let _ = self.exec("sysctl -w net.ipv4.tcp_keepalive_intvl=1 2>/dev/null").await;
        let _ = self.exec("sysctl -w net.ipv4.tcp_keepalive_probes=2 2>/dev/null").await;
    }

    /// Black hole all SSH traffic: packets go in, nothing comes back.
    /// Simulates laptop sleep / WiFi drop.
    pub async fn blackhole(&self) -> Result<()> {
        self.exec("iptables -A INPUT -p tcp --dport 22 -j DROP").await?;
        self.exec("iptables -A OUTPUT -p tcp --sport 22 -j DROP").await?;
        Ok(())
    }

    /// Restore all network conditions (flush iptables and tc).
    pub async fn restore(&self) -> Result<()> {
        let _ = self.exec("iptables -F").await;
        let _ = self.exec("tc qdisc del dev eth0 root 2>/dev/null").await;
        Ok(())
    }

    /// Degrade network: add latency and packet loss.
    pub async fn degrade(&self, delay_ms: u32, loss_pct: f32) -> Result<()> {
        self.exec(&format!(
            "tc qdisc add dev eth0 root netem delay {delay_ms}ms loss {loss_pct}%"
        ))
        .await?;
        Ok(())
    }

    /// Kill the sshd child process serving the test user.
    /// Simulates a clean server-side disconnect.
    pub async fn kill_sshd_session(&self) -> Result<()> {
        // Kill sshd session processes. We grep ps output to avoid pkill/pgrep
        // matching their own sh -c wrapper (which contains the pattern string).
        // Alpine ps aux: column 2 is PID.
        self.exec("ps aux | grep 'sshd.*testuser' | grep -v grep | awk '{print $2}' | xargs kill 2>/dev/null; true").await?;
        Ok(())
    }

    /// Freeze (SIGSTOP) the sshd child process.
    /// Simulates half-open TCP: SSH pipe looks alive but no data flows.
    pub async fn freeze_sshd_session(&self) -> Result<()> {
        self.exec("pkill -STOP -f 'sshd:.*testuser' || true").await?;
        Ok(())
    }

    /// Unfreeze (SIGCONT) the sshd child process.
    pub async fn unfreeze_sshd_session(&self) -> Result<()> {
        self.exec("pkill -CONT -f 'sshd:.*testuser' || true").await?;
        Ok(())
    }

    /// Check if the agent process is running in the container.
    pub async fn is_agent_alive(&self) -> bool {
        self.exec("pgrep -f nexus-agent")
            .await
            .map_or(false, |out| !out.trim().is_empty())
    }

    /// Check if a UDS socket exists for the given instance ID.
    pub async fn agent_socket_exists(&self, instance_id: &str) -> bool {
        self.exec(&format!(
            "test -S /home/testuser/.nexus/agent-{instance_id}.sock && echo yes"
        ))
        .await
        .map_or(false, |out| out.trim() == "yes")
    }

    async fn exec(&self, cmd: &str) -> Result<String> {
        let output = tokio::process::Command::new("docker")
            .args(["exec", &self.container_id, "sh", "-c", cmd])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("docker exec `{cmd}` failed: {stderr}");
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}
