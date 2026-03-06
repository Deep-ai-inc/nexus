//! Event bridge: reads `Response` frames from the remote agent and injects
//! `ShellEvent`s into the local broadcast channel.
//!
//! This is the key integration point: the UI doesn't care where ShellEvents
//! come from. Remote events flow through the same channel as local ones.
//!
//! Non-event responses (ClassifyResult, CompleteResult, etc.) are forwarded
//! via a bounded channel to RemoteBackend's response handler.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use nexus_api::ShellEvent;
use nexus_protocol::codec::FrameReader;
use nexus_protocol::messages::{Request, Response};
use tokio::io::AsyncRead;
use tokio::sync::{broadcast, mpsc, Mutex};

use super::RequestEnvelope;

/// Credit replenishment threshold: send a new grant after consuming this many bytes.
const REPLENISH_THRESHOLD: u64 = 128 * 1024;
/// Credit grant size.
const GRANT_SIZE: u64 = 256 * 1024;

/// Run the event bridge loop: read responses from the remote agent and
/// route ShellEvents to the local broadcast channel.
///
/// Non-event responses are forwarded via `response_tx` for RemoteBackend.
/// RTT is computed from Pong responses and stored in `rtt_ms`.
/// The last seen event sequence number is tracked in `last_seen_seq`.
/// Flow control credits are replenished via `request_tx` after consuming data.
pub(crate) async fn run<R: AsyncRead + Unpin>(
    mut reader: FrameReader<R>,
    kernel_tx: broadcast::Sender<ShellEvent>,
    response_tx: mpsc::UnboundedSender<Response>,
    request_tx: mpsc::UnboundedSender<RequestEnvelope>,
    ping_timestamps: Arc<Mutex<HashMap<u64, Instant>>>,
    rtt_ms: Arc<AtomicU64>,
    last_seen_seq: Arc<AtomicU64>,
) {
    let mut bytes_since_grant: u64 = 0;

    loop {
        let response: Response = match reader.read().await {
            Ok(resp) => resp,
            Err(nexus_protocol::codec::CodecError::ConnectionClosed) => {
                tracing::info!("remote agent connection closed");
                break;
            }
            Err(e) => {
                tracing::error!("error reading from remote agent: {e}");
                break;
            }
        };

        // Estimate frame size for flow control accounting
        let frame_size = nexus_protocol::codec::encode_payload(&response)
            .map(|v| v.len() as u64)
            .unwrap_or(256);
        bytes_since_grant += frame_size;

        // Replenish credits if threshold crossed
        if bytes_since_grant >= REPLENISH_THRESHOLD {
            bytes_since_grant = 0;
            let _ = request_tx.send(RequestEnvelope {
                request: Request::GrantCredits {
                    bytes: GRANT_SIZE,
                },
            });
        }

        match response {
            Response::Event { seq, event } => {
                // Seq dedup: skip events already seen (safety net for resume replay)
                let prev = last_seen_seq.load(Ordering::Relaxed);
                if prev > 0 && seq <= prev {
                    continue;
                }
                last_seen_seq.store(seq, Ordering::Relaxed);
                let _ = kernel_tx.send(event);
            }
            Response::Pong { seq } => {
                // Compute RTT from stored ping timestamp
                let mut timestamps = ping_timestamps.lock().await;
                if let Some(sent_at) = timestamps.remove(&seq) {
                    let elapsed = sent_at.elapsed().as_millis() as u64;
                    rtt_ms.store(elapsed, Ordering::Relaxed);
                }
            }
            Response::ChildLost {
                reason,
                surviving_env,
            } => {
                tracing::warn!("remote child lost: {reason}");
                // Forward to response handler for UI notification
                let _ = response_tx.send(Response::ChildLost {
                    reason,
                    surviving_env,
                });
            }
            other => {
                // Non-event responses (ClassifyResult, CompleteResult, etc.)
                // Forward to RemoteBackend's response handler.
                if response_tx.send(other).is_err() {
                    tracing::debug!("response channel closed, stopping event bridge");
                    break;
                }
            }
        }
    }
    // Dropping response_tx here signals the receiver that the bridge is gone
}
