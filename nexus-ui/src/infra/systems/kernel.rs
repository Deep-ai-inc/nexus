//! Kernel subscription for handling native command events.

use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};

use nexus_api::ShellEvent;

/// Async subscription that awaits kernel events.
/// Returns raw ShellEvent for caller to map to messages.
pub fn kernel_subscription(
    rx: Arc<Mutex<broadcast::Receiver<ShellEvent>>>,
) -> strata::Subscription<ShellEvent> {
    strata::shell::subscription::from_broadcast(rx)
}
