//! Reconnection logic with exponential backoff.
//!
//! Two-phase approach per attempt:
//!   1. Try `Resume` with the stored session token — if the agent is still alive
//!      (e.g. running as a daemon or the UDS is intact), we seamlessly continue
//!      without killing orphan blocks or clearing the nesting stack.
//!   2. If Resume fails (agent restarted, token invalid, server rebooted), fall
//!      back to a fresh `Hello` handshake — this triggers orphan cleanup and
//!      stack clearing on the caller's side.
//!
//! Shared by both the GUI and the integration test harness.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use nexus_api::ShellEvent;
use nexus_protocol::messages::{EnvInfo, Transport};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::{RequestSender, TransportHandle};

/// Outcome of a successful reconnection attempt.
pub enum ReconnectOutcome {
    /// Session was resumed seamlessly (agent was still alive).
    /// The ring buffer replay has already been forwarded to `kernel_tx`.
    Resumed {
        handle: TransportHandle,
        env: EnvInfo,
        request_tx: RequestSender,
    },
    /// Fresh connection established (agent had restarted).
    /// The caller should perform orphan cleanup and stack clearing.
    FreshConnect {
        handle: TransportHandle,
        env: EnvInfo,
        session_token: [u8; 16],
        request_tx: RequestSender,
    },
}

/// Parameters for the reconnection loop.
pub struct ReconnectParams {
    pub transport: Transport,
    pub agent_path: String,
    pub instance_id: String,
    pub session_token: [u8; 16],
    pub last_seen_seq: u64,
    pub cols: u16,
    pub rows: u16,
    pub kernel_tx: broadcast::Sender<ShellEvent>,
    /// Shared counter so the caller can observe the current attempt number.
    pub attempt_counter: Arc<AtomicUsize>,
    /// Cancel token — cancels the loop when triggered.
    pub cancel: CancellationToken,
    /// Environment variables to forward on fresh Hello (empty = none).
    pub forwarded_env: HashMap<String, String>,
}

/// Default backoff schedule: exponential then steady 15s retries (~5.5 min total).
const DEFAULT_DELAYS: &[u64] = &[
    1, 2, 4, 8, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15,
];

/// Run the reconnection loop with exponential backoff.
///
/// Returns `Ok(outcome)` on success, `Err` if all attempts fail or cancelled.
pub async fn reconnect_loop(params: ReconnectParams) -> Result<ReconnectOutcome, ReconnectError> {
    reconnect_loop_with_delays(params, DEFAULT_DELAYS).await
}

/// Run the reconnection loop with a custom delay schedule (for testing).
pub async fn reconnect_loop_with_delays(
    params: ReconnectParams,
    delays: &[u64],
) -> Result<ReconnectOutcome, ReconnectError> {
    let ReconnectParams {
        transport,
        agent_path,
        instance_id,
        session_token,
        last_seen_seq,
        cols,
        rows,
        kernel_tx,
        attempt_counter,
        cancel,
        forwarded_env,
    } = params;

    for (attempt, &delay_secs) in delays.iter().enumerate() {
        // Wait before attempt (cancellable)
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(delay_secs)) => {}
            _ = cancel.cancelled() => {
                tracing::info!("reconnect cancelled");
                return Err(ReconnectError::Cancelled);
            }
        }

        attempt_counter.store(attempt + 1, Ordering::Relaxed);
        tracing::info!("reconnect attempt {}/{}", attempt + 1, delays.len());

        // Phase 1: Try Resume (session token still valid → seamless reconnect)
        let attach_id = if instance_id.is_empty() {
            None
        } else {
            Some(instance_id.as_str())
        };
        let resume_future = TransportHandle::resume(
            &transport,
            &agent_path,
            attach_id,
            session_token,
            last_seen_seq,
            cols,
            rows,
            kernel_tx.clone(),
        );

        let resume_result = tokio::select! {
            res = resume_future => res,
            _ = cancel.cancelled() => {
                tracing::info!("reconnect cancelled during resume attempt");
                return Err(ReconnectError::Cancelled);
            }
        };

        match resume_result {
            Ok((handle, env, request_tx)) => {
                tracing::info!("resumed session on {}@{}", env.user, env.hostname);
                return Ok(ReconnectOutcome::Resumed {
                    handle,
                    env,
                    request_tx,
                });
            }
            Err(e) => {
                tracing::info!("resume failed (expected if agent restarted): {e}");
            }
        }

        // Phase 2: Fall back to fresh Hello (agent restarted → orphan cleanup on caller side)
        let connect_future = TransportHandle::connect(
            &transport,
            &agent_path,
            forwarded_env.clone(),
            kernel_tx.clone(),
        );

        let connect_result = tokio::select! {
            res = connect_future => res,
            _ = cancel.cancelled() => {
                tracing::info!("reconnect cancelled during fresh connect attempt");
                return Err(ReconnectError::Cancelled);
            }
        };

        match connect_result {
            Ok((handle, env, new_session_token, request_tx)) => {
                tracing::info!("reconnected (fresh) to {}@{}", env.user, env.hostname);
                return Ok(ReconnectOutcome::FreshConnect {
                    handle,
                    env,
                    session_token: new_session_token,
                    request_tx,
                });
            }
            Err(e) => {
                tracing::warn!("reconnect attempt {} failed: {}", attempt + 1, e);
            }
        }
    }

    tracing::error!("reconnect failed after {} attempts", delays.len());
    Err(ReconnectError::Exhausted)
}

/// Error from the reconnection loop.
#[derive(Debug)]
pub enum ReconnectError {
    /// All retry attempts were exhausted.
    Exhausted,
    /// The loop was cancelled via the cancellation token.
    Cancelled,
}

impl std::fmt::Display for ReconnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exhausted => write!(f, "reconnect failed after all attempts"),
            Self::Cancelled => write!(f, "reconnect cancelled"),
        }
    }
}

impl std::error::Error for ReconnectError {}
