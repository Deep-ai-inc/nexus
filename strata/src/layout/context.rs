//! Layout context for debug tracing and resource sharing.
//!
//! The LayoutContext carries state through the layout tree:
//! - The LayoutSnapshot to write primitives to
//! - Debug path tracking (only in debug builds)
//! - Reusable scratch buffers for flex allocation
//!
//! Performance: All debug fields are `#[cfg(debug_assertions)]` so they
//! are completely compiled out in release builds.

use crate::layout_snapshot::LayoutSnapshot;
use super::constraints::LayoutConstraints;
use crate::primitives::Size;

/// Flex allocation result for a single child.
#[derive(Debug, Clone, Copy, Default)]
pub struct FlexAllocation {
    /// Allocated size on main axis
    pub main_size: f32,
    /// Whether this child is flex (vs fixed)
    pub is_flex: bool,
}

/// Layout context passed through the widget tree.
///
/// In release builds, this is essentially just a wrapper around `&mut LayoutSnapshot`
/// with a scratch Vec for flex allocations.
///
/// In debug builds, it also tracks the path through the widget tree for
/// debugging layout issues.
pub struct LayoutContext<'a> {
    /// Snapshot to write layout results to
    pub snapshot: &'a mut LayoutSnapshot,

    /// Reusable scratch buffer for flex allocations (avoids per-container alloc)
    pub flex_scratch: Vec<FlexAllocation>,

    // Debug-only fields (compiled out in release)
    #[cfg(debug_assertions)]
    depth: u32,

    #[cfg(debug_assertions)]
    current_name: &'static str,

    #[cfg(debug_assertions)]
    debug_enabled: bool,

    #[cfg(debug_assertions)]
    warnings: Vec<LayoutWarning>,
}

/// A layout warning (debug builds only).
#[cfg(debug_assertions)]
#[derive(Debug, Clone)]
pub struct LayoutWarning {
    pub depth: u32,
    pub container: &'static str,
    pub message: String,
}

impl<'a> LayoutContext<'a> {
    /// Create a new layout context.
    pub fn new(snapshot: &'a mut LayoutSnapshot) -> Self {
        Self {
            snapshot,
            flex_scratch: Vec::with_capacity(32), // Pre-allocate for typical UI depth
            #[cfg(debug_assertions)]
            depth: 0,
            #[cfg(debug_assertions)]
            current_name: "Root",
            #[cfg(debug_assertions)]
            debug_enabled: false,
            #[cfg(debug_assertions)]
            warnings: Vec::new(),
        }
    }

    /// Enable debug logging (debug builds only, no-op in release).
    #[cfg(debug_assertions)]
    pub fn with_debug(mut self, enabled: bool) -> Self {
        self.debug_enabled = enabled;
        self
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn with_debug(self, _enabled: bool) -> Self {
        self
    }

    /// Enter a child scope (for path tracking in debug builds).
    #[cfg(debug_assertions)]
    pub fn enter(&mut self, name: &'static str) {
        self.depth += 1;
        self.current_name = name;
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn enter(&mut self, _name: &'static str) {}

    /// Exit the current scope.
    #[cfg(debug_assertions)]
    pub fn exit(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn exit(&mut self) {}

    /// Log a layout decision (debug builds only).
    #[cfg(debug_assertions)]
    pub fn log_layout(&self, constraints: LayoutConstraints, result_size: Size) {
        if self.debug_enabled {
            let indent = "  ".repeat(self.depth as usize);
            eprintln!(
                "[LAYOUT] {}{} | {{w:{:.0}-{:.0}, h:{:.0}-{:.0}}} -> {{w:{:.0}, h:{:.0}}}",
                indent,
                self.current_name,
                constraints.min_width,
                constraints.max_width,
                constraints.min_height,
                constraints.max_height,
                result_size.width,
                result_size.height,
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn log_layout(&self, _constraints: LayoutConstraints, _result_size: Size) {}

    /// Warn when a shrink container produces oversized output.
    #[cfg(debug_assertions)]
    pub fn warn_oversized(&mut self, container: &'static str, actual: f32, max: f32, axis: &str) {
        if actual > max && max.is_finite() {
            let warning = LayoutWarning {
                depth: self.depth,
                container,
                message: format!(
                    "{} produced {:.0} {} but only {:.0} available",
                    container, actual, axis, max
                ),
            };
            if self.debug_enabled {
                let indent = "  ".repeat(self.depth as usize);
                eprintln!("[LAYOUT WARNING] {}{}", indent, warning.message);
            }
            self.warnings.push(warning);
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn warn_oversized(&mut self, _container: &'static str, _actual: f32, _max: f32, _axis: &str) {}

    /// Take collected warnings (debug builds only).
    #[cfg(debug_assertions)]
    pub fn take_warnings(&mut self) -> Vec<LayoutWarning> {
        std::mem::take(&mut self.warnings)
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn take_warnings(&mut self) -> Vec<()> {
        Vec::new()
    }

    /// Check if debug mode is enabled.
    #[cfg(debug_assertions)]
    pub fn is_debug(&self) -> bool {
        self.debug_enabled
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn is_debug(&self) -> bool {
        false
    }
}

/// Macro for layout logging (compiled out in release).
///
/// Usage:
/// ```ignore
/// layout_log!(ctx, "Column calculated height: {}", height);
/// ```
#[macro_export]
macro_rules! layout_log {
    ($ctx:expr, $($arg:tt)*) => {
        #[cfg(debug_assertions)]
        if $ctx.is_debug() {
            eprintln!("[LAYOUT] {}", format!($($arg)*));
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_snapshot::LayoutSnapshot;

    #[test]
    fn test_context_creation() {
        let mut snapshot = LayoutSnapshot::new();
        let ctx = LayoutContext::new(&mut snapshot);
        assert!(!ctx.is_debug());
    }

    #[test]
    fn test_flex_scratch_reuse() {
        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        // Simulate flex allocation
        ctx.flex_scratch.push(FlexAllocation { main_size: 50.0, is_flex: false });
        ctx.flex_scratch.push(FlexAllocation { main_size: 50.0, is_flex: true });
        assert_eq!(ctx.flex_scratch.len(), 2);

        // Clear and reuse
        ctx.flex_scratch.clear();
        ctx.flex_scratch.push(FlexAllocation { main_size: 100.0, is_flex: false });
        assert_eq!(ctx.flex_scratch.len(), 1);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_debug_mode() {
        let mut snapshot = LayoutSnapshot::new();
        let ctx = LayoutContext::new(&mut snapshot).with_debug(true);
        assert!(ctx.is_debug());
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_enter_exit() {
        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        assert_eq!(ctx.depth, 0);
        ctx.enter("Column");
        assert_eq!(ctx.depth, 1);
        assert_eq!(ctx.current_name, "Column");
        ctx.enter("Row");
        assert_eq!(ctx.depth, 2);
        ctx.exit();
        assert_eq!(ctx.depth, 1);
        ctx.exit();
        assert_eq!(ctx.depth, 0);
    }
}
