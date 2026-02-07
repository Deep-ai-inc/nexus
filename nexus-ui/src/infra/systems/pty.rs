//! PTY (pseudo-terminal) subscription for handling external commands.

use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use nexus_api::BlockId;

use crate::data::PtyEvent;

/// Async subscription that awaits PTY events, then drains all remaining
/// pending events into a single batch.  This coalesces rapid output
/// (e.g. `top` refreshes) into one message → one `update()` → one
/// `view()` cycle, preventing FPS from spiking above the display rate.
pub fn pty_subscription(
    rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
) -> strata::Subscription<Vec<(BlockId, PtyEvent)>> {
    // 2ms delay coalesces a full `top` refresh (~3-4 chunks) while staying
    // well below perceptible latency. Max 256 events per batch.
    strata::shell::subscription::from_receiver_batched(rx, 2, 256)
}
