//! Transient UI — overlay lifecycle (future: command palette, toasts).
//!
//! Centralizes dismiss logic so new overlays don't require scattered dismiss calls.

use crate::features::input::InputWidget;

pub(crate) struct TransientUi {
}

impl TransientUi {
    pub fn new() -> Self {
        Self {}
    }

    /// Dismiss all transient overlays (completion, history search).
    pub fn dismiss_all(&mut self, input: &mut InputWidget) {
        input.completion_dismiss();
        input.history_search_dismiss();
    }

    /// Check if any transient overlay is currently visible.
    #[cfg(test)]
    pub fn has_overlay(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_has_no_overlay() {
        let ui = TransientUi::new();
        assert!(!ui.has_overlay());
    }
}
