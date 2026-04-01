//! Slice 2: Happy path — connect, execute command, verify output.

use std::time::Duration;

use nexus_api::BlockId;
use nexus_test_harness::client::TestClient;
use nexus_test_harness::container::TestEnv;

#[tokio::test]
async fn connect_and_execute_echo() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Verify we got valid env info
    assert_eq!(client.env.user, "testuser");
    assert!(!client.instance_id().is_empty(), "instance_id should be set");

    // Execute a simple command
    let block_id = BlockId(1);
    client.execute("echo hello-nexus", block_id);

    let (output, exit_code) = client.collect_output(block_id, Duration::from_secs(5)).await;
    assert!(
        output.contains("hello-nexus"),
        "expected 'hello-nexus' in output, got: {output:?}"
    );
    assert_eq!(exit_code, Some(0), "expected exit code 0");
}

#[tokio::test]
async fn connect_and_execute_multiple_commands() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    for i in 0..5 {
        let block_id = BlockId(i + 1);
        client.execute(&format!("echo count-{i}"), block_id);

        let (output, exit_code) = client.collect_output(block_id, Duration::from_secs(5)).await;
        assert!(
            output.contains(&format!("count-{i}")),
            "command {i}: expected 'count-{i}' in output, got: {output:?}"
        );
        assert_eq!(exit_code, Some(0));
    }
}

#[tokio::test]
async fn connect_and_verify_pong() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let client = TestClient::connect(&env).await.expect("connect failed");

    // Wait for at least one pong (ping interval is 500ms)
    nexus_test_harness::assertions::poll_until(
        "pong received",
        Duration::from_secs(5),
        || async { client.last_pong_at.load(std::sync::atomic::Ordering::Relaxed) > 0 },
    )
    .await;

    assert!(client.is_data_flowing(), "data should be flowing");
    // RTT may be 0ms for local Docker — just verify pong_at was set
    let pong = client.last_pong_at.load(std::sync::atomic::Ordering::Relaxed);
    assert!(pong > 0, "last_pong_at should be set after pong");
}
