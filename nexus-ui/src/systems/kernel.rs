//! Kernel subscription for handling native command events.

use std::sync::Arc;

use iced::futures::stream;
use iced::Subscription;
use tokio::sync::{broadcast, Mutex};

use nexus_api::ShellEvent;

/// Async subscription that awaits kernel events.
/// Returns raw ShellEvent for caller to map to messages.
pub fn kernel_subscription(
    rx: Arc<Mutex<broadcast::Receiver<ShellEvent>>>,
) -> Subscription<ShellEvent> {
    struct KernelSubscription;

    Subscription::run_with_id(
        std::any::TypeId::of::<KernelSubscription>(),
        stream::unfold(rx, |rx| async move {
            loop {
                let result = {
                    let mut guard = rx.lock().await;
                    guard.recv().await
                };

                match result {
                    Ok(shell_event) => return Some((shell_event, rx)),
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
        }),
    )
}
