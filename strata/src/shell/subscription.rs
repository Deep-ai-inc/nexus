//! Subscription helpers for async event streams.
//!
//! Provides utilities for creating subscriptions from async channels without
//! exposing iced internals to application code.

use std::hash::Hash;
use std::sync::Arc;

use iced::futures::stream::{self, BoxStream, StreamExt};
use tokio::sync::{broadcast, mpsc, Mutex};

/// Create a subscription from an unbounded receiver.
///
/// Events are delivered one at a time as they arrive.
/// The subscription is identified by the Arc pointer for deduplication.
pub fn from_receiver<T: Send + 'static>(
    rx: Arc<Mutex<mpsc::UnboundedReceiver<T>>>,
) -> crate::Subscription<T> {
    crate::Subscription::from_iced(iced::advanced::subscription::from_recipe(
        ReceiverRecipe(rx),
    ))
}

/// Create a batching subscription from an unbounded receiver.
///
/// Waits for at least one event, then drains remaining pending events
/// into a batch. This coalesces rapid updates into fewer messages.
///
/// - `batch_delay_ms`: How long to wait after first event before draining (typically 1-2ms)
/// - `max_batch`: Maximum events per batch to prevent stalls
pub fn from_receiver_batched<T: Send + 'static>(
    rx: Arc<Mutex<mpsc::UnboundedReceiver<T>>>,
    batch_delay_ms: u64,
    max_batch: usize,
) -> crate::Subscription<Vec<T>> {
    crate::Subscription::from_iced(iced::advanced::subscription::from_recipe(
        BatchedReceiverRecipe {
            rx,
            batch_delay_ms,
            max_batch,
        },
    ))
}

// Single-event recipe
struct ReceiverRecipe<T>(Arc<Mutex<mpsc::UnboundedReceiver<T>>>);

impl<T: Send + 'static> iced::advanced::subscription::Recipe for ReceiverRecipe<T> {
    type Output = T;

    fn hash(&self, state: &mut iced::advanced::subscription::Hasher) {
        struct Marker;
        std::any::TypeId::of::<Marker>().hash(state);
        (Arc::as_ptr(&self.0) as *const ()).hash(state);
    }

    fn stream(
        self: Box<Self>,
        _input: iced::advanced::subscription::EventStream,
    ) -> BoxStream<'static, Self::Output> {
        let rx = self.0;
        stream::unfold(rx, |rx| async move {
            let event = {
                let mut guard = rx.lock().await;
                guard.recv().await
            };
            event.map(|e| (e, rx))
        })
        .boxed()
    }
}

// Batched recipe
struct BatchedReceiverRecipe<T> {
    rx: Arc<Mutex<mpsc::UnboundedReceiver<T>>>,
    batch_delay_ms: u64,
    max_batch: usize,
}

impl<T: Send + 'static> iced::advanced::subscription::Recipe for BatchedReceiverRecipe<T> {
    type Output = Vec<T>;

    fn hash(&self, state: &mut iced::advanced::subscription::Hasher) {
        struct Marker;
        std::any::TypeId::of::<Marker>().hash(state);
        (Arc::as_ptr(&self.rx) as *const ()).hash(state);
        self.batch_delay_ms.hash(state);
        self.max_batch.hash(state);
    }

    fn stream(
        self: Box<Self>,
        _input: iced::advanced::subscription::EventStream,
    ) -> BoxStream<'static, Self::Output> {
        let rx = self.rx;
        let delay_ms = self.batch_delay_ms;
        let max_batch = self.max_batch;

        stream::unfold(rx, move |rx| async move {
            // Block until at least one event arrives
            let first = {
                let mut guard = rx.lock().await;
                guard.recv().await
            }?;

            // Brief delay to let more events accumulate
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;

            // Drain pending events up to max_batch
            let mut batch = vec![first];
            {
                let mut guard = rx.lock().await;
                while batch.len() < max_batch {
                    match guard.try_recv() {
                        Ok(evt) => batch.push(evt),
                        Err(_) => break,
                    }
                }
            }

            Some((batch, rx))
        })
        .boxed()
    }
}

/// Create a subscription from a broadcast receiver.
///
/// Handles lagged receivers gracefully by skipping dropped messages.
/// The subscription is identified by the Arc pointer for deduplication.
pub fn from_broadcast<T: Clone + Send + 'static>(
    rx: Arc<Mutex<broadcast::Receiver<T>>>,
) -> crate::Subscription<T> {
    crate::Subscription::from_iced(iced::advanced::subscription::from_recipe(
        BroadcastRecipe(rx),
    ))
}

// Broadcast recipe
struct BroadcastRecipe<T>(Arc<Mutex<broadcast::Receiver<T>>>);

impl<T: Clone + Send + 'static> iced::advanced::subscription::Recipe for BroadcastRecipe<T> {
    type Output = T;

    fn hash(&self, state: &mut iced::advanced::subscription::Hasher) {
        struct Marker;
        std::any::TypeId::of::<Marker>().hash(state);
        (Arc::as_ptr(&self.0) as *const ()).hash(state);
    }

    fn stream(
        self: Box<Self>,
        _input: iced::advanced::subscription::EventStream,
    ) -> BoxStream<'static, Self::Output> {
        let rx = self.0;
        stream::unfold(rx, |rx| async move {
            loop {
                let result = {
                    let mut guard = rx.lock().await;
                    guard.recv().await
                };

                match result {
                    Ok(event) => return Some((event, rx)),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Messages were dropped due to slow receiver, continue
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Channel closed, stop subscription
                        return None;
                    }
                }
            }
        })
        .boxed()
    }
}
