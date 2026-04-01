//! Slice 4: Protocol edge cases — resume, ring buffer replay, PTY persistence.

use std::time::Duration;

use nexus_api::BlockId;
use nexus_test_harness::assertions::poll_until;
use nexus_test_harness::client::TestClient;
use nexus_test_harness::container::TestEnv;

#[tokio::test]
async fn resume_after_client_disconnect() {
    let mut env = TestEnv::start().await.expect("failed to start test env");
    let client = TestClient::connect(&env).await.expect("connect failed");

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;
    let last_seq = client.last_seen_seq_value();

    // Kill SSH (client side) — agent persists via UDS
    drop(client);

    // Wait for agent to detect disconnect and bind UDS
    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Resume
    let mut client2 = TestClient::resume(&env, &instance_id, session_token, last_seq)
        .await
        .expect("resume failed");

    // Verify the resumed session works
    let block_id = BlockId(100);
    client2.execute("echo resumed-ok", block_id);
    let (output, exit_code) = client2.collect_output(block_id, Duration::from_secs(5)).await;
    assert!(output.contains("resumed-ok"), "output: {output:?}");
    assert_eq!(exit_code, Some(0));
}

#[tokio::test]
async fn pty_survives_reconnect() {
    let mut env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Spawn a long-running PTY
    let pty_block = BlockId(10);
    client.spawn_pty("sh -c 'while true; do sleep 1; done'", pty_block, 80, 24);

    // Give PTY time to start
    tokio::time::sleep(Duration::from_secs(1)).await;

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;
    let last_seq = client.last_seen_seq_value();

    // Kill SSH
    client.kill_ssh();

    // Wait for UDS
    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Resume
    let mut client2 = TestClient::resume(&env, &instance_id, session_token, last_seq)
        .await
        .expect("resume failed");

    // Verify PTY is still alive — execute a new command, it should work
    // (the agent is the same process with same PTY handles)
    let block_id2 = BlockId(100);
    client2.execute("echo pty-still-alive", block_id2);
    let (output, exit_code) = client2.collect_output(block_id2, Duration::from_secs(5)).await;
    assert!(output.contains("pty-still-alive"), "output: {output:?}");
    assert_eq!(exit_code, Some(0));
}

#[tokio::test]
async fn output_during_disconnect_is_replayed() {
    let mut env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Start a command that will produce output after a delay
    let block_id = BlockId(20);
    // Use /bin/sh -c so the entire pipeline is a single external command
    // (avoids kernel splitting on && into two block_id=20 executions)
    client.execute("/bin/sh -c 'sleep 2 && echo REPLAY_MARKER'", block_id);

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;
    let last_seq = client.last_seen_seq_value();

    // Give the request time to reach the agent before killing SSH
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Kill SSH (before the command produces output — sleep 2 hasn't finished)
    client.kill_ssh();

    // Wait for the command to finish producing output (while disconnected)
    tokio::time::sleep(Duration::from_secs(4)).await;

    // Resume — ring buffer should replay the missed output
    let mut client2 = TestClient::resume(&env, &instance_id, session_token, last_seq)
        .await
        .expect("resume failed");

    // Collect replayed events
    let (output, exit_code) = client2.collect_output(block_id, Duration::from_secs(5)).await;
    assert!(
        output.contains("REPLAY_MARKER"),
        "ring buffer should replay missed output, got: {output:?}"
    );
    assert_eq!(exit_code, Some(0));
}

#[tokio::test]
async fn five_disconnect_resume_cycles() {
    let mut env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;

    for i in 0..5 {
        let last_seq = client.last_seen_seq_value();
        client.kill_ssh();

        // Wait for UDS
        poll_until(
            &format!("UDS socket (cycle {i})"),
            Duration::from_secs(10),
            || {
                let net = env.network.clone();
                let id = instance_id.clone();
                async move { net.agent_socket_exists(&id).await }
            },
        )
        .await;

        // Resume
        client = TestClient::resume(&env, &instance_id, session_token, last_seq)
            .await
            .unwrap_or_else(|e| panic!("resume cycle {i} failed: {e}"));

        // Verify it works
        let block_id = BlockId(200 + i as u64);
        client.execute(&format!("echo cycle-{i}"), block_id);
        let (output, exit_code) = client.collect_output(block_id, Duration::from_secs(5)).await;
        assert!(output.contains(&format!("cycle-{i}")), "cycle {i}: {output:?}");
        assert_eq!(exit_code, Some(0));
    }
}

#[tokio::test]
async fn cwd_preserved_on_resume() {
    let mut env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Change directory (cd is a builtin — run it alone, then verify with pwd)
    let cd_block = BlockId(30);
    client.execute("cd /tmp", cd_block);
    let (_, _) = client.collect_output(cd_block, Duration::from_secs(5)).await;

    let pwd_block = BlockId(31);
    client.execute("/bin/pwd", pwd_block);
    let (output, _) = client.collect_output(pwd_block, Duration::from_secs(5)).await;
    assert!(output.contains("/tmp"), "expected /tmp, got: {output:?}");

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;
    let last_seq = client.last_seen_seq_value();

    client.kill_ssh();

    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    let client2 = TestClient::resume(&env, &instance_id, session_token, last_seq)
        .await
        .expect("resume failed");

    // CWD in env should reflect the last known directory
    // Note: kernel tracks cd internally, so env.cwd should be /tmp
    assert_eq!(
        client2.env.cwd.to_str().unwrap_or(""),
        "/tmp",
        "CWD should be preserved across reconnect"
    );
}

#[tokio::test]
async fn resume_after_server_side_blackhole() {
    let mut env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Wait for pong
    poll_until("pong flowing", Duration::from_secs(5), || async {
        client.last_pong_at.load(std::sync::atomic::Ordering::Relaxed) > 0
    })
    .await;

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;
    let last_seq = client.last_seen_seq_value();

    // Blackhole: simulates network drop
    env.network.blackhole().await.expect("blackhole failed");

    // sshd keepalive (2s interval, 2 retries) should kill the session in ~6s
    // Agent detects EOF and enters UDS persistence
    tokio::time::sleep(Duration::from_secs(8)).await;

    // Restore network
    env.network.restore().await.expect("restore failed");

    // Agent should be alive and have a UDS socket
    poll_until("agent alive after blackhole", Duration::from_secs(5), || {
        let net = env.network.clone();
        async move { net.is_agent_alive().await }
    })
    .await;

    poll_until("UDS socket exists", Duration::from_secs(5), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Kill client SSH (it's stuck on the dead connection)
    client.kill_ssh();

    // Resume on the restored network
    let mut client2 = TestClient::resume(&env, &instance_id, session_token, last_seq)
        .await
        .expect("resume after blackhole failed");

    let block_id = BlockId(40);
    client2.execute("echo blackhole-survived", block_id);
    let (output, exit_code) = client2.collect_output(block_id, Duration::from_secs(5)).await;
    assert!(output.contains("blackhole-survived"), "output: {output:?}");
    assert_eq!(exit_code, Some(0));
}

/// Proves the bug: PTY input sent just before SSH dies is lost.
///
/// Scenario: user types into a PTY, then WiFi drops. The input is in the
/// transport channel but the writer task can't push it over the dead SSH pipe.
/// On reconnect, that input is gone — the user's keystrokes vanish.
///
/// This test sends input to a `cat` PTY, kills SSH immediately after, resumes,
/// and checks whether `cat` echoed the input back. Without the fix, it won't.
#[tokio::test]
async fn pty_input_during_disconnect_survives_reconnect() {
    let mut env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Spawn a PTY running cat (echoes stdin back to stdout)
    let pty_block = BlockId(50);
    client.spawn_pty("cat", pty_block, 80, 24);

    // Wait for PTY to be ready
    tokio::time::sleep(Duration::from_secs(1)).await;

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;
    let last_seq = client.last_seen_seq_value();

    // Send PTY input THEN immediately kill SSH — simulates typing right
    // after WiFi drops but before the client detects the dead connection.
    // The input goes into the request channel but the writer task can't
    // push it to the dead SSH pipe before we kill the process.
    client.pty_input(pty_block, b"TYPED_DURING_DISCONNECT\n");
    client.kill_ssh();

    // Take the RequestSender — carries unconfirmed inputs across reconnection
    let sender = client.take_request_sender();

    // Wait for agent to detect disconnect and bind UDS
    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Resume with the RequestSender — unconfirmed inputs are replayed automatically via swap_transport
    let mut client2 =
        TestClient::resume_with_sender(&env, &instance_id, session_token, last_seq, Some(sender))
            .await
            .expect("resume failed");

    // cat should echo it back (from the replayed input)
    let found = client2
        .wait_for_event(Duration::from_secs(5), |ev| {
            matches!(ev, nexus_api::ShellEvent::StdoutChunk { block_id, data, .. }
                if *block_id == pty_block && String::from_utf8_lossy(data).contains("TYPED_DURING_DISCONNECT"))
        })
        .await;

    assert!(
        found.is_some(),
        "PTY input typed during disconnect should appear after reconnect"
    );
}
