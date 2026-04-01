//! Polling assertion helpers — ban hardcoded sleeps.
//!
//! All assertions poll with short intervals until a condition is met or a
//! timeout expires. This avoids flaky tests from fixed sleep durations.

use std::future::Future;
use std::time::Duration;

/// Poll a condition until it returns true, or panic after timeout.
///
/// ```ignore
/// poll_until("agent socket exists", Duration::from_secs(10), || async {
///     env.network.agent_socket_exists("abc123").await
/// }).await;
/// ```
pub async fn poll_until<F, Fut>(description: &str, timeout: Duration, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    let mut interval = tokio::time::interval(Duration::from_millis(200));

    loop {
        interval.tick().await;

        if check().await {
            return;
        }

        if tokio::time::Instant::now() > deadline {
            panic!(
                "Timed out after {}s waiting for: {description}",
                timeout.as_secs()
            );
        }
    }
}

/// Poll until a condition returns `Some(T)`, or panic after timeout.
pub async fn poll_for<F, Fut, T>(description: &str, timeout: Duration, mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    let mut interval = tokio::time::interval(Duration::from_millis(200));

    loop {
        interval.tick().await;

        if let Some(value) = check().await {
            return value;
        }

        if tokio::time::Instant::now() > deadline {
            panic!(
                "Timed out after {}s waiting for: {description}",
                timeout.as_secs()
            );
        }
    }
}
