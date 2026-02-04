//! Scroll model — owns scroll state + target-based follow policy.
//!
//! Three scroll targets:
//!   Bottom    — tail mode, auto-scroll on new output
//!   Block(id) — focusing a specific block, resolved after layout
//!   None      — free scroll, user reading history

use std::cell::Cell;

use nexus_api::BlockId;
use strata::{LayoutSnapshot, ScrollAction, ScrollState};

/// Where the viewport wants to be.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScrollTarget {
    /// Tail mode: stick to bottom, follow new output.
    Bottom,
    /// Focus a specific block: resolve position after layout.
    Block(BlockId),
    /// Free scroll: user is reading history, don't auto-scroll.
    None,
}

pub(crate) struct ScrollModel {
    pub(super) state: ScrollState,
    pub(crate) target: ScrollTarget,
    /// Deferred offset computed in view() for Block(id) targets.
    /// Applied at the start of the next update() frame.
    pub(crate) pending_offset: Cell<Option<f32>>,
}

impl ScrollModel {
    pub fn new() -> Self {
        Self {
            state: ScrollState::new(),
            target: ScrollTarget::Bottom,
            pending_offset: Cell::new(None),
        }
    }

    /// Passive hint: returns true if target is Bottom (viewport will follow
    /// new output via f32::MAX in view), false if the user is scrolled away.
    pub fn hint_bottom(&mut self) -> bool {
        self.target == ScrollTarget::Bottom
    }

    /// Active snap: set target to Bottom. The view pass uses f32::MAX
    /// so the layout engine clamps to the true new bottom.
    pub fn snap_to_bottom(&mut self) {
        self.target = ScrollTarget::Bottom;
    }

    /// Navigate to a specific block. Sets target to Block(id).
    /// Actual offset is computed in view() after layout, applied next frame.
    pub fn scroll_to_block(&mut self, id: BlockId) {
        self.target = ScrollTarget::Block(id);
    }

    /// Reset scroll to top with Bottom target. Used by clear screen.
    pub fn reset(&mut self) {
        self.state.offset = 0.0;
        self.target = ScrollTarget::Bottom;
    }

    /// Apply a user scroll action (wheel, scrollbar drag, etc.).
    /// If locked to Bottom, sync offset to the real max first (since view()
    /// uses f32::MAX) and break the lock so the delta applies correctly.
    pub fn apply_user_scroll(&mut self, action: ScrollAction) {
        if self.target == ScrollTarget::Bottom {
            self.state.offset = self.state.max.get();
            self.target = ScrollTarget::None;
        }
        self.state.apply(action);
    }

    /// Apply deferred offset from Block(id) resolution.
    /// Called at the top of dispatch_update().
    pub fn apply_pending(&mut self) {
        if let Some(offset) = self.pending_offset.take() {
            self.state.offset = offset;
            // After navigating to a block, enter free-scroll so output
            // arriving doesn't yank the viewport away from the target.
            self.target = ScrollTarget::None;
        }
    }

    pub fn sync_from_snapshot(&self, snapshot: &mut LayoutSnapshot) {
        self.state.sync_from_snapshot(snapshot);
    }
}
