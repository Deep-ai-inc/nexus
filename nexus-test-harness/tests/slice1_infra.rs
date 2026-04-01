//! Slice 1: Infrastructure — verify Docker container lifecycle and SSH reachability.

use nexus_test_harness::container::TestEnv;

#[tokio::test]
async fn container_starts_and_ssh_is_reachable() {
    let env = TestEnv::start().await.expect("failed to start test env");

    // SSH port should be reachable
    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", env.ssh_port))
        .await
        .expect("SSH port not reachable");

    // Should receive SSH banner
    let mut buf = [0u8; 64];
    stream.readable().await.unwrap();
    let n = stream.try_read(&mut buf).unwrap_or(0);
    let banner = String::from_utf8_lossy(&buf[..n]);
    assert!(
        banner.contains("SSH"),
        "expected SSH banner, got: {banner:?}"
    );

    // Agent binary should be deployed
    let version = env
        .docker_exec("/home/testuser/.nexus/nexus-agent --protocol-version")
        .await
        .expect("agent binary not found");
    assert!(
        !version.trim().is_empty(),
        "agent binary returned empty version"
    );

    // Container cleans up on drop
    let container_id = env.container_id.clone();
    drop(env);

    let check = std::process::Command::new("docker")
        .args(["inspect", &container_id])
        .output()
        .unwrap();
    assert!(
        !check.status.success(),
        "container should be removed after drop"
    );
}
