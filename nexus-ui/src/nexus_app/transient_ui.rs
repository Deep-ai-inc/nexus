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

    /// Check if any transient overlay is currently visible.
    pub fn has_overlay(&self) -> bool {
        self.context_menu.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_api::BlockId;
    use std::path::PathBuf;

    // -------------------------------------------------------------------------
    // TransientUi::new tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_new_has_no_context_menu() {
        let ui = TransientUi::new();
        assert!(ui.context_menu().is_none());
    }

    #[test]
    fn test_new_has_no_overlay() {
        let ui = TransientUi::new();
        assert!(!ui.has_overlay());
    }

    // -------------------------------------------------------------------------
    // show_context_menu tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_show_context_menu_sets_state() {
        let mut ui = TransientUi::new();
        ui.show_context_menu(100.0, 200.0, vec![ContextMenuItem::Copy], ContextTarget::Input);

        let menu = ui.context_menu().expect("menu should be present");
        assert_eq!(menu.x, 100.0);
        assert_eq!(menu.y, 200.0);
    }

    #[test]
    fn test_show_context_menu_stores_items() {
        let mut ui = TransientUi::new();
        let items = vec![ContextMenuItem::Copy, ContextMenuItem::Paste, ContextMenuItem::Clear];
        ui.show_context_menu(0.0, 0.0, items, ContextTarget::Input);

        let menu = ui.context_menu().unwrap();
        assert_eq!(menu.items.len(), 3);
        assert_eq!(menu.items[0], ContextMenuItem::Copy);
        assert_eq!(menu.items[1], ContextMenuItem::Paste);
        assert_eq!(menu.items[2], ContextMenuItem::Clear);
    }

    #[test]
    fn test_show_context_menu_stores_block_target() {
        let mut ui = TransientUi::new();
        let block_id = BlockId(42);
        ui.show_context_menu(0.0, 0.0, vec![ContextMenuItem::Rerun], ContextTarget::Block(block_id));

        let menu = ui.context_menu().unwrap();
        match &menu.target {
            ContextTarget::Block(id) => assert_eq!(*id, block_id),
            _ => panic!("Expected Block target"),
        }
    }

    #[test]
    fn test_show_context_menu_stores_agent_block_target() {
        let mut ui = TransientUi::new();
        let block_id = BlockId(99);
        ui.show_context_menu(0.0, 0.0, vec![], ContextTarget::AgentBlock(block_id));

        let menu = ui.context_menu().unwrap();
        match &menu.target {
            ContextTarget::AgentBlock(id) => assert_eq!(*id, block_id),
            _ => panic!("Expected AgentBlock target"),
        }
    }

    #[test]
    fn test_show_context_menu_has_overlay() {
        let mut ui = TransientUi::new();
        ui.show_context_menu(0.0, 0.0, vec![], ContextTarget::Input);
        assert!(ui.has_overlay());
    }

    #[test]
    fn test_show_context_menu_initializes_hovered_to_none() {
        let mut ui = TransientUi::new();
        ui.show_context_menu(0.0, 0.0, vec![ContextMenuItem::Copy], ContextTarget::Input);

        let menu = ui.context_menu().unwrap();
        assert!(menu.hovered_item.get().is_none());
    }

    // -------------------------------------------------------------------------
    // dismiss_context_menu tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_dismiss_context_menu_clears_state() {
        let mut ui = TransientUi::new();
        ui.show_context_menu(100.0, 200.0, vec![ContextMenuItem::Copy], ContextTarget::Input);
        assert!(ui.context_menu().is_some());

        ui.dismiss_context_menu();
        assert!(ui.context_menu().is_none());
    }

    #[test]
    fn test_dismiss_context_menu_no_overlay() {
        let mut ui = TransientUi::new();
        ui.show_context_menu(0.0, 0.0, vec![], ContextTarget::Input);
        ui.dismiss_context_menu();
        assert!(!ui.has_overlay());
    }

    #[test]
    fn test_dismiss_context_menu_when_already_empty() {
        let mut ui = TransientUi::new();
        ui.dismiss_context_menu(); // Should not panic
        assert!(ui.context_menu().is_none());
    }

    // -------------------------------------------------------------------------
    // State machine lifecycle tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_show_dismiss_show_cycle() {
        let mut ui = TransientUi::new();

        // First show
        ui.show_context_menu(10.0, 20.0, vec![ContextMenuItem::Copy], ContextTarget::Input);
        assert_eq!(ui.context_menu().unwrap().x, 10.0);

        // Dismiss
        ui.dismiss_context_menu();
        assert!(ui.context_menu().is_none());

        // Second show (different position)
        ui.show_context_menu(30.0, 40.0, vec![ContextMenuItem::Paste], ContextTarget::Input);
        let menu = ui.context_menu().unwrap();
        assert_eq!(menu.x, 30.0);
        assert_eq!(menu.y, 40.0);
        assert_eq!(menu.items[0], ContextMenuItem::Paste);
    }

    #[test]
    fn test_show_replaces_previous_menu() {
        let mut ui = TransientUi::new();

        ui.show_context_menu(0.0, 0.0, vec![ContextMenuItem::Copy], ContextTarget::Input);
        ui.show_context_menu(100.0, 100.0, vec![ContextMenuItem::Paste, ContextMenuItem::Clear], ContextTarget::Block(BlockId(5)));

        let menu = ui.context_menu().unwrap();
        assert_eq!(menu.x, 100.0);
        assert_eq!(menu.items.len(), 2);
        match &menu.target {
            ContextTarget::Block(id) => assert_eq!(*id, BlockId(5)),
            _ => panic!("Expected Block target"),
        }
    }

    // -------------------------------------------------------------------------
    // File-specific context menu items
    // -------------------------------------------------------------------------

    #[test]
    fn test_context_menu_with_file_items() {
        let mut ui = TransientUi::new();
        let path = PathBuf::from("/Users/test/document.txt");
        let items = vec![
            ContextMenuItem::QuickLook(path.clone()),
            ContextMenuItem::Open(path.clone()),
            ContextMenuItem::CopyPath(path.clone()),
            ContextMenuItem::RevealInFinder(path.clone()),
        ];
        ui.show_context_menu(50.0, 50.0, items, ContextTarget::Input);

        let menu = ui.context_menu().unwrap();
        assert_eq!(menu.items.len(), 4);

        // Verify paths are stored correctly
        if let ContextMenuItem::QuickLook(p) = &menu.items[0] {
            assert_eq!(p, &path);
        } else {
            panic!("Expected QuickLook");
        }
    }
}
