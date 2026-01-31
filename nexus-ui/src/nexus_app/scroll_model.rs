//! Scroll model — owns scroll state + follow-output policy.
//!
//! Prevents direct field writes from accidentally breaking sticky-bottom semantics.

use strata::{LayoutSnapshot, ScrollAction, ScrollState};

pub(crate) struct ScrollModel {
    pub(super) state: ScrollState,
    follow: bool,
}

impl ScrollModel {
    pub fn new() -> Self {
        Self {
            state: ScrollState::new(),
            follow: true,
        }
    }

    /// Hint: scroll to bottom only if the user is already following output.
    /// Used by periodic output ticks and widget scroll-to-bottom outputs.
    pub fn hint(&mut self) {
        if self.follow {
            self.state.offset = self.state.max.get();
        }
    }

    /// Force: scroll to bottom and re-enable follow mode.
    /// Used by explicit user actions: submit, clear, command finished.
    pub fn force(&mut self) {
        self.state.offset = self.state.max.get();
        self.follow = true;
    }

    /// Reset scroll to top with follow enabled. Used by clear screen.
    pub fn reset(&mut self) {
        self.state.offset = 0.0;
        self.follow = true;
    }

    /// Apply a user scroll action, then update follow based on position.
    pub fn apply_user_scroll(&mut self, action: ScrollAction) {
        self.state.apply(action);
        let max = self.state.max.get();
        self.follow = (max - self.state.offset).abs() < 2.0;
    }

    pub fn sync_from_snapshot(&self, snapshot: &mut LayoutSnapshot) {
        self.state.sync_from_snapshot(snapshot);
    }
}
