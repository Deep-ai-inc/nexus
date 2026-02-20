//! Scroll model — owns scroll state + target-based follow policy.
//!
//! Three scroll targets:
//!   Bottom    — tail mode, auto-scroll on new output
//!   Block(id) — focusing a specific block, resolved after layout
//!   None      — free scroll, user reading history

use std::cell::Cell;

use nexus_api::BlockId;
use strata::{LayoutSnapshot, ScrollAction, ScrollPhase, ScrollState};

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
    pub(crate) state: ScrollState,
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
        self.state.reset_overscroll();
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
        self.state.reset_overscroll();
    }

    /// Apply a user scroll action (wheel, scrollbar drag, etc.).
    /// If locked to Bottom, sync offset to the real max first (since view()
    /// uses f32::MAX) and break the lock so the delta applies correctly.
    /// After the action, re-engage Bottom if the user scrolled near the max.
    pub fn apply_user_scroll(&mut self, action: ScrollAction) {
        let can_reengage = match &action {
            // Discrete wheel ticks and gesture end — interaction is over.
            ScrollAction::ScrollBy { phase: None | Some(ScrollPhase::Ended), .. } => true,
            // Scrollbar release.
            ScrollAction::DragEnd => true,
            // Mid-gesture: contact, momentum, drag move/start — don't lock.
            _ => false,
        };
        if self.target == ScrollTarget::Bottom {
            self.state.offset = self.state.max.get();
            self.target = ScrollTarget::None;
        }
        self.state.apply(action);
        if can_reengage {
            self.maybe_reengage_bottom();
        }
    }

    /// Re-engage Bottom follow mode when the user has scrolled to (or very
    /// near) the maximum offset. A small tolerance absorbs sub-pixel rounding
    /// so the user doesn't have to hit the exact bottom.
    fn maybe_reengage_bottom(&mut self) {
        const TOLERANCE: f32 = 5.0;
        // Don't interfere with active rubber-band / momentum bounce.
        if self.state.animating || self.state.overscroll != 0.0 {
            return;
        }
        let max = self.state.max.get();
        // Guard against the initial f32::MAX sentinel before the first layout.
        if max < f32::MAX / 2.0 && (max - self.state.offset) <= TOLERANCE {
            self.target = ScrollTarget::Bottom;
        }
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

    /// Advance the spring-back animation. Returns true if still animating.
    pub fn tick_overscroll(&mut self) -> bool {
        self.state.tick_spring_back()
    }

    pub fn sync_from_snapshot(&self, snapshot: &mut LayoutSnapshot) {
        self.state.sync_from_snapshot(snapshot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scroll_model_new() {
        let model = ScrollModel::new();
        assert_eq!(model.state.offset, 0.0);
        assert_eq!(model.target, ScrollTarget::Bottom);
        assert!(model.pending_offset.get().is_none());
    }

    #[test]
    fn test_hint_bottom_when_at_bottom() {
        let mut model = ScrollModel::new();
        model.target = ScrollTarget::Bottom;
        assert!(model.hint_bottom());
    }

    #[test]
    fn test_hint_bottom_when_not_at_bottom() {
        let mut model = ScrollModel::new();
        model.target = ScrollTarget::None;
        assert!(!model.hint_bottom());
    }

    #[test]
    fn test_hint_bottom_when_at_block() {
        let mut model = ScrollModel::new();
        model.target = ScrollTarget::Block(BlockId(42));
        assert!(!model.hint_bottom());
    }

    #[test]
    fn test_snap_to_bottom() {
        let mut model = ScrollModel::new();
        model.target = ScrollTarget::None;
        model.snap_to_bottom();
        assert_eq!(model.target, ScrollTarget::Bottom);
    }

    #[test]
    fn test_scroll_to_block() {
        let mut model = ScrollModel::new();
        let block_id = BlockId(123);
        model.scroll_to_block(block_id);
        assert_eq!(model.target, ScrollTarget::Block(block_id));
    }

    #[test]
    fn test_reset() {
        let mut model = ScrollModel::new();
        model.state.offset = 500.0;
        model.target = ScrollTarget::None;
        model.reset();
        assert_eq!(model.state.offset, 0.0);
        assert_eq!(model.target, ScrollTarget::Bottom);
    }

    #[test]
    fn test_apply_user_scroll_breaks_bottom_lock() {
        let mut model = ScrollModel::new();
        model.target = ScrollTarget::Bottom;
        model.state.max.set(1000.0);

        model.apply_user_scroll(ScrollAction::ScrollBy { delta: 100.0, phase: None });

        // Should break the bottom lock
        assert_eq!(model.target, ScrollTarget::None);
    }

    #[test]
    fn test_apply_user_scroll_when_not_at_bottom() {
        let mut model = ScrollModel::new();
        model.target = ScrollTarget::None;
        model.state.offset = 500.0;
        model.state.max.set(1000.0);

        model.apply_user_scroll(ScrollAction::ScrollBy { delta: -100.0, phase: None });

        // Target should remain None (still far from bottom)
        assert_eq!(model.target, ScrollTarget::None);
        // Offset should change
        assert_eq!(model.state.offset, 600.0);
    }

    #[test]
    fn test_scroll_to_bottom_reengages_follow() {
        let mut model = ScrollModel::new();
        model.target = ScrollTarget::None;
        model.state.offset = 995.0;
        model.state.max.set(1000.0);

        // Scroll down past max (clamps to 1000, within tolerance)
        model.apply_user_scroll(ScrollAction::ScrollBy { delta: -50.0, phase: None });

        assert_eq!(model.state.offset, 1000.0);
        assert_eq!(model.target, ScrollTarget::Bottom);
    }

    #[test]
    fn test_scroll_near_bottom_reengages_follow() {
        let mut model = ScrollModel::new();
        model.target = ScrollTarget::None;
        model.state.offset = 993.0;
        model.state.max.set(1000.0);

        // Scroll down to within tolerance (998 is within 5px of 1000)
        model.apply_user_scroll(ScrollAction::ScrollBy { delta: -5.0, phase: None });

        assert_eq!(model.state.offset, 998.0);
        assert_eq!(model.target, ScrollTarget::Bottom);
    }

    #[test]
    fn test_scroll_not_close_enough_stays_free() {
        let mut model = ScrollModel::new();
        model.target = ScrollTarget::None;
        model.state.offset = 980.0;
        model.state.max.set(1000.0);

        // Scroll down a bit but still >5px from bottom
        model.apply_user_scroll(ScrollAction::ScrollBy { delta: -5.0, phase: None });

        assert_eq!(model.state.offset, 985.0);
        assert_eq!(model.target, ScrollTarget::None);
    }

    #[test]
    fn test_apply_pending_with_pending_offset() {
        let mut model = ScrollModel::new();
        model.target = ScrollTarget::Block(BlockId(1));
        model.pending_offset.set(Some(300.0));

        model.apply_pending();

        assert_eq!(model.state.offset, 300.0);
        assert_eq!(model.target, ScrollTarget::None);
        assert!(model.pending_offset.get().is_none());
    }

    #[test]
    fn test_apply_pending_without_pending_offset() {
        let mut model = ScrollModel::new();
        model.state.offset = 100.0;
        model.target = ScrollTarget::Bottom;

        model.apply_pending();

        // Nothing should change
        assert_eq!(model.state.offset, 100.0);
        assert_eq!(model.target, ScrollTarget::Bottom);
    }

    #[test]
    fn test_scroll_target_equality() {
        assert_eq!(ScrollTarget::Bottom, ScrollTarget::Bottom);
        assert_eq!(ScrollTarget::None, ScrollTarget::None);
        assert_eq!(ScrollTarget::Block(BlockId(1)), ScrollTarget::Block(BlockId(1)));
        assert_ne!(ScrollTarget::Bottom, ScrollTarget::None);
        assert_ne!(ScrollTarget::Block(BlockId(1)), ScrollTarget::Block(BlockId(2)));
    }
}
