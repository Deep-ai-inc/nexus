//! Subscription helpers for async event streams.
//!
//! Provides utilities for creating subscriptions from async channels.
//! These are polled by the native backend on a timer.

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, Mutex};

use crate::app::SubscriptionStream;

// ============================================================================
// Receiver-based subscription (single events)
// ============================================================================

/// Wrapper around an unbounded receiver that implements SubscriptionStream.
#[allow(dead_code)]
struct ReceiverStream<T> {
    rx: Arc<Mutex<mpsc::UnboundedReceiver<T>>>,
}

impl<T: Send + 'static> SubscriptionStream for ReceiverStream<T> {
    type Item = T;

    fn try_recv(&mut self) -> Option<T> {
        // Try to receive without blocking using try_lock + try_recv.
        let mut guard = self.rx.try_lock().ok()?;
        guard.try_recv().ok()
    }
}

/// Create a subscription from an unbounded receiver.
///
/// Events are delivered one at a time as they arrive.
pub fn from_receiver<T: Send + 'static>(
    rx: Arc<Mutex<mpsc::UnboundedReceiver<T>>>,
) -> crate::Subscription<T> {
    crate::Subscription {
        streams: vec![Box::new(ReceiverStream { rx })],
    }
}

// ============================================================================
// Batched receiver subscription
// ============================================================================

/// Wrapper that batches events from an unbounded receiver.
#[allow(dead_code)]
struct BatchedReceiverStream<T> {
    rx: Arc<Mutex<mpsc::UnboundedReceiver<T>>>,
    max_batch: usize,
}

impl<T: Send + 'static> SubscriptionStream for BatchedReceiverStream<T> {
    type Item = Vec<T>;

    fn try_recv(&mut self) -> Option<Vec<T>> {
        let mut guard = self.rx.try_lock().ok()?;

        // Try to get at least one event.
        let first = guard.try_recv().ok()?;
        let mut batch = vec![first];

        // Drain remaining up to max_batch.
        while batch.len() < self.max_batch {
            match guard.try_recv() {
                Ok(evt) => batch.push(evt),
                Err(_) => break,
            }
        }

        Some(batch)
    }
}

/// Create a batching subscription from an unbounded receiver.
///
/// Waits for at least one event, then drains remaining pending events
/// into a batch. This coalesces rapid updates into fewer messages.
///
/// - `_batch_delay_ms`: Ignored (reserved for future use)
/// - `max_batch`: Maximum events per batch to prevent stalls
pub fn from_receiver_batched<T: Send + 'static>(
    rx: Arc<Mutex<mpsc::UnboundedReceiver<T>>>,
    _batch_delay_ms: u64,
    max_batch: usize,
) -> crate::Subscription<Vec<T>> {
    crate::Subscription {
        streams: vec![Box::new(BatchedReceiverStream { rx, max_batch })],
    }
}

// ============================================================================
// Broadcast receiver subscription
// ============================================================================

/// Wrapper around a broadcast receiver that implements SubscriptionStream.
#[allow(dead_code)]
struct BroadcastStream<T> {
    rx: Arc<Mutex<broadcast::Receiver<T>>>,
}

impl<T: Clone + Send + 'static> SubscriptionStream for BroadcastStream<T> {
    type Item = T;

    fn try_recv(&mut self) -> Option<T> {
        let mut guard = self.rx.try_lock().ok()?;
        loop {
            match guard.try_recv() {
                Ok(event) => return Some(event),
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    // Skip dropped messages.
                    continue;
                }
                Err(_) => return None,
            }
        }
    }
}

/// Create a subscription from a broadcast receiver.
///
/// Handles lagged receivers gracefully by skipping dropped messages.
pub fn from_broadcast<T: Clone + Send + 'static>(
    rx: Arc<Mutex<broadcast::Receiver<T>>>,
) -> crate::Subscription<T> {
    crate::Subscription {
        streams: vec![Box::new(BroadcastStream { rx })],
    }
}
