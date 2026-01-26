//! PTY (pseudo-terminal) subscription for handling external commands.

use std::sync::Arc;

use iced::futures::stream;
use iced::Subscription;
use tokio::sync::{mpsc, Mutex};

use nexus_api::BlockId;

use crate::blocks::PtyEvent;

/// Async subscription that awaits PTY events instead of polling.
/// Returns raw (BlockId, PtyEvent) tuples for caller to map to messages.
pub fn pty_subscription(
    rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
) -> Subscription<(BlockId, PtyEvent)> {
    struct PtySubscription;

    Subscription::run_with_id(
        std::any::TypeId::of::<PtySubscription>(),
        stream::unfold(rx, |rx| async move {
            let event = {
                let mut guard = rx.lock().await;
                guard.recv().await
            };

            event.map(|e| (e, rx))
        }),
    )
}
