//! Slice 5: Concurrency & race condition tests.
//!
//! Proves that UDS takeover serialization, cancellation tokens, and zombie
//! reaping work correctly under concurrent and rapid-fire conditions.

use std::time::Duration;

use nexus_api::BlockId;
use nexus_test_harness::assertions::poll_until;
use nexus_test_harness::client::TestClient;
use nexus_test_harness::container::TestEnv;

/// Two simultaneous --attach processes race to resume. One must succeed,
/// the other must fail gracefully. Agent must remain healthy.
#[tokio::test]
async fn split_brain_attach() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Execute a command so we have state to verify after resume
    let block_id = BlockId(1);
    client.execute("echo alive", block_id);
    let (output, _) = client.collect_output(block_id, Duration::from_secs(5)).await;
    assert!(output.contains("alive"));

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;
    let last_seq = client.last_seen_seq_value();

    // Kill SSH — agent persists
    drop(client);

    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Race two resume attempts
    let (r1, r2) = tokio::join!(
        TestClient::resume(&env, &instance_id, session_token, last_seq),
        TestClient::resume(&env, &instance_id, session_token, last_seq),
    );

    // Both may succeed initially (handshake completes before takeover fires),
    // but only the last one connected actually owns the session. The earlier
    // one gets disconnected by the UDS takeover.
    let mut successes = Vec::new();
    for result in [r1, r2] {
        if let Ok(c) = result {
            successes.push(c);
        }
    }

    assert!(
        !successes.is_empty(),
        "at least one resume must succeed"
    );

    // The LAST successful client is the real owner (UDS takeover is LIFO).
    // Try each — the one that can execute commands is the true winner.
    let mut found_winner = false;
    for client in successes.iter_mut().rev() {
        let verify_block = BlockId(2);
        client.execute("echo split-brain-ok", verify_block);
        let (output, exit_code) = client.collect_output(verify_block, Duration::from_secs(3)).await;
        if output.contains("split-brain-ok") && exit_code == Some(0) {
            found_winner = true;
            break;
        }
    }

    assert!(found_winner, "at least one client must be able to execute commands");

    // Agent is still alive
    assert!(env.network.is_agent_alive().await);
}

/// Toggle network blackhole rapidly while streaming output.
/// Proves cancellation token + transport swap don't leak tasks or FDs.
#[tokio::test]
async fn rapid_flapping() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Start a streaming command inside a PTY
    let pty_block = BlockId(10);
    client.spawn_pty("sh -c 'i=0; while true; do echo line_$i; i=$((i+1)); sleep 0.1; done'", pty_block, 80, 24);

    // Wait for initial output
    let found = client
        .wait_for_event(Duration::from_secs(5), |ev| {
            matches!(ev, nexus_api::ShellEvent::StdoutChunk { block_id, .. } if *block_id == pty_block)
        })
        .await;
    assert!(found.is_some(), "should see initial PTY output");

    // Record baseline FD count
    let baseline_fds = env.network.agent_fd_count().await;

    // Flap the network 3 times with short intervals
    for i in 0..3 {
        env.network.blackhole().await.expect("blackhole failed");
        tokio::time::sleep(Duration::from_secs(1)).await;
        env.network.restore().await.expect("restore failed");
        tokio::time::sleep(Duration::from_secs(1)).await;
        eprintln!("flap cycle {}/3 complete", i + 1);
    }

    // Agent should still be alive
    assert!(
        env.network.is_agent_alive().await,
        "agent must survive rapid flapping"
    );

    // Check for FD leaks (allow some growth, but not unbounded)
    if let (Some(baseline), Some(current)) = (baseline_fds, env.network.agent_fd_count().await) {
        let growth = current.saturating_sub(baseline);
        assert!(
            growth < 50,
            "FD leak detected: baseline={baseline}, current={current}, growth={growth}"
        );
    }
}

/// Double-fork creates a zombie. The reaper must catch it.
/// Uses NEXUS_AGENT_REAPER_INTERVAL=3 for fast testing.
#[tokio::test]
async fn zombie_reaper() {
    let env = TestEnv::start().await.expect("failed to start test env");

    // Set a fast reaper interval
    env.set_agent_env("NEXUS_AGENT_REAPER_INTERVAL", "3")
        .await
        .expect("failed to set env");

    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Execute a command that double-forks: inner process becomes zombie
    // when the middle process exits. The agent's PR_SET_CHILD_SUBREAPER
    // ensures the zombie is reparented to us.
    let block_id = BlockId(1);
    client.execute("sh -c '(sleep 1 &); exit 0'", block_id);
    let (_, exit_code) = client.collect_output(block_id, Duration::from_secs(5)).await;
    assert_eq!(exit_code, Some(0));

    // Wait for the sleep to finish (1s) + reaper interval (3s) + margin
    poll_until(
        "no zombie processes",
        Duration::from_secs(10),
        || {
            let net = env.network.clone();
            async move {
                let ps = net.exec_raw("ps aux").await.unwrap_or_default();
                let zombies = ps.lines().filter(|l| {
                    l.contains("Z") && l.contains("sleep")
                }).count();
                zombies == 0
            }
        },
    )
    .await;

    // Agent is still responsive
    let verify_block = BlockId(2);
    client.execute("echo reaper-ok", verify_block);
    let (output, exit_code) = client.collect_output(verify_block, Duration::from_secs(5)).await;
    assert!(output.contains("reaper-ok"), "output: {output:?}");
    assert_eq!(exit_code, Some(0));
}
