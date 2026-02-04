//! PTY (pseudo-terminal) subscription for handling external commands.

use std::sync::Arc;

use iced::futures::stream;
use iced::Subscription;
use tokio::sync::{mpsc, Mutex};

use nexus_api::BlockId;

use crate::blocks::PtyEvent;

/// Async subscription that awaits PTY events, then drains all remaining
/// pending events into a single batch.  This coalesces rapid output
/// (e.g. `top` refreshes) into one message → one `update()` → one
/// `view()` cycle, preventing FPS from spiking above the display rate.
pub fn pty_subscription(
    rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
) -> Subscription<Vec<(BlockId, PtyEvent)>> {
    struct PtySubscription;

    Subscription::run_with_id(
        std::any::TypeId::of::<PtySubscription>(),
        stream::unfold(rx, |rx| async move {
            // Block until at least one event arrives.
            let first = {
                let mut guard = rx.lock().await;
                guard.recv().await
            }?;

            // Sleep briefly to let the PTY reader thread (which is an OS
            // thread, not a tokio task) push more chunks into the channel.
            // Without this, try_recv() fires nanoseconds after recv() and
            // finds nothing — defeating the batching.  2ms is enough to
            // coalesce a full `top` refresh (~3-4 chunks) into one batch
            // while staying well below perceptible latency.
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;

            // Drain remaining pending events, capped to avoid a single
            // giant batch that stalls the update loop.
            const MAX_BATCH: usize = 256;
            let mut batch = vec![first];
            {
                let mut guard = rx.lock().await;
                while batch.len() < MAX_BATCH {
                    match guard.try_recv() {
                        Ok(evt) => batch.push(evt),
                        Err(_) => break,
                    }
                }
            }

            Some((batch, rx))
        }),
    )
}
