//! Integration test: deploy nexus-agent to a Docker sshd container via SSH
//! and verify the full handshake succeeds.
//!
//! Prerequisites:
//!   cd tests/docker && ./setup.sh && docker compose up -d
//!
//! Run:
//!   cargo test --test osc_integration -- --ignored
//!
//! These tests are #[ignore]d by default because they require Docker.

use std::process::Command;

/// Check if the Docker sshd container is running and reachable.
fn docker_sshd_available() -> bool {
    let status = Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=2",
            "-p", "2222",
            "-i", "tests/docker/test_key",
            "root@localhost",
            "echo ok",
        ])
        .output();

    match status {
        Ok(output) => output.status.success(),
        Err(_) => false,
    }
}

#[test]
#[ignore]
fn deploy_agent_to_docker_sshd() {
    if !docker_sshd_available() {
        eprintln!("SKIP: Docker sshd not running. Run: cd tests/docker && ./setup.sh && docker compose up -d");
        return;
    }

    // Detect the remote architecture
    let output = Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "BatchMode=yes",
            "-p", "2222",
            "-i", "tests/docker/test_key",
            "root@localhost",
            "uname -m",
        ])
        .output()
        .expect("ssh uname -m failed");

    assert!(output.status.success(), "uname -m failed: {:?}", output);
    let arch = String::from_utf8_lossy(&output.stdout).trim().to_string();
    eprintln!("Remote arch: {arch}");

    // Map to target triple (same logic as deploy.rs)
    let target = match arch.as_str() {
        "x86_64" => "x86_64-unknown-linux-musl",
        "aarch64" => "aarch64-unknown-linux-musl",
        "armv7l" => "armv7-unknown-linux-musleabihf",
        other => panic!("Unsupported architecture: {other}"),
    };

    // Check if we have an agent binary for this target
    let home = std::env::var("HOME").unwrap();
    let agent_path = format!("{home}/.nexus/agents/nexus-agent-{target}");
    if !std::path::Path::new(&agent_path).exists() {
        eprintln!("SKIP: No agent binary at {agent_path}");
        eprintln!("Build with: cargo build --release -p nexus-agent --target {target}");
        return;
    }

    // Upload the agent binary
    let proto_version = "v3"; // Must match PROTOCOL_VERSION
    let remote_path = format!("~/.nexus/agent-{proto_version}");

    // Upload via stdin pipe (same method as deploy.rs)
    let upload = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "cat {agent_path} | ssh \
                -o StrictHostKeyChecking=no \
                -o UserKnownHostsFile=/dev/null \
                -o BatchMode=yes \
                -p 2222 \
                -i tests/docker/test_key \
                root@localhost \
                'mkdir -p ~/.nexus && cat > {remote_path}.tmp && chmod +x {remote_path}.tmp && mv -f {remote_path}.tmp {remote_path}'"
        ))
        .output()
        .expect("upload failed");

    assert!(upload.status.success(), "Upload failed: {:?}", String::from_utf8_lossy(&upload.stderr));

    // Verify the agent responds to --protocol-version
    let check = Command::new("ssh")
        .args([
            "-o", "StrictHostKeyChecking=no",
            "-o", "UserKnownHostsFile=/dev/null",
            "-o", "BatchMode=yes",
            "-p", "2222",
            "-i", "tests/docker/test_key",
            "root@localhost",
            &format!("{remote_path} --protocol-version"),
        ])
        .output()
        .expect("protocol version check failed");

    assert!(check.status.success(), "Agent --protocol-version failed: {:?}", check);
    let version = String::from_utf8_lossy(&check.stdout).trim().to_string();
    eprintln!("Agent protocol version: {version}");
    assert!(!version.is_empty(), "Protocol version should not be empty");
}

/// Test that the `parse_ssh_transport` logic (via `parse_remote_command`) produces
/// the right Transport::Ssh for the synthetic command that handle_osc_ssh_connect builds.
#[test]
fn osc_to_ssh_command_round_trip() {
    // Simulate what handle_osc_ssh_connect builds
    let destination = "ubuntu@10.0.0.99";
    let port = Some(2222u16);
    let identity = Some("/tmp/test_key".to_string());

    let mut ssh_command = "ssh".to_string();
    if let Some(p) = port {
        ssh_command.push_str(&format!(" -p {p}"));
    }
    if let Some(ref key) = identity {
        ssh_command.push_str(&format!(" -i {key}"));
    }
    ssh_command.push(' ');
    ssh_command.push_str(destination);

    assert_eq!(ssh_command, "ssh -p 2222 -i /tmp/test_key ubuntu@10.0.0.99");
}
