//! Slice 8: Lifecycle & security edge cases.
//!
//! Tests agent process death/cleanup, stale socket sweep, and nested
//! agent chain recovery.

use std::time::Duration;

use nexus_api::BlockId;
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
