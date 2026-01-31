//! Transient UI — overlay lifecycle (context menu, future: command palette, toasts).
//!
//! Centralizes dismiss logic so new overlays don't require scattered dismiss calls.

use std::cell::Cell;

use super::context_menu::{ContextMenuItem, ContextMenuState, ContextTarget};
use super::input::InputWidget;

pub(crate) struct TransientUi {
    context_menu: Option<ContextMenuState>,
}

impl TransientUi {
    pub fn new() -> Self {
        Self { context_menu: None }
    }

    pub fn context_menu(&self) -> Option<&ContextMenuState> {
        self.context_menu.as_ref()
    }

    pub fn show_context_menu(
        &mut self,
        x: f32,
        y: f32,
        items: Vec<ContextMenuItem>,
        target: ContextTarget,
    ) {
        self.context_menu = Some(ContextMenuState {
            x,
            y,
            items,
            target,
            hovered_item: Cell::new(None),
        });
    }

    pub fn dismiss_context_menu(&mut self) {
        self.context_menu = None;
    }

    /// Dismiss all transient overlays (context menu, completion, history search).
    pub fn dismiss_all(&mut self, input: &mut InputWidget) {
        self.context_menu = None;
        input.completion_dismiss();
        input.history_search_dismiss();
    }
}
