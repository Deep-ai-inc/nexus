//! Slice 3: Chaos engine — verify network manipulation works before using
//! it in protocol tests. Prove the chaos works first.

use std::time::Duration;

use nexus_api::BlockId;
use nexus_test_harness::assertions::poll_until;
use nexus_test_harness::client::TestClient;
use nexus_test_harness::container::TestEnv;

#[tokio::test]
async fn blackhole_causes_command_timeout() {
    let mut env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Verify connection works first
    let block_id = BlockId(1);
    client.execute("echo before-blackhole", block_id);
    let (output, _) = client.collect_output(block_id, Duration::from_secs(5)).await;
    assert!(output.contains("before-blackhole"));

    // Blackhole the network
    env.network.blackhole().await.expect("blackhole failed");

    // New command should timeout (no response possible)
    let block_id2 = BlockId(2);
    client.execute("echo after-blackhole", block_id2);
    let (output, exit_code) = client.collect_output(block_id2, Duration::from_secs(3)).await;
    assert!(
        exit_code.is_none(),
        "command should not complete during blackhole"
    );
    assert!(
        !output.contains("after-blackhole"),
        "should not receive output during blackhole"
    );

    // Restore and verify agent is still running
    env.network.restore().await.expect("restore failed");
    assert!(env.network.is_agent_alive().await, "agent should survive blackhole");
}

#[tokio::test]
async fn kill_sshd_session_triggers_disconnect() {
    let mut env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Verify pong is flowing
    poll_until("pong flowing", Duration::from_secs(5), || async {
        client.last_pong_at.load(std::sync::atomic::Ordering::Relaxed) > 0
    })
    .await;

    // Kill the sshd session from the server side
    env.network.kill_sshd_session().await.expect("kill failed");

    // Pongs should stop flowing
    poll_until("data stops flowing", Duration::from_secs(15), || async {
        !client.is_data_flowing()
    })
    .await;
}

#[tokio::test]
async fn agent_persists_after_ssh_death() {
    let mut env = TestEnv::start().await.expect("failed to start test env");
    let client = TestClient::connect(&env).await.expect("connect failed");
    let instance_id = client.instance_id().to_string();

    // Kill SSH from client side
    drop(client);

    // Agent should persist via UDS
    poll_until("agent still alive", Duration::from_secs(10), || {
        let net = env.network.clone();
        async move { net.is_agent_alive().await }
    })
    .await;

    // UDS socket should exist
    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;
}
