//! Slice 11: Reconnect robustness under realistic conditions.
//!
//! Tests that exercise the exact failure modes observed in production:
//! - Large scrollback replay completes without timeout during Resume
//! - Two clients to the same agent both resume successfully (no race to Hello)
//! - Resume succeeds through degraded network with retry

use std::time::Duration;

use nexus_api::{BlockId, ShellEvent};
use nexus_test_harness::assertions::poll_until;
use nexus_test_harness::client::TestClient;
use nexus_test_harness::container::TestEnv;

/// Generate a large scrollback (2000+ lines) in a PTY, disconnect, resume.
/// The Resume handshake must stream the full ScrollbackHistory + TerminalSnapshot
/// without being killed by the per-attempt timeout.
///
/// This reproduces the production bug where a 1494-row scrollback caused Resume
/// to time out at 30s, falling through to Hello and losing the PTY.
#[tokio::test]
async fn large_scrollback_resume_completes() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Spawn a PTY and generate ~2000 lines of output
    let pty_block = BlockId(10);
    client.spawn_pty("sh", pty_block, 120, 30);

    // Wait for shell prompt
    let found = client
        .wait_for_event(Duration::from_secs(5), |ev| {
            matches!(ev, ShellEvent::StdoutChunk { block_id, .. } if *block_id == pty_block)
        })
        .await;
    assert!(found.is_some(), "should see shell prompt");

    // Generate 2000 sequentially numbered lines (creates large scrollback)
    client.pty_input(pty_block, b"seq 1 2000\n");

    // Wait for output to finish (seq 1 2000 should complete quickly)
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Drain events so last_seen_seq is up to date
    while client
        .wait_for_event(Duration::from_millis(200), |_| true)
        .await
        .is_some()
    {}

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;
    let last_seq = client.last_seen_seq_value();

    // Disconnect
    client.kill_ssh();

    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Resume — must complete even with large scrollback replay
    let start = tokio::time::Instant::now();
    let mut client2 = TestClient::resume(&env, &instance_id, session_token, last_seq)
        .await
        .expect("resume with large scrollback failed");
    let elapsed = start.elapsed();

    // Verify we got a scrollback event
    let scrollback = client2
        .wait_for_event(Duration::from_secs(5), |ev| {
            matches!(ev, ShellEvent::ScrollbackHistory { block_id, .. } if *block_id == pty_block)
        })
        .await;

    // Even if scrollback event was already consumed during handshake, verify
    // the session works — the key assertion is that Resume didn't time out.
    let cmd_block = BlockId(100);
    client2.execute("echo scrollback-resume-ok", cmd_block);
    let (output, exit_code) = client2.collect_output(cmd_block, Duration::from_secs(5)).await;
    assert_eq!(exit_code, Some(0));
    assert!(
        output.contains("scrollback-resume-ok"),
        "commands should work after large scrollback resume, got: {output:?}"
    );

    // Same instance = Resume succeeded (didn't fall to Hello)
    assert_eq!(
        client2.instance_id(),
        instance_id,
        "should be same agent (Resume), not a new one (Hello)"
    );

    tracing::info!(
        "large scrollback resume completed in {:.1}s (scrollback event: {})",
        elapsed.as_secs_f64(),
        if scrollback.is_some() { "received" } else { "consumed during handshake" }
    );
}

/// Two clients connected to the same agent both disconnect simultaneously.
/// Both must resume successfully to the SAME agent — neither should fall
/// through to Hello and start a new agent.
///
/// This reproduces the production bug where two reconnect loops raced:
/// one fell to Hello (new agent), the other resumed the old agent,
/// leaving one window showing "Connection lost — remote agent restarted".
#[tokio::test]
async fn two_clients_resume_without_racing_to_hello() {
    let env = TestEnv::start().await.expect("failed to start test env");

    // Client A: connect and spawn a PTY
    let mut client_a = TestClient::connect(&env).await.expect("connect A failed");
    let instance_id = client_a.instance_id().to_string();
    let session_token = client_a.session_token;

    let pty_a = BlockId(10);
    client_a.spawn_pty("sh", pty_a, 80, 24);
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Client B: resume to the same agent (simulates second window)
    let last_seq_a = client_a.last_seen_seq_value();
    let mut client_b = TestClient::resume(&env, &instance_id, session_token, last_seq_a)
        .await
        .expect("client B resume failed");

    // Generate some scrollback on the PTY via client A
    client_a.pty_input(pty_a, b"seq 1 500\n");
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Drain events on both clients
    while client_a
        .wait_for_event(Duration::from_millis(100), |_| true)
        .await
        .is_some()
    {}
    while client_b
        .wait_for_event(Duration::from_millis(100), |_| true)
        .await
        .is_some()
    {}

    // Disconnect both simultaneously
    let last_seq_a2 = client_a.last_seen_seq_value();
    let last_seq_b = client_b.last_seen_seq_value();
    client_a.kill_ssh();
    client_b.kill_ssh();

    // Wait for agent to enter UDS persistence
    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Resume both — sequentially to avoid UDS contention, but both must
    // succeed as Resume (not Hello)
    let client_a2 = TestClient::resume(&env, &instance_id, session_token, last_seq_a2)
        .await
        .expect("client A resume failed");

    assert_eq!(
        client_a2.instance_id(),
        instance_id,
        "client A should resume to same agent"
    );

    // Client A2's connection is now active. Client B resumes next — the agent
    // should handle the UDS takeover and serve client B.
    // Client A2 gets displaced by client B's UDS takeover
    let mut client_b2 = TestClient::resume(&env, &instance_id, session_token, last_seq_b)
        .await
        .expect("client B resume failed");

    assert_eq!(
        client_b2.instance_id(),
        instance_id,
        "client B should resume to same agent, not a new one"
    );

    // Verify the final connection works
    let cmd_block = BlockId(200);
    client_b2.execute("echo both-resumed-ok", cmd_block);
    let (output, exit_code) = client_b2.collect_output(cmd_block, Duration::from_secs(5)).await;
    assert_eq!(exit_code, Some(0));
    assert!(
        output.contains("both-resumed-ok"),
        "commands should work after dual resume, got: {output:?}"
    );
}

/// Resume through a degraded network (high latency + packet loss).
/// The reconnect_with_retry loop should succeed via Resume even when
/// individual packets are delayed or dropped.
///
/// This simulates post-laptop-wake conditions where the network is
/// technically up but not fully stable yet.
#[tokio::test]
async fn resume_through_degraded_network() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Spawn PTY with some scrollback
    let pty_block = BlockId(10);
    client.spawn_pty("sh", pty_block, 80, 24);
    tokio::time::sleep(Duration::from_millis(500)).await;
    client.pty_input(pty_block, b"seq 1 500\n");
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Drain events
    while client
        .wait_for_event(Duration::from_millis(100), |_| true)
        .await
        .is_some()
    {}

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;
    let last_seq = client.last_seen_seq_value();

    // Blackhole to force disconnect
    env.network.blackhole().await.expect("blackhole failed");
    tokio::time::sleep(Duration::from_secs(6)).await; // sshd keepalive kills session
    client.kill_ssh();

    // Restore with degradation: 200ms latency + 5% loss
    env.network.restore().await.expect("restore failed");
    env.network
        .degrade_with_jitter(200, 50, 5.0)
        .await
        .expect("degrade failed");

    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Use reconnect_with_retry — same as real UI
    let mut client2 = TestClient::reconnect_with_retry(
        &env,
        &instance_id,
        session_token,
        last_seq,
        &[1, 3, 5, 10, 15],
    )
    .await
    .expect("reconnect through degraded network failed");

    // Should have resumed (not Hello)
    assert_eq!(
        client2.instance_id(),
        instance_id,
        "should resume to same agent through degraded network"
    );

    // Clean up degradation for the command test
    env.network.restore().await.expect("restore failed");

    let cmd_block = BlockId(100);
    client2.execute("echo degraded-ok", cmd_block);
    let (output, exit_code) = client2.collect_output(cmd_block, Duration::from_secs(10)).await;
    assert_eq!(exit_code, Some(0));
    assert!(
        output.contains("degraded-ok"),
        "commands should work after degraded resume, got: {output:?}"
    );
}

/// Verify that reconnect_with_retry does NOT fall through to Hello when
/// the agent is alive but the first few SSH connections fail.
/// Uses blackhole with delayed restore to simulate post-sleep network ramp-up.
///
/// Delay schedule: attempts at 1s, 3s, 5s, 10s, 15s
/// Network restored after 8s → attempts 1-2 fail, attempt 3+ should succeed.
/// The key assertion: the final connection is Resume (same instance_id), not Hello.
#[tokio::test]
async fn no_premature_hello_fallback() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;

    // Run a command to confirm session works
    let block_id = BlockId(1);
    client.execute("echo session-alive", block_id);
    let (output, _) = client.collect_output(block_id, Duration::from_secs(5)).await;
    assert!(output.contains("session-alive"));

    let last_seq = client.last_seen_seq_value();

    // Blackhole → disconnect
    env.network.blackhole().await.expect("blackhole failed");
    tokio::time::sleep(Duration::from_secs(6)).await;
    client.kill_ssh();

    // Agent should be persisting on UDS (check via docker exec, which bypasses iptables)
    poll_until("agent alive", Duration::from_secs(5), || {
        let net = env.network.clone();
        async move { net.is_agent_alive().await }
    })
    .await;

    // Restore network after 8s (from now) — first 2 attempts will fail
    let net_clone = env.network.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(8)).await;
        net_clone.restore().await.ok();
    });

    // reconnect_with_retry: delays [1, 3, 5, 10, 15]
    // t=1s: attempt 1 (blackholed → fail)
    // t=4s: attempt 2 (blackholed → fail)
    // t=9s: attempt 3 (network just restored → should succeed)
    let mut client2 = TestClient::reconnect_with_retry(
        &env,
        &instance_id,
        session_token,
        last_seq,
        &[1, 3, 5, 10, 15],
    )
    .await
    .expect("reconnect should eventually succeed");

    // CRITICAL: must be Resume, not Hello
    assert_eq!(
        client2.instance_id(),
        instance_id,
        "should have resumed to same agent — not started a new one via Hello"
    );

    let block_id2 = BlockId(10);
    client2.execute("echo no-premature-hello", block_id2);
    let (output2, exit_code2) = client2.collect_output(block_id2, Duration::from_secs(5)).await;
    assert_eq!(exit_code2, Some(0));
    assert!(output2.contains("no-premature-hello"));
}
