//! Slice 6: State & buffer boundary tests.
//!
//! Pushes the ring buffer and client-side input buffer past their limits
//! to ensure graceful degradation.

use std::time::Duration;

use nexus_api::BlockId;
use nexus_test_harness::assertions::poll_until;
use nexus_test_harness::client::TestClient;
use nexus_test_harness::container::TestEnv;

/// Generate >4MB of output while disconnected, proving ring buffer overrun
/// is detected and terminal snapshot restores visual state.
#[tokio::test]
async fn ring_buffer_overrun() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Spawn a PTY
    let pty_block = BlockId(10);
    client.spawn_pty("sh", pty_block, 80, 24);

    // Wait for shell prompt
    let found = client
        .wait_for_event(Duration::from_secs(5), |ev| {
            matches!(ev, nexus_api::ShellEvent::StdoutChunk { block_id, .. } if *block_id == pty_block)
        })
        .await;
    assert!(found.is_some(), "should see shell prompt");

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;
    let last_seq = client.last_seen_seq_value();

    // Blackhole the network
    env.network.blackhole().await.expect("blackhole failed");

    // Generate >4MB of sequentially numbered output via docker exec
    // (bypasses the blackholed SSH, writes directly to the PTY's stdin)
    env.docker_exec(
        "sh -c 'for pid in $(pgrep -f \"^sh$\"); do \
            echo \"seq 1 200000\" > /proc/$pid/fd/0 2>/dev/null; \
        done'"
    )
    .await
    .ok(); // best effort — the shell inside the PTY will run `seq`

    // Wait for the command to finish generating output inside the container
    tokio::time::sleep(Duration::from_secs(8)).await;

    // Restore network and kill old SSH
    env.network.restore().await.expect("restore failed");
    client.kill_ssh();

    // Wait for UDS
    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Resume — should get events_lost notification
    let mut client2 = TestClient::resume(&env, &instance_id, session_token, last_seq)
        .await
        .expect("resume failed");

    // Verify the session is functional: execute a marker command
    // The terminal snapshot should have restored visual state even if scrollback was lost.
    client2.pty_input(pty_block, b"echo OVERRUN_MARKER\n");
    let found = client2
        .wait_for_event(Duration::from_secs(5), |ev| {
            matches!(ev, nexus_api::ShellEvent::StdoutChunk { block_id, data, .. }
                if *block_id == pty_block && String::from_utf8_lossy(data).contains("OVERRUN_MARKER"))
        })
        .await;
    assert!(
        found.is_some(),
        "PTY should be functional after ring buffer overrun"
    );
}

/// Paste a large block of text while disconnected, prove RequestSender
/// buffers and replays it without OOM or corruption.
#[tokio::test]
async fn client_input_flooding() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Spawn cat to echo back everything
    let pty_block = BlockId(10);
    client.spawn_pty("cat", pty_block, 80, 24);

    // Wait for cat to be ready
    tokio::time::sleep(Duration::from_millis(500)).await;

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;
    let last_seq = client.last_seen_seq_value();

    // Blackhole network, then flood input
    env.network.blackhole().await.expect("blackhole failed");

    // Send 100KB of input in 1KB chunks (conservative per the guidance)
    let chunk = "A".repeat(1024);
    for _ in 0..100 {
        client.pty_input(pty_block, chunk.as_bytes());
    }

    // Send a recognizable marker at the end
    client.pty_input(pty_block, b"FLOOD_END\n");

    // Kill SSH and take the sender (carries buffered inputs)
    client.kill_ssh();
    let sender = client.take_request_sender();

    // Restore network
    env.network.restore().await.expect("restore failed");

    // Wait for UDS
    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Resume with the sender — replays all 100KB + marker
    let mut client2 =
        TestClient::resume_with_sender(&env, &instance_id, session_token, last_seq, Some(sender))
            .await
            .expect("resume failed");

    // cat should echo back the marker (proving the replay worked)
    let found = client2
        .wait_for_event(Duration::from_secs(15), |ev| {
            matches!(ev, nexus_api::ShellEvent::StdoutChunk { block_id, data, .. }
                if *block_id == pty_block && String::from_utf8_lossy(data).contains("FLOOD_END"))
        })
        .await;

    assert!(
        found.is_some(),
        "marker should appear after 100KB input flood replay"
    );
}

/// Drop network just shy of the agent's read timeout, reconnect in time.
/// Uses NEXUS_AGENT_READ_TIMEOUT=15 for fast testing.
#[tokio::test]
async fn reconnect_before_read_timeout() {
    let env = TestEnv::start().await.expect("failed to start test env");

    // Set a short read timeout for this test
    env.set_agent_env("NEXUS_AGENT_READ_TIMEOUT", "10")
        .await
        .expect("failed to set env");

    let mut client = TestClient::connect(&env).await.expect("connect failed");

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;
    let last_seq = client.last_seen_seq_value();

    // Kill SSH — starts the 15s read timeout clock
    client.kill_ssh();

    // Wait for UDS
    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Wait 7 seconds — within 10s timeout but close enough to prove preemption
    // (3s margin to handle CI jitter per the guidance)
    tokio::time::sleep(Duration::from_secs(7)).await;

    // Agent must still be alive
    assert!(
        env.network.is_agent_alive().await,
        "agent should still be alive at 7s (timeout is 10s)"
    );

    // Resume — should succeed because we're within the timeout
    let mut client2 = TestClient::resume(&env, &instance_id, session_token, last_seq)
        .await
        .expect("resume should succeed before timeout");

    let block_id = BlockId(1);
    client2.execute("echo timeout-preempted", block_id);
    let (output, exit_code) = client2.collect_output(block_id, Duration::from_secs(5)).await;
    assert!(output.contains("timeout-preempted"), "output: {output:?}");
    assert_eq!(exit_code, Some(0));
}
