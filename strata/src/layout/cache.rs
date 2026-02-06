//! Layout cache for memoizing expensive container layouts.
//!
//! The cache stores (content_hash, constraints) -> Size mappings.
//! Only "heavy" containers (FlowContainer, ScrollColumn) should use this.
//! Simple Row/Column layouts are fast enough that caching overhead exceeds benefit.

use std::collections::HashMap;
use super::constraints::LayoutConstraints;
use crate::primitives::Size;

/// A persistent cache for layout results, retained across frames.
///
/// The cache uses content-based keys: if the content and constraints
/// are identical, the layout result is reused without re-computation.
#[derive(Debug, Default)]
pub struct LayoutCache {
    /// Cache entries: (path_id, content_hash, constraints_hash) -> cached size
    entries: HashMap<LayoutCacheKey, CachedLayout>,

    /// Current frame generation (for expiry tracking)
    generation: u64,

    /// Stats for debugging
    #[cfg(debug_assertions)]
    pub hits: u64,
    #[cfg(debug_assertions)]
    pub misses: u64,
}

/// The key for a cached layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayoutCacheKey {
    /// Path-based ID (hash of container path in tree)
    pub path_id: u64,
    /// Content hash (hash of children/text)
    pub content_hash: u64,
    /// Constraints hash (hash of relevant constraint values)
    pub constraints_hash: u64,
}

/// A cached layout result.
#[derive(Debug, Clone, Copy)]
struct CachedLayout {
    size: Size,
    generation: u64,
}

impl LayoutCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new frame. Call this once per frame before layout.
    ///
    /// This increments the generation counter but doesn't clear old entries
    /// (they expire naturally when not accessed).
    pub fn begin_frame(&mut self) {
        self.generation += 1;
    }

    /// Look up a cached layout result.
    ///
    /// Returns `Some(size)` if the cache contains a valid entry for this key,
    /// or `None` if the layout needs to be recomputed.
    #[inline]
    pub fn get(&mut self, key: LayoutCacheKey) -> Option<Size> {
        if let Some(entry) = self.entries.get_mut(&key) {
            // Update generation to mark as recently used
            entry.generation = self.generation;
            #[cfg(debug_assertions)]
            {
                self.hits += 1;
            }
            Some(entry.size)
        } else {
            #[cfg(debug_assertions)]
            {
                self.misses += 1;
            }
            None
        }
    }

    /// Store a layout result in the cache.
    #[inline]
    pub fn insert(&mut self, key: LayoutCacheKey, size: Size) {
        self.entries.insert(key, CachedLayout {
            size,
            generation: self.generation,
        });
    }

    /// Remove stale entries (not accessed in the last N frames).
    ///
    /// Call periodically (e.g., every 60 frames) to prevent unbounded growth.
    pub fn gc(&mut self, max_age: u64) {
        let cutoff = self.generation.saturating_sub(max_age);
        self.entries.retain(|_, entry| entry.generation >= cutoff);
    }

    /// Get the number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all cached entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Get cache stats (debug builds only).
    #[cfg(debug_assertions)]
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// Reset stats (debug builds only).
    #[cfg(debug_assertions)]
    pub fn reset_stats(&mut self) {
        self.hits = 0;
        self.misses = 0;
    }
}

impl LayoutCacheKey {
    /// Create a new cache key.
    #[inline]
    pub fn new(path_id: u64, content_hash: u64, constraints: &LayoutConstraints) -> Self {
        Self {
            path_id,
            content_hash,
            constraints_hash: hash_constraints(constraints),
        }
    }

    /// Create a key for a FlowContainer (only max_width matters).
    #[inline]
    pub fn for_flow(path_id: u64, content_hash: u64, max_width: f32) -> Self {
        Self {
            path_id,
            content_hash,
            constraints_hash: hash_f32(max_width),
        }
    }
}

/// Hash constraints into a u64 for cache key.
#[inline]
fn hash_constraints(c: &LayoutConstraints) -> u64 {
    // XOR the bit patterns of the constraint floats
    let w1 = c.min_width.to_bits() as u64;
    let w2 = c.max_width.to_bits() as u64;
    let h1 = c.min_height.to_bits() as u64;
    let h2 = c.max_height.to_bits() as u64;
    w1 ^ (w2 << 16) ^ (h1 << 32) ^ (h2 << 48)
}

/// Hash a single f32 (for FlowContainer's width-only key).
#[inline]
fn hash_f32(f: f32) -> u64 {
    f.to_bits() as u64
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_insert_get() {
        let mut cache = LayoutCache::new();
        let key = LayoutCacheKey::new(1, 100, &LayoutConstraints::loose(500.0, 300.0));
        let size = Size::new(200.0, 150.0);

        cache.insert(key, size);
        let result = cache.get(key);

        assert_eq!(result, Some(size));
    }

    #[test]
    fn test_cache_miss() {
        let mut cache = LayoutCache::new();
        let key = LayoutCacheKey::new(1, 100, &LayoutConstraints::loose(500.0, 300.0));

        let result = cache.get(key);
        assert_eq!(result, None);
    }

    #[test]
    fn test_cache_different_constraints() {
        let mut cache = LayoutCache::new();
        let key1 = LayoutCacheKey::new(1, 100, &LayoutConstraints::loose(500.0, 300.0));
        let key2 = LayoutCacheKey::new(1, 100, &LayoutConstraints::loose(400.0, 300.0));

        cache.insert(key1, Size::new(200.0, 150.0));

        // Different constraints = cache miss
        assert_eq!(cache.get(key2), None);
    }

    #[test]
    fn test_cache_gc() {
        let mut cache = LayoutCache::new();
        let key1 = LayoutCacheKey::new(1, 100, &LayoutConstraints::loose(500.0, 300.0));
        let key2 = LayoutCacheKey::new(2, 200, &LayoutConstraints::loose(500.0, 300.0));

        cache.insert(key1, Size::new(100.0, 50.0));
        cache.begin_frame();
        cache.begin_frame();
        cache.insert(key2, Size::new(200.0, 100.0));

        // key1 is now 2 generations old, key2 is current
        cache.gc(1); // Keep only entries from last 1 frame

        assert_eq!(cache.get(key1), None);  // Expired
        assert!(cache.get(key2).is_some()); // Still valid
    }

    #[test]
    fn test_flow_cache_key() {
        let key1 = LayoutCacheKey::for_flow(1, 100, 500.0);
        let key2 = LayoutCacheKey::for_flow(1, 100, 500.0);
        let key3 = LayoutCacheKey::for_flow(1, 100, 400.0);

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }
}
