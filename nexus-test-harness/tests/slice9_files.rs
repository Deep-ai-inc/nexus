//! Slice 9: File operation edge cases.
//!
//! Tests CancelFileRead memory leak and FileWrite resume integrity
//! across network interruptions.

use std::time::Duration;

use nexus_protocol::messages::{Request, Response};
use nexus_test_harness::client::TestClient;
use nexus_test_harness::container::TestEnv;

/// Issue a FileRead for a small file, wait for eof, then send CancelFileRead.
/// The cancel arrives after the read task has already completed and cleaned up.
/// Verify no leaked entry remains in the agent's cancelled_reads set.
///
/// We can't directly inspect agent internals, but we can verify the agent
/// still functions correctly by doing another FileRead with the same ID —
/// if the ID leaked into cancelled_reads, the second read would be
/// immediately cancelled (empty data, eof: true on first chunk).
#[tokio::test]
async fn cancel_after_completed_read_no_leak() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    // Create a small test file in the container
    env.docker_exec("echo 'hello world' > /home/testuser/testfile.txt")
        .await
        .expect("create file failed");

    // FileRead id=42
    client.send(Request::FileRead {
        id: 42,
        path: "/home/testuser/testfile.txt".to_string(),
        offset: 0,
        len: None,
    });

    // Collect all FileData responses until eof
    let mut got_eof = false;
    let mut file_data = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !got_eof {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            panic!("timed out waiting for FileData eof");
        }
        let resp = client.recv_response(remaining).await.expect("no response");
        match resp {
            Response::FileData { id: 42, data, eof } => {
                file_data.extend_from_slice(&data);
                got_eof = eof;
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }
    assert!(
        String::from_utf8_lossy(&file_data).contains("hello world"),
        "file data should contain expected content"
    );

    // Now send CancelFileRead AFTER the read task finished
    client.send(Request::CancelFileRead { id: 42 });

    // Small delay so the cancel is processed
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Issue another FileRead with the SAME id=42.
    // If the cancelled_reads set leaked the ID, this read would be
    // immediately cancelled (empty data, eof:true).
    client.send(Request::FileRead {
        id: 42,
        path: "/home/testuser/testfile.txt".to_string(),
        offset: 0,
        len: None,
    });

    // Collect response — should have actual file content, not empty cancelled data
    let resp = client
        .recv_response(Duration::from_secs(5))
        .await
        .expect("no response for second read");
    match resp {
        Response::FileData { id: 42, data, eof } => {
            // If leaked, data would be empty and eof=true
            assert!(
                !data.is_empty(),
                "second read should return data, not be immediately cancelled"
            );
            // Drain remaining chunks if not eof
            if !eof {
                loop {
                    let r = client
                        .recv_response(Duration::from_secs(2))
                        .await
                        .expect("no response");
                    match r {
                        Response::FileData { eof: true, .. } => break,
                        Response::FileData { .. } => continue,
                        other => panic!("unexpected: {other:?}"),
                    }
                }
            }
        }
        other => panic!("expected FileData, got: {other:?}"),
    }
}

/// Stream a 1MB file via FileWrite in 64KB chunks. Blackhole the connection
/// after ~512KB. Reconnect, resume from the last confirmed offset, and
/// finish the transfer. Verify the file hash matches.
#[tokio::test]
async fn file_write_resume_integrity() {
    let env = TestEnv::start().await.expect("failed to start test env");
    let mut client = TestClient::connect(&env).await.expect("connect failed");

    let instance_id = client.instance_id().to_string();
    let session_token = client.session_token;

    // Generate deterministic 1MB test data
    let total_size: usize = 1_048_576; // 1MB
    let chunk_size: usize = 65_536; // 64KB
    let test_data: Vec<u8> = (0..total_size).map(|i| (i % 251) as u8).collect();
    let expected_hash = sha256_hex(&test_data);

    let path = "/home/testuser/upload_test.bin";
    let mut offset: u64 = 0;
    let mut last_confirmed_offset: u64 = 0;
    let mut write_id: u32 = 100;

    // Phase 1: Write ~512KB, confirming each chunk
    while offset < (total_size / 2) as u64 {
        let end = (offset as usize + chunk_size).min(total_size);
        let data = test_data[offset as usize..end].to_vec();
        let data_len = data.len() as u64;

        client.send(Request::FileWrite {
            id: write_id,
            path: path.to_string(),
            offset,
            data,
        });

        let resp = client
            .recv_response(Duration::from_secs(5))
            .await
            .expect("no FileWriteOk");
        match resp {
            Response::FileWriteOk {
                id,
                bytes_written,
            } => {
                assert_eq!(id, write_id);
                assert_eq!(bytes_written, data_len);
                last_confirmed_offset = offset + bytes_written;
            }
            Response::Error { message, .. } => panic!("write failed: {message}"),
            other => panic!("unexpected: {other:?}"),
        }

        offset += data_len;
        write_id += 1;
    }

    assert!(
        last_confirmed_offset >= (total_size / 2) as u64,
        "should have written at least 512KB before blackhole"
    );

    // Phase 2: Kill SSH to simulate disconnect
    let last_seq = client.last_seen_seq_value();
    client.kill_ssh();

    // Wait for agent to enter persistence
    use nexus_test_harness::assertions::poll_until;
    poll_until("UDS socket exists", Duration::from_secs(10), || {
        let net = env.network.clone();
        let id = instance_id.clone();
        async move { net.agent_socket_exists(&id).await }
    })
    .await;

    // Phase 3: Reconnect and resume writing from last confirmed offset
    let mut client2 = TestClient::resume(&env, &instance_id, session_token, last_seq)
        .await
        .expect("resume failed");

    offset = last_confirmed_offset;
    while offset < total_size as u64 {
        let end = (offset as usize + chunk_size).min(total_size);
        let data = test_data[offset as usize..end].to_vec();
        let data_len = data.len() as u64;

        client2.send(Request::FileWrite {
            id: write_id,
            path: path.to_string(),
            offset,
            data,
        });

        let resp = client2
            .recv_response(Duration::from_secs(5))
            .await
            .expect("no FileWriteOk after resume");
        match resp {
            Response::FileWriteOk {
                id,
                bytes_written,
            } => {
                assert_eq!(id, write_id);
                assert_eq!(bytes_written, data_len);
            }
            Response::Error { message, .. } => panic!("write failed after resume: {message}"),
            other => panic!("unexpected: {other:?}"),
        }

        offset += data_len;
        write_id += 1;
    }

    // Phase 4: Verify file integrity via sha256sum in container
    let hash_output = env
        .docker_exec(&format!("sha256sum {path}"))
        .await
        .expect("sha256sum failed");
    let remote_hash = hash_output.split_whitespace().next().unwrap_or("");

    assert_eq!(
        remote_hash, expected_hash,
        "file hash mismatch: remote={remote_hash}, expected={expected_hash}"
    );
}

fn sha256_hex(data: &[u8]) -> String {
    use std::io::Write;
    // Simple SHA-256 using the `sha2` crate would be ideal, but to avoid
    // adding a dependency we shell out to the host's shasum.
    let mut child = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("shasum not found");
    child.stdin.take().unwrap().write_all(data).unwrap();
    let output = child.wait_with_output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}
