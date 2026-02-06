//! Layout context for debug tracing and resource sharing.
//!
//! The LayoutContext carries state through the layout tree:
//! - The LayoutSnapshot to write primitives to
//! - Debug path tracking (only in debug builds)
//! - Reusable scratch buffers for flex allocation
//! - Layout cache for memoizing expensive containers
//!
//! Performance: All debug fields are `#[cfg(debug_assertions)]` so they
//! are completely compiled out in release builds.

use crate::layout_snapshot::LayoutSnapshot;
use super::cache::{LayoutCache, LayoutCacheKey};
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
/// with a scratch Vec for flex allocations and optional layout cache.
///
/// In debug builds, it also tracks the path through the widget tree for
/// debugging layout issues.
pub struct LayoutContext<'a> {
    /// Snapshot to write layout results to
    pub snapshot: &'a mut LayoutSnapshot,

    /// Reusable scratch buffer for flex allocations (avoids per-container alloc)
    pub flex_scratch: Vec<FlexAllocation>,

    /// Optional layout cache for memoizing expensive containers.
    /// If None, caching is disabled.
    pub cache: Option<&'a mut LayoutCache>,

    /// Path-based ID for cache keys (rolling hash of container path).
    /// Updated by enter()/exit() to track position in tree.
    path_id: u64,

    /// Stack of path IDs for restoring on exit().
    path_stack: Vec<u64>,

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
    /// Create a new layout context without caching.
    pub fn new(snapshot: &'a mut LayoutSnapshot) -> Self {
        Self {
            snapshot,
            flex_scratch: Vec::with_capacity(32), // Pre-allocate for typical UI depth
            cache: None,
            path_id: 0,
            path_stack: Vec::with_capacity(16),
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

    /// Create a new layout context with caching enabled.
    pub fn with_cache(snapshot: &'a mut LayoutSnapshot, cache: &'a mut LayoutCache) -> Self {
        Self {
            snapshot,
            flex_scratch: Vec::with_capacity(32),
            cache: Some(cache),
            path_id: 0,
            path_stack: Vec::with_capacity(16),
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
        // Also set on snapshot so legacy layout() methods can participate
        self.snapshot.set_debug_enabled(enabled);
        self
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn with_debug(self, _enabled: bool) -> Self {
        self
    }

    /// Enter a child scope for path tracking.
    ///
    /// Updates the path_id with a rolling hash of the container name.
    /// This creates stable, path-based IDs for cache keys.
    ///
    /// Note: For sibling disambiguation, use `enter_indexed()` instead.
    pub fn enter(&mut self, name: &'static str) {
        // Save current path_id for restoration on exit
        self.path_stack.push(self.path_id);

        // Update path_id with a rolling hash (FNV-1a inspired)
        let name_hash = hash_str(name);
        self.path_id = self.path_id.wrapping_mul(0x100000001b3).wrapping_add(name_hash);

        #[cfg(debug_assertions)]
        {
            self.depth += 1;
            self.current_name = name;
        }
    }

    /// Enter a child scope with index for sibling disambiguation.
    ///
    /// This is used when iterating over children to ensure siblings with
    /// the same type/name get different path_ids. For example, two
    /// TextElement siblings would otherwise have identical paths.
    ///
    /// ```ignore
    /// ctx.enter("Row");
    /// for (i, child) in children.iter().enumerate() {
    ///     ctx.enter_indexed("child", i);
    ///     child.layout_with_constraints(ctx, ...);
    ///     ctx.exit();
    /// }
    /// ctx.exit();
    /// ```
    pub fn enter_indexed(&mut self, name: &'static str, index: usize) {
        // Save current path_id for restoration on exit
        self.path_stack.push(self.path_id);

        // Update path_id with a rolling hash that includes the index
        let name_hash = hash_str(name);
        self.path_id = self.path_id
            .wrapping_mul(0x100000001b3)
            .wrapping_add(name_hash)
            .wrapping_mul(0x100000001b3)
            .wrapping_add(index as u64);

        #[cfg(debug_assertions)]
        {
            self.depth += 1;
            self.current_name = name;
        }
    }

    /// Exit the current scope, restoring the parent's path_id.
    pub fn exit(&mut self) {
        self.path_id = self.path_stack.pop().unwrap_or(0);

        #[cfg(debug_assertions)]
        {
            self.depth = self.depth.saturating_sub(1);
        }
    }

    /// Get the current path-based ID for cache keys.
    #[inline]
    pub fn path_id(&self) -> u64 {
        self.path_id
    }

    /// Try to get a cached layout result.
    ///
    /// Returns `Some(size)` if cached, `None` if layout needs computation.
    #[inline]
    pub fn cache_get(&mut self, content_hash: u64, constraints: &LayoutConstraints) -> Option<Size> {
        self.cache.as_mut().and_then(|cache| {
            let key = LayoutCacheKey::new(self.path_id, content_hash, constraints);
            cache.get(key)
        })
    }

    /// Try to get a cached layout for FlowContainer (width-only key).
    #[inline]
    pub fn cache_get_flow(&mut self, content_hash: u64, max_width: f32) -> Option<Size> {
        self.cache.as_mut().and_then(|cache| {
            let key = LayoutCacheKey::for_flow(self.path_id, content_hash, max_width);
            cache.get(key)
        })
    }

    /// Store a layout result in the cache.
    #[inline]
    pub fn cache_insert(&mut self, content_hash: u64, constraints: &LayoutConstraints, size: Size) {
        if let Some(cache) = self.cache.as_mut() {
            let key = LayoutCacheKey::new(self.path_id, content_hash, constraints);
            cache.insert(key, size);
        }
    }

    /// Store a FlowContainer layout result in the cache.
    #[inline]
    pub fn cache_insert_flow(&mut self, content_hash: u64, max_width: f32, size: Size) {
        if let Some(cache) = self.cache.as_mut() {
            let key = LayoutCacheKey::for_flow(self.path_id, content_hash, max_width);
            cache.insert(key, size);
        }
    }

    /// Check if caching is enabled.
    #[inline]
    pub fn has_cache(&self) -> bool {
        self.cache.is_some()
    }

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

    /// Push a debug rectangle for layout visualization.
    ///
    /// Call this from containers during layout when debug mode is enabled.
    /// The rectangle will be rendered as a semi-transparent overlay.
    ///
    /// # Arguments
    /// * `rect` - The bounds of this layout element
    /// * `is_overflow` - Whether this element exceeded its constraints
    #[cfg(debug_assertions)]
    pub fn push_debug_rect(&mut self, rect: crate::primitives::Rect, is_overflow: bool) {
        if self.debug_enabled {
            self.snapshot.push_debug_rect(
                rect,
                self.current_name.to_string(),
                self.depth,
                is_overflow,
            );
        }
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn push_debug_rect(&mut self, _rect: crate::primitives::Rect, _is_overflow: bool) {}

    /// Get the current depth in the layout tree (debug builds only).
    #[cfg(debug_assertions)]
    pub fn depth(&self) -> u32 {
        self.depth
    }

    #[cfg(not(debug_assertions))]
    #[inline(always)]
    pub fn depth(&self) -> u32 {
        0
    }
}

/// Fast string hash for path-based IDs (FNV-1a).
#[inline]
fn hash_str(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV prime
    }
    hash
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

    #[test]
    fn test_path_id_tracking() {
        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        assert_eq!(ctx.path_id(), 0);

        ctx.enter("Column");
        let id1 = ctx.path_id();
        assert_ne!(id1, 0); // Path changed

        ctx.enter("Row");
        let id2 = ctx.path_id();
        assert_ne!(id2, id1); // Deeper path = different ID

        ctx.exit();
        assert_eq!(ctx.path_id(), id1); // Back to parent's ID

        ctx.exit();
        assert_eq!(ctx.path_id(), 0); // Back to root
    }

    #[test]
    fn test_path_id_stability() {
        // Same path should produce same ID
        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        ctx.enter("Column");
        ctx.enter("Row");
        let id1 = ctx.path_id();
        ctx.exit();
        ctx.exit();

        ctx.enter("Column");
        ctx.enter("Row");
        let id2 = ctx.path_id();

        assert_eq!(id1, id2, "Same path should produce same ID");
    }

    #[test]
    fn test_enter_indexed_sibling_disambiguation() {
        // Two siblings with the same name but different indices should have different path_ids
        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        ctx.enter("Row");

        // First child at index 0
        ctx.enter_indexed("child", 0);
        let id0 = ctx.path_id();
        ctx.exit();

        // Second child at index 1
        ctx.enter_indexed("child", 1);
        let id1 = ctx.path_id();
        ctx.exit();

        // Third child at index 2
        ctx.enter_indexed("child", 2);
        let id2 = ctx.path_id();
        ctx.exit();

        ctx.exit();

        // All siblings should have different path_ids
        assert_ne!(id0, id1, "Different indices must produce different path_ids");
        assert_ne!(id1, id2, "Different indices must produce different path_ids");
        assert_ne!(id0, id2, "Different indices must produce different path_ids");
    }

    #[test]
    fn test_enter_indexed_stability() {
        // Same index should produce same ID across frames
        let mut snapshot = LayoutSnapshot::new();
        let mut ctx = LayoutContext::new(&mut snapshot);

        ctx.enter("Row");
        ctx.enter_indexed("child", 5);
        let id1 = ctx.path_id();
        ctx.exit();
        ctx.exit();

        ctx.enter("Row");
        ctx.enter_indexed("child", 5);
        let id2 = ctx.path_id();
        ctx.exit();
        ctx.exit();

        assert_eq!(id1, id2, "Same path with same index should produce same ID");
    }

    #[test]
    fn test_cache_disabled_by_default() {
        let mut snapshot = LayoutSnapshot::new();
        let ctx = LayoutContext::new(&mut snapshot);
        assert!(!ctx.has_cache());
    }

    #[test]
    fn test_cache_enabled() {
        let mut snapshot = LayoutSnapshot::new();
        let mut cache = LayoutCache::new();
        let ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);
        assert!(ctx.has_cache());
    }

    #[test]
    fn test_cache_get_insert() {
        let mut snapshot = LayoutSnapshot::new();
        let mut cache = LayoutCache::new();
        let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);

        ctx.enter("TestContainer");
        let constraints = LayoutConstraints::loose(500.0, 300.0);
        let size = Size::new(200.0, 150.0);

        // Initially no cache entry
        assert_eq!(ctx.cache_get(123, &constraints), None);

        // Insert and retrieve
        ctx.cache_insert(123, &constraints, size);
        assert_eq!(ctx.cache_get(123, &constraints), Some(size));
    }

    #[test]
    fn test_cache_flow_specific() {
        let mut snapshot = LayoutSnapshot::new();
        let mut cache = LayoutCache::new();
        let mut ctx = LayoutContext::with_cache(&mut snapshot, &mut cache);

        ctx.enter("FlowContainer");
        let size = Size::new(180.0, 54.0);

        // Flow uses width-only key
        ctx.cache_insert_flow(456, 200.0, size);
        assert_eq!(ctx.cache_get_flow(456, 200.0), Some(size));

        // Different width = cache miss
        assert_eq!(ctx.cache_get_flow(456, 300.0), None);
    }
}
