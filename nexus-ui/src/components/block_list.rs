//! Block list component - virtualized list of command blocks.

#![allow(dead_code)]

use crate::view_model::Store;

/// A virtualized list of blocks.
pub struct BlockList {
    /// Visible range of blocks.
    visible_range: (usize, usize),

    /// Scroll offset in pixels.
    scroll_offset: f32,

    /// Total height of all blocks (estimated).
    total_height: f32,

    /// Height of the viewport.
    viewport_height: f32,
}

impl BlockList {
    /// Create a new block list.
    pub fn new() -> Self {
        Self {
            visible_range: (0, 0),
            scroll_offset: 0.0,
            total_height: 0.0,
            viewport_height: 0.0,
        }
    }

    /// Update the visible range based on scroll position.
    pub fn update_visible_range(&mut self, store: &Store, viewport_height: f32) {
        self.viewport_height = viewport_height;

        // Simple implementation: assume fixed block height
        let block_height = 100.0; // TODO: Dynamic height calculation
        let block_count = store.block_count();

        self.total_height = block_count as f32 * block_height;

        let first_visible = (self.scroll_offset / block_height) as usize;
        let visible_count = (viewport_height / block_height).ceil() as usize + 1;
        let last_visible = (first_visible + visible_count).min(block_count);

        self.visible_range = (first_visible, last_visible);
    }

    /// Scroll by the given delta.
    pub fn scroll(&mut self, delta: f32) {
        self.scroll_offset = (self.scroll_offset + delta)
            .max(0.0)
            .min((self.total_height - self.viewport_height).max(0.0));
    }

    /// Scroll to the bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = (self.total_height - self.viewport_height).max(0.0);
    }

    /// Get the visible range of block indices.
    pub fn visible_range(&self) -> (usize, usize) {
        self.visible_range
    }

    /// Get the current scroll offset.
    pub fn scroll_offset(&self) -> f32 {
        self.scroll_offset
    }
}

impl Default for BlockList {
    fn default() -> Self {
        Self::new()
    }
}
