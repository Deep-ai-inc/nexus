//! Slice 10: Reconnection edge cases.
//!
//! Tests that exercise the reconnect loop's Resume→Hello fallback logic
//! and verify commands work after each reconnection mode.

use std::time::Duration;

use nexus_api::BlockId;
use nexus_test_harness::assertions::poll_until;
use nexus_test_harness::client::TestClient;
use nexus_test_harness::container::TestEnv;

/// After a fresh Hello reconnect (agent "restarted" from the client's
/// perspective), commands must still work. This catches the case where
/// the UI shows a green status dot but input is silently dropped.
#[tokio::test]
async fn commands_work_after_fresh_hello_reconnect() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Run a command to prove the first session works
    let block_id = BlockId(1);
    client.execute("echo first-session", block_id);
    let (output, exit_code) = client.collect_output(block_id, Duration::from_secs(5)).await;
    assert_eq!(exit_code, Some(0));
    assert!(output.contains("first-session"));

    // Kill SSH — agent persists
    client.kill_ssh();

    let instance_id = client.instance_id().to_string();
    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Do a FRESH Hello connect (not Resume) — simulates what happens when
    // Resume fails and the reconnect loop falls back to Hello.
    let mut client2 = TestClient::connect(&env).await.expect("fresh connect failed");

    // This is a different agent instance (fresh Hello starts a new agent).
    // Commands must work on this new connection.
    let block_id2 = BlockId(10);
    client2.execute("echo fresh-session-works", block_id2);
    let (output2, exit_code2) = client2.collect_output(block_id2, Duration::from_secs(5)).await;
    assert_eq!(exit_code2, Some(0));
    assert!(
        output2.contains("fresh-session-works"),
        "commands should work after fresh Hello reconnect, got: {output2:?}"
    );
}

/// Simulate the laptop-sleep scenario: the agent survives multiple
/// disconnect/resume cycles. Each time the client reconnects via Resume,
/// the session and commands should work.
///
/// This proves that if the reconnect loop retries Resume (instead of
/// falling through to Hello), it will succeed — the agent is still alive.
#[tokio::test]
async fn resume_survives_multiple_disconnect_cycles() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;

    // Spawn a PTY to have persistent state
    let block_id = BlockId(1);
    client.spawn_pty("/bin/sh", block_id, 80, 24);
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Disconnect
    let last_seq = client.last_seen_seq_value();
    client.kill_ssh();

    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Resume #1
    let mut client2 = TestClient::resume(&env, &instance_id, session_token, last_seq)
        .await
        .expect("resume #1 failed");

    let block_id2 = BlockId(2);
    client2.execute("echo resume-one-ok", block_id2);
    let (output, exit_code) = client2.collect_output(block_id2, Duration::from_secs(5)).await;
    assert_eq!(exit_code, Some(0));
    assert!(output.contains("resume-one-ok"));

    // Disconnect again
    let last_seq2 = client2.last_seen_seq_value();
    client2.kill_ssh();

    poll_until("UDS socket still exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Resume #2 — agent should still be alive
    let mut client3 = TestClient::resume(&env, &instance_id, session_token, last_seq2)
        .await
        .expect("resume #2 failed — agent should survive multiple disconnects");

    let block_id3 = BlockId(3);
    client3.execute("echo resume-two-ok", block_id3);
    let (output3, exit_code3) = client3.collect_output(block_id3, Duration::from_secs(5)).await;
    assert_eq!(exit_code3, Some(0));
    assert!(
        output3.contains("resume-two-ok"),
        "commands should work after second resume, got: {output3:?}"
    );
}

/// Force the Hello fallback path by killing the agent process. The
/// reconnect_with_retry loop tries Resume (fails — agent is dead),
/// then falls back to Hello (starts new agent). Commands must work
/// through the new connection.
#[tokio::test]
async fn commands_work_after_reconnect_hello_fallback() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;

    let block_id = BlockId(1);
    client.execute("echo before-kill", block_id);
    let (output, _) = client.collect_output(block_id, Duration::from_secs(5)).await;
    assert!(output.contains("before-kill"));

    let last_seq = client.last_seen_seq_value();

    // Kill the agent process — Resume will fail, forcing Hello fallback
    client.kill_ssh();
    env.network.kill_agent().await.expect("kill agent failed");

    poll_until("agent is dead", Duration::from_secs(5), || {
        let net = env.network.clone();
        async move { !net.is_agent_alive().await }
    })
    .await;

    // reconnect_with_retry: Resume fails (agent dead), Hello succeeds
    let mut client2 = TestClient::reconnect_with_retry(
        &env,
        &instance_id,
        session_token,
        last_seq,
        &[1, 2, 3, 4],
    )
    .await
    .expect("reconnect_with_retry should succeed via Hello fallback");

    // Different instance_id = fresh agent
    assert_ne!(
        client2.instance_id(),
        instance_id,
        "should be a new agent instance after Hello fallback"
    );

    // Commands MUST work on the fresh connection
    let block_id2 = BlockId(10);
    client2.execute("echo hello-fallback-works", block_id2);
    let (output2, exit_code2) = client2.collect_output(block_id2, Duration::from_secs(5)).await;
    assert_eq!(exit_code2, Some(0));
    assert!(
        output2.contains("hello-fallback-works"),
        "commands must work after Hello fallback, got: {output2:?}"
    );
}

/// When Resume fails with a transport error (blackholed network), the
/// reconnect_with_retry loop should retry Resume on subsequent attempts.
/// After connectivity is restored, Resume should succeed — proving we
/// didn't prematurely fall through to Hello.
#[tokio::test]
async fn reconnect_retries_resume_after_transport_failure() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;

    let block_id = BlockId(1);
    client.execute("echo preserve-me", block_id);
    let (output, _) = client.collect_output(block_id, Duration::from_secs(5)).await;
    assert!(output.contains("preserve-me"));

    let last_seq = client.last_seen_seq_value();
    client.kill_ssh();

    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Blackhole traffic — first resume attempt will fail
    env.network.blackhole().await;

    // Restore after 3 seconds (first attempt at t=1s fails, second at t=4s succeeds)
    let net_clone = env.network.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        net_clone.restore().await;
    });

    // reconnect_with_retry: attempts at 1s, 4s, 8s
    let mut client2 = TestClient::reconnect_with_retry(
        &env,
        &instance_id,
        session_token,
        last_seq,
        &[1, 3, 5],
    )
    .await
    .expect("reconnect_with_retry failed");

    // Should have resumed to the SAME agent (not a fresh Hello)
    assert_eq!(
        client2.instance_id(),
        instance_id,
        "should have resumed to the same agent, not started a new one"
    );

    // Commands should work
    let block_id2 = BlockId(2);
    client2.execute("echo retry-resume-ok", block_id2);
    let (output2, exit_code2) = client2.collect_output(block_id2, Duration::from_secs(5)).await;
    assert_eq!(exit_code2, Some(0));
    assert!(output2.contains("retry-resume-ok"));
}
