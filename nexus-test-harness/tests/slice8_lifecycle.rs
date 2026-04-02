//! Slice 8: Lifecycle & security edge cases.
//!
//! Tests agent process death/cleanup, stale socket sweep, and nested
//! agent chain recovery.

use std::time::Duration;

use nexus_api::BlockId;
use nexus_protocol::messages::Response;
use nexus_test_harness::assertions::poll_until;
use nexus_test_harness::client::TestClient;
use nexus_test_harness::container::TestEnv;

/// SIGKILL the agent so it can't clean up its UDS socket. Start a new agent
/// and prove the stale socket sweep unlinks the dead socket and binds correctly.
#[tokio::test]
async fn sigkill_stale_socket_sweep() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let client = TestClient::connect(&env).await.expect("connect failed");

    let instance_id = client.instance_id().to_string();

    // Verify agent is running and socket exists
    assert!(env.network.is_agent_alive().await);

    // Wait for UDS socket to be bound
    poll_until("UDS socket exists", Duration::from_secs(5), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Drop the client (closes SSH) then SIGKILL the agent
    drop(client);
    env.network.kill_agent().await.expect("kill agent failed");

    // Verify agent is dead
    poll_until("agent is dead", Duration::from_secs(5), || {
        let net = env.network.clone();
        async move { !net.is_agent_alive().await }
    })
    .await;

    // Stale socket should still be on disk (agent couldn't clean up)
    let stale_exists = env.network.agent_socket_exists(&instance_id).await;
    assert!(
        stale_exists,
        "stale socket should remain after SIGKILL"
    );

    // Connect a new client — this starts a fresh agent which must sweep
    // the stale socket during startup
    let mut client2 = TestClient::connect(&env).await.expect("fresh connect failed");

    // The old stale socket should be gone (swept by new agent startup)
    // Note: the new agent has a different instance_id
    let new_instance_id = client2.instance_id().to_string();
    assert_ne!(instance_id, new_instance_id);

    // New agent works
    let block_id = BlockId(1);
    client2.execute("echo sweep-ok", block_id);
    let (output, exit_code) = client2.collect_output(block_id, Duration::from_secs(5)).await;
    assert!(output.contains("sweep-ok"), "output: {output:?}");
    assert_eq!(exit_code, Some(0));

    // Old stale socket should have been cleaned up by the new agent
    let stale_still_exists = env.network.agent_socket_exists(&instance_id).await;
    assert!(
        !stale_still_exists,
        "stale socket should be swept by new agent startup"
    );
}

/// Nested agent chain: client → agent A → agent B.
/// Blackhole between client and container A, reconnect, prove the whole
/// chain survives — commands still reach agent B through agent A's relay.
#[tokio::test]
async fn nested_agent_chain_recovery() {
    let (env_a, env_b) = TestEnv::start_pair().await.expect("failed to start pair");

    // Connect client to agent A
    let mut client = TestClient::connect(&env_a).await.expect("connect to A failed");

    // Save agent A's identity BEFORE nesting (after nest, env reflects agent B)
    let instance_id_a = client.instance_id().to_string();
    let session_token = client.session_token;

    // Get container B's internal IP for nesting
    let b_ip = env_b.container_ip().await.expect("failed to get B IP");

    // Send Nest request: agent A SSHes to agent B
    let nest_transport = nexus_protocol::Transport::Ssh {
        destination: format!("testuser@{b_ip}"),
        port: None,
        identity: Some("/home/testuser/.ssh/id_test".to_string()),
        extra_args: vec![
            "-o".to_string(), "StrictHostKeyChecking=no".to_string(),
            "-o".to_string(), "UserKnownHostsFile=/dev/null".to_string(),
            "-o".to_string(), "LogLevel=ERROR".to_string(),
        ],
    };
    let nest_id = client.nest(nest_transport);

    // Wait for NestOk response
    let resp = client
        .recv_response(Duration::from_secs(30))
        .await
        .expect("no NestOk response");
    match resp {
        Response::NestOk { id, .. } => assert_eq!(id, nest_id),
        Response::Error { message, .. } => panic!("nest failed: {message}"),
        other => panic!("unexpected response: {other:?}"),
    }

    // Now in relay mode — commands go through A to B.
    // Verify by running a command that succeeds through the chain.
    let block_id = BlockId(100);
    client.execute("echo nested-ok", block_id);
    let (output, exit_code) = client.collect_output(block_id, Duration::from_secs(10)).await;
    assert_eq!(exit_code, Some(0));
    assert!(output.contains("nested-ok"), "relay command failed: {output:?}");

    // Save last_seen_seq for resume (instance_id and token saved before nesting)
    let last_seq = client.last_seen_seq_value();

    // Kill SSH (client-side disconnect) — agent A persists with active relay to B
    client.kill_ssh();

    // Wait for agent A's UDS
    poll_until("agent A UDS socket exists", Duration::from_secs(10), || {
        let net = env_a.network.clone();
        let id = instance_id_a.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Resume to agent A
    let mut client2 = TestClient::resume(&env_a, &instance_id_a, session_token, last_seq)
        .await
        .expect("resume to A failed");

    // The relay to B should still be alive — execute a command through the chain
    let block_id2 = BlockId(200);
    client2.execute("echo nested-chain-survived", block_id2);
    let (output2, exit_code2) = client2.collect_output(block_id2, Duration::from_secs(10)).await;
    assert!(
        output2.contains("nested-chain-survived"),
        "nested chain should survive reconnect, output: {output2:?}"
    );
    assert_eq!(exit_code2, Some(0));
}
