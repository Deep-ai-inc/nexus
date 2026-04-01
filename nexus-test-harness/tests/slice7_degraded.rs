//! Slice 7: Network degradation tests (beyond binary up/down).
//!
//! Proves the protocol handles high latency, jitter, and half-open TCP
//! connections correctly.

use std::time::Duration;

use nexus_api::BlockId;
use nexus_test_harness::assertions::poll_until;
use nexus_test_harness::client::TestClient;
use nexus_test_harness::container::TestEnv;

/// High latency + jitter: prove flow control works and SSH keepalive
/// doesn't false-trigger a disconnect.
#[tokio::test]
async fn high_latency_with_jitter() {
    let env = TestEnv::start().await.expect("failed to start test env");

    // Apply 200ms ± 100ms latency (no packet loss) — enough to stress
    // flow control without making SSH connection establishment too slow
    env.network
        .degrade_with_jitter(200, 100, 0.0)
        .await
        .expect("degrade failed");

    // Connect under degraded conditions (generous timeout)
    let mut client = TestClient::connect(&env).await.expect("connect under latency failed");

    // Execute several commands — each round-trip takes ~400ms with 200ms latency each way
    for i in 0..3 {
        let block_id = BlockId(i + 1);
        client.execute(&format!("echo latency_test_{i}"), block_id);
        let (output, exit_code) = client
            .collect_output(block_id, Duration::from_secs(15))
            .await;
        assert!(
            output.contains(&format!("latency_test_{i}")),
            "command {i} output: {output:?}"
        );
        assert_eq!(exit_code, Some(0));
    }

    // Pong should still be flowing (SSH keepalive didn't false-trigger)
    assert!(
        client.is_data_flowing(),
        "data should still be flowing under high latency"
    );

    // Restore and verify normal-speed operation
    env.network.restore().await.expect("restore failed");

    let block_id = BlockId(100);
    client.execute("echo restored", block_id);
    let (output, exit_code) = client.collect_output(block_id, Duration::from_secs(5)).await;
    assert!(output.contains("restored"), "output: {output:?}");
    assert_eq!(exit_code, Some(0));
}

/// TCP half-open: DROP packets (no FIN/RST). Proves the agent's read timeout
/// and client's keepalive detection both fire correctly.
/// Uses NEXUS_AGENT_READ_TIMEOUT=15 for fast testing.
#[tokio::test]
async fn tcp_half_open_detection() {
    let env = TestEnv::start().await.expect("failed to start test env");

    // Short read timeout so we don't wait 120s
    env.set_agent_env("NEXUS_AGENT_READ_TIMEOUT", "10")
        .await
        .expect("failed to set env");

    let mut client = TestClient::connect(&env).await.expect("connect failed");

    let instance_id = client.instance_id().to_string();

    // Blackhole uses DROP (not REJECT) — simulates dead TCP, no FIN
    env.network.blackhole().await.expect("blackhole failed");

    // Client-side: pong stops, is_data_flowing() should go false within ~10s
    poll_until(
        "client detects stale connection",
        Duration::from_secs(15),
        || {
            let flowing = client.is_data_flowing();
            async move { !flowing }
        },
    )
    .await;

    // Agent-side: read timeout (10s) should cause agent to break out of run loop.
    // It enters the persistence loop waiting on UDS. Wait a bit past the timeout.
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Restore network so we can check state
    env.network.restore().await.expect("restore failed");

    // Agent should still be alive (in persistence loop, not dead)
    assert!(
        env.network.is_agent_alive().await,
        "agent should persist after read timeout"
    );

    // UDS socket should exist
    assert!(
        env.network.agent_socket_exists(&instance_id).await,
        "UDS socket should exist after timeout"
    );
}
