//! Thin test client that drives the nexus protocol directly.
//!
//! Uses `nexus_client::TransportHandle` for all transport, handshake, and
//! event bridge logic — no duplication with the real application.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use nexus_api::ShellEvent;
use nexus_client::{RequestSender, TransportHandle};
use nexus_protocol::messages::{EnvInfo, Request, Response, Transport};
use tokio::sync::{broadcast, mpsc};

use crate::container::TestEnv;

/// A connected test client.
pub struct TestClient {
    pub env: EnvInfo,
    pub session_token: [u8; 16],
    pub last_seen_seq: Arc<AtomicU64>,
    pub rtt_ms: Arc<AtomicU64>,
    pub last_pong_at: Arc<AtomicU64>,

    request_tx: RequestSender,
    last_confirmed_epoch: Arc<AtomicU64>,
    response_rx: mpsc::UnboundedReceiver<Response>,
    event_rx: broadcast::Receiver<ShellEvent>,
    kernel_tx: broadcast::Sender<ShellEvent>,

    /// SSH child process (kill this to simulate client disconnect).
    child: Option<tokio::process::Child>,

    /// For resume
    transport: Transport,
    agent_path: String,

    next_request_id: u32,
    next_echo_epoch: u64,
}

impl TestClient {
    /// Connect to the agent via SSH (fresh Hello handshake).
    pub async fn connect(env: &TestEnv) -> Result<Self> {
        let transport = env.transport();
        let agent_path = env.agent_path().to_string();
        let (kernel_tx, _) = broadcast::channel::<ShellEvent>(256);

        let (handle, env_info, session_token, request_tx) =
            TransportHandle::connect(&transport, &agent_path, HashMap::new(), kernel_tx.clone())
                .await?;

        let event_rx = kernel_tx.subscribe();

        Ok(Self {
            env: env_info,
            session_token,
            last_seen_seq: handle.last_seen_seq,
            rtt_ms: handle.rtt_ms,
            last_pong_at: handle.last_pong_at,
            request_tx,
            last_confirmed_epoch: handle.last_confirmed_epoch,
            response_rx: handle.response_rx,
            event_rx,
            kernel_tx,
            child: Some(handle.child),
            transport,
            agent_path,
            next_request_id: 1,
            next_echo_epoch: 0,
        })
    }

    /// Resume an existing session (Resume handshake via --attach).
    pub async fn resume(
        env: &TestEnv,
        instance_id: &str,
        session_token: [u8; 16],
        last_seen_seq: u64,
    ) -> Result<Self> {
        Self::resume_with_sender(env, instance_id, session_token, last_seen_seq, None).await
    }

    /// Resume with an optional RequestSender carried over from a previous client.
    /// If provided, unconfirmed PTY inputs are replayed on the new transport via swap_transport.
    pub async fn resume_with_sender(
        env: &TestEnv,
        instance_id: &str,
        session_token: [u8; 16],
        last_seen_seq: u64,
        mut prev_sender: Option<RequestSender>,
    ) -> Result<Self> {
        let transport = env.transport();
        let agent_path = env.agent_path().to_string();
        let (kernel_tx, _) = broadcast::channel::<ShellEvent>(256);
        // Subscribe before resume so we capture replay events sent during handshake
        let event_rx = kernel_tx.subscribe();

        let (handle, env_info, new_request_tx) = TransportHandle::resume(
            &transport,
            &agent_path,
            Some(instance_id),
            session_token,
            last_seen_seq,
            80,
            24,
            kernel_tx.clone(),
        )
        .await?;

        // If we have a previous sender with buffered inputs, swap transport to replay them
        let request_tx = if let Some(ref mut sender) = prev_sender {
            let confirmed = handle.last_confirmed_epoch.load(Ordering::Relaxed);
            sender.swap_transport(new_request_tx.into_inner(), confirmed);
            prev_sender.take().unwrap()
        } else {
            new_request_tx
        };

        Ok(Self {
            env: env_info,
            session_token,
            last_seen_seq: handle.last_seen_seq,
            rtt_ms: handle.rtt_ms,
            last_pong_at: handle.last_pong_at,
            request_tx,
            last_confirmed_epoch: handle.last_confirmed_epoch,
            response_rx: handle.response_rx,
            event_rx,
            kernel_tx,
            child: Some(handle.child),
            transport,
            agent_path,
            next_request_id: 1,
            next_echo_epoch: 0,
        })
    }

    /// Reconnect with retry using the shared reconnection loop.
    ///
    /// Uses the same Resume→Hello fallback + exponential backoff as the real UI.
    /// `delays` controls the retry schedule (seconds per attempt).
    pub async fn reconnect_with_retry(
        env: &TestEnv,
        instance_id: &str,
        session_token: [u8; 16],
        last_seen_seq: u64,
        delays: &[u64],
    ) -> Result<Self> {
        let transport = env.transport();
        let agent_path = env.agent_path().to_string();
        let (kernel_tx, _) = broadcast::channel::<ShellEvent>(256);
        let event_rx = kernel_tx.subscribe();

        let params = nexus_client::ReconnectParams {
            transport: transport.clone(),
            agent_path: agent_path.clone(),
            instance_id: instance_id.to_string(),
            session_token,
            last_seen_seq,
            cols: 80,
            rows: 24,
            kernel_tx: kernel_tx.clone(),
            attempt_counter: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            cancel: tokio_util::sync::CancellationToken::new(),
            forwarded_env: HashMap::new(),
        };

        let outcome = nexus_client::reconnect::reconnect_loop_with_delays(params, delays)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let (handle, env_info, request_tx, session_token) = match outcome {
            nexus_client::ReconnectOutcome::Resumed {
                handle,
                env,
                request_tx,
            } => (handle, env, request_tx, session_token),
            nexus_client::ReconnectOutcome::FreshConnect {
                handle,
                env,
                session_token: new_token,
                request_tx,
            } => (handle, env, request_tx, new_token),
        };

        Ok(Self {
            env: env_info,
            session_token,
            last_seen_seq: handle.last_seen_seq,
            rtt_ms: handle.rtt_ms,
            last_pong_at: handle.last_pong_at,
            request_tx,
            last_confirmed_epoch: handle.last_confirmed_epoch,
            response_rx: handle.response_rx,
            event_rx,
            kernel_tx,
            child: Some(handle.child),
            transport,
            agent_path,
            next_request_id: 1,
            next_echo_epoch: 0,
        })
    }

    /// Take the RequestSender (for carrying across reconnections).
    /// The sender holds buffered unconfirmed inputs that will be replayed on swap_transport.
    pub fn take_request_sender(&mut self) -> RequestSender {
        // Replace with a dummy sender (channel will be immediately closed, but that's fine
        // since this client is about to be dropped)
        let (dummy_tx, _) = mpsc::unbounded_channel();
        std::mem::replace(&mut self.request_tx, RequestSender::new(dummy_tx))
    }

    /// Kill the SSH child process (simulates client-side disconnect).
    pub fn kill_ssh(&mut self) {
        if let Some(ref mut child) = self.child {
            let _ = child.start_kill();
        }
        self.child = None;
    }

    /// Get the agent's instance ID.
    pub fn instance_id(&self) -> &str {
        &self.env.instance_id
    }

    /// Get the last seen sequence number.
    pub fn last_seen_seq_value(&self) -> u64 {
        self.last_seen_seq.load(Ordering::Relaxed)
    }

    /// Check if pong data is flowing (mirrors RemoteBackend::is_data_flowing).
    pub fn is_data_flowing(&self) -> bool {
        let last = self.last_pong_at.load(Ordering::Relaxed);
        if last == 0 {
            return true;
        }
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now_ms.saturating_sub(last) < 10_000
    }

    fn next_id(&mut self) -> u32 {
        let id = self.next_request_id;
        self.next_request_id += 1;
        id
    }

    /// Send a request to the agent.
    pub fn send(&mut self, request: Request) {
        self.request_tx.send(request);
    }

    /// Execute a command and return the block ID.
    pub fn execute(&mut self, command: &str, block_id: nexus_api::BlockId) {
        let id = self.next_id();
        self.send(Request::Execute {
            id,
            command: command.to_string(),
            block_id,
        });
    }

    /// Spawn a PTY.
    pub fn spawn_pty(
        &mut self,
        command: &str,
        block_id: nexus_api::BlockId,
        cols: u16,
        rows: u16,
    ) {
        let id = self.next_id();
        self.send(Request::PtySpawn {
            id,
            command: command.to_string(),
            block_id,
            cols,
            rows,
            term: "xterm-256color".to_string(),
            cwd: "/home/testuser".to_string(),
        });
    }

    /// Send input to a remote PTY (buffered for reconnect replay via RequestSender).
    pub fn pty_input(&mut self, block_id: nexus_api::BlockId, data: &[u8]) {
        self.next_echo_epoch += 1;
        let epoch = self.next_echo_epoch;
        self.request_tx.send(Request::PtyInput {
            block_id,
            data: data.to_vec(),
            echo_epoch: epoch,
        });
    }

    /// Wait for a specific event, with timeout.
    pub async fn wait_for_event<F>(
        &mut self,
        timeout: Duration,
        pred: F,
    ) -> Option<ShellEvent>
    where
        F: Fn(&ShellEvent) -> bool,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }

            match tokio::time::timeout(remaining, self.event_rx.recv()).await {
                Ok(Ok(event)) if pred(&event) => return Some(event),
                Ok(Ok(_)) => continue, // not the event we want
                Ok(Err(_)) => return None, // channel closed
                Err(_) => return None, // timeout
            }
        }
    }

    /// Collect all stdout output for a block until CommandFinished or timeout.
    pub async fn collect_output(
        &mut self,
        block_id: nexus_api::BlockId,
        timeout: Duration,
    ) -> (String, Option<i32>) {
        let mut output = Vec::new();
        let mut exit_code = None;
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            match tokio::time::timeout(remaining, self.event_rx.recv()).await {
                Ok(Ok(ShellEvent::StdoutChunk {
                    block_id: bid,
                    data,
                    ..
                })) if bid == block_id => {
                    output.extend_from_slice(&data);
                }
                Ok(Ok(ShellEvent::StderrChunk {
                    block_id: bid,
                    data,
                    ..
                })) if bid == block_id => {
                    output.extend_from_slice(&data);
                }
                Ok(Ok(ShellEvent::CommandOutput {
                    block_id: bid,
                    value,
                })) if bid == block_id => {
                    // Builtin commands emit structured Value, not raw bytes
                    output.extend_from_slice(format!("{value}").as_bytes());
                }
                Ok(Ok(ShellEvent::CommandFinished {
                    block_id: bid,
                    exit_code: code,
                    ..
                })) if bid == block_id => {
                    exit_code = Some(code);
                    break;
                }
                Ok(Ok(_)) => continue,
                Ok(Err(_)) | Err(_) => break,
            }
        }

        (String::from_utf8_lossy(&output).to_string(), exit_code)
    }

    /// Wait for the next non-event response (ClassifyResult, Error, etc).
    pub async fn recv_response(&mut self, timeout: Duration) -> Option<Response> {
        tokio::time::timeout(timeout, self.response_rx.recv())
            .await
            .ok()
            .flatten()
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        self.kill_ssh();
    }
}
