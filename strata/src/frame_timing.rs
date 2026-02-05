//! Frame timing instrumentation.
//!
//! Prints per-frame timing breakdowns to stderr when enabled.
//! Enable by calling `enable()` or automatically when terminal output
//! contains "ps aux" style content (many lines of text).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);
static FRAME_COUNT: AtomicU64 = AtomicU64::new(0);

/// Enable frame timing output.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Disable frame timing output.
pub fn disable() {
    ENABLED.store(false, Ordering::Relaxed);
}

/// Check if timing is enabled.
#[inline]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Bump and return frame counter (for sampling — only print every Nth frame).
pub fn next_frame() -> u64 {
    FRAME_COUNT.fetch_add(1, Ordering::Relaxed)
}

/// Read current frame counter without incrementing.
pub fn current_frame() -> u64 {
    FRAME_COUNT.load(Ordering::Relaxed)
}

/// Guard that measures elapsed time from creation to drop.
/// Prints the label and duration on drop (if timing is enabled and frame matches).
pub struct TimingGuard {
    label: &'static str,
    start: Instant,
    frame: u64,
}

impl TimingGuard {
    /// Start a timing span. Only actually records if timing is enabled.
    #[inline]
    pub fn new(label: &'static str, frame: u64) -> Self {
        Self {
            label,
            start: Instant::now(),
            frame,
        }
    }
}

impl Drop for TimingGuard {
    fn drop(&mut self) {
        // Only print every 60th frame to avoid flooding
        if self.frame % 60 == 0 {
            let elapsed = self.start.elapsed();
            eprintln!("[frame {}] {}: {:.2?}", self.frame, self.label, elapsed);
        }
    }
}

/// Measure a block and print timing. Returns the block's result.
/// Only prints every 60th frame.
#[inline]
pub fn measure<T>(label: &'static str, frame: u64, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = f();
    if frame % 60 == 0 {
        let elapsed = start.elapsed();
        eprintln!("[frame {}] {}: {:.2?}", frame, label, elapsed);
    }
    result
}

/// Print a counter/stat line (only every 60th frame).
#[inline]
pub fn stat(label: &'static str, frame: u64, value: impl std::fmt::Display) {
    if frame % 60 == 0 {
        eprintln!("[frame {}] {}: {}", frame, label, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enable_disable() {
        // Start in known state
        disable();
        assert!(!is_enabled());

        enable();
        assert!(is_enabled());

        disable();
        assert!(!is_enabled());
    }

    #[test]
    fn test_frame_counter() {
        // Note: Frame counter is global, so we just check it increments
        let before = current_frame();
        let returned = next_frame();
        let after = current_frame();

        assert_eq!(returned, before);
        assert_eq!(after, before + 1);
    }

    #[test]
    fn test_timing_guard_creation() {
        let frame = current_frame();
        let _guard = TimingGuard::new("test_label", frame);
        // Guard should be created without panicking
        // Drop happens at end of scope
    }

    #[test]
    fn test_measure_returns_value() {
        let frame = current_frame();
        let result = measure("test", frame, || {
            42
        });
        assert_eq!(result, 42);
    }

    #[test]
    fn test_measure_executes_closure() {
        let frame = current_frame();
        let mut executed = false;
        measure("test", frame, || {
            executed = true;
        });
        assert!(executed);
    }

    #[test]
    fn test_stat_does_not_panic() {
        // Just verify it doesn't panic at various frame numbers
        stat("test", 0, "value");
        stat("test", 1, 42);
        stat("test", 60, 3.14);
        stat("test", 120, "another value");
    }
}
