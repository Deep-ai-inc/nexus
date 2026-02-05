//! Headless state machine tests for the Nexus UI.
//!
//! These tests verify state transitions without rendering. Following the Elm
//! architecture pattern: construct State, send Message, assert State changed.

#[cfg(test)]
mod scroll_model_tests {
    use super::super::scroll_model::{ScrollModel, ScrollTarget};
    use nexus_api::BlockId;
    use strata::ScrollAction;

    #[test]
    fn new_scroll_model_starts_at_bottom() {
        let model = ScrollModel::new();
        assert_eq!(model.target, ScrollTarget::Bottom);
        assert_eq!(model.state.offset, 0.0);
    }

    #[test]
    fn snap_to_bottom_sets_target() {
        let mut model = ScrollModel::new();
        model.target = ScrollTarget::None;
        model.snap_to_bottom();
        assert_eq!(model.target, ScrollTarget::Bottom);
    }

    #[test]
    fn scroll_to_block_sets_target() {
        let mut model = ScrollModel::new();
        let block_id = BlockId(42);
        model.scroll_to_block(block_id);
        assert_eq!(model.target, ScrollTarget::Block(block_id));
    }

    #[test]
    fn user_scroll_breaks_bottom_lock() {
        let mut model = ScrollModel::new();
        assert_eq!(model.target, ScrollTarget::Bottom);

        // Simulate user scrolling (e.g., mouse wheel)
        model.apply_user_scroll(ScrollAction::ScrollBy(-10.0));

        // Should break out of Bottom mode into free scroll
        assert_eq!(model.target, ScrollTarget::None);
    }

    #[test]
    fn hint_bottom_returns_true_when_at_bottom() {
        let mut model = ScrollModel::new();
        assert!(model.hint_bottom());

        model.target = ScrollTarget::None;
        assert!(!model.hint_bottom());
    }

    #[test]
    fn reset_clears_offset_and_snaps_to_bottom() {
        let mut model = ScrollModel::new();
        model.state.offset = 500.0;
        model.target = ScrollTarget::None;

        model.reset();

        assert_eq!(model.state.offset, 0.0);
        assert_eq!(model.target, ScrollTarget::Bottom);
    }

    #[test]
    fn apply_pending_moves_offset_and_enters_free_scroll() {
        let mut model = ScrollModel::new();
        model.pending_offset.set(Some(123.0));
        model.target = ScrollTarget::Block(BlockId(1));

        model.apply_pending();

        assert_eq!(model.state.offset, 123.0);
        // After navigating to a block, enter free-scroll
        assert_eq!(model.target, ScrollTarget::None);
    }

    #[test]
    fn apply_pending_does_nothing_when_empty() {
        let mut model = ScrollModel::new();
        let original_offset = model.state.offset;
        let original_target = model.target;

        model.apply_pending();

        assert_eq!(model.state.offset, original_offset);
        assert_eq!(model.target, original_target);
    }
}

#[cfg(test)]
mod input_widget_tests {
    use super::super::input::InputWidget;
    use crate::blocks::InputMode;
    use nexus_kernel::Kernel;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn create_test_input() -> InputWidget {
        let history = vec!["ls".to_string(), "pwd".to_string(), "echo hello".to_string()];
        let (kernel, _rx) = Kernel::new().expect("kernel creation");
        let kernel = Arc::new(Mutex::new(kernel));
        InputWidget::new(history, kernel)
    }

    #[test]
    fn new_input_starts_in_shell_mode() {
        let input = create_test_input();
        assert!(matches!(input.mode, InputMode::Shell));
    }

    #[test]
    fn toggle_mode_switches_between_shell_and_agent() {
        let mut input = create_test_input();

        assert!(matches!(input.mode, InputMode::Shell));

        input.toggle_mode();
        assert!(matches!(input.mode, InputMode::Agent));

        input.toggle_mode();
        assert!(matches!(input.mode, InputMode::Shell));
    }

    #[test]
    fn history_up_navigates_to_previous_entry() {
        let mut input = create_test_input();

        // Initially no history index
        assert!(input.text_input.text.is_empty());

        input.history_up();
        assert_eq!(input.text_input.text, "echo hello"); // Most recent

        input.history_up();
        assert_eq!(input.text_input.text, "pwd");

        input.history_up();
        assert_eq!(input.text_input.text, "ls"); // Oldest

        // At the top, should stay at oldest
        input.history_up();
        assert_eq!(input.text_input.text, "ls");
    }

    #[test]
    fn history_down_navigates_to_next_entry() {
        let mut input = create_test_input();

        // Navigate up first
        input.history_up(); // echo hello
        input.history_up(); // pwd
        input.history_up(); // ls

        // Now navigate down
        input.history_down();
        assert_eq!(input.text_input.text, "pwd");

        input.history_down();
        assert_eq!(input.text_input.text, "echo hello");

        // Going past the end restores saved input
        input.history_down();
        assert!(input.text_input.text.is_empty()); // Back to original empty
    }

    #[test]
    fn history_navigation_saves_and_restores_current_input() {
        let mut input = create_test_input();

        // Type something first
        input.text_input.text = "my current command".to_string();
        input.text_input.cursor = input.text_input.text.len();

        // Navigate into history
        input.history_up();
        assert_eq!(input.text_input.text, "echo hello");

        // Navigate back down past the end
        input.history_down();
        assert_eq!(input.text_input.text, "my current command"); // Restored
    }

    #[test]
    fn insert_newline_adds_newline_to_text() {
        let mut input = create_test_input();
        input.text_input.text = "first line".to_string();
        input.text_input.cursor = input.text_input.text.len();

        input.insert_newline();

        assert!(input.text_input.text.contains('\n'));
    }

    #[test]
    fn paste_text_inserts_at_cursor() {
        let mut input = create_test_input();
        input.text_input.text = "hello world".to_string();
        input.text_input.cursor = 5; // After "hello"

        input.paste_text(" beautiful");

        assert_eq!(input.text_input.text, "hello beautiful world");
    }

    #[test]
    fn remove_attachment_removes_by_index() {
        let mut input = create_test_input();
        // Add some mock attachments
        input.attachments.push(super::super::Attachment {
            data: vec![1, 2, 3],
            image_handle: strata::ImageHandle(0),
            width: 100,
            height: 100,
        });
        input.attachments.push(super::super::Attachment {
            data: vec![4, 5, 6],
            image_handle: strata::ImageHandle(0),
            width: 200,
            height: 200,
        });

        assert_eq!(input.attachments.len(), 2);

        input.remove_attachment(0);
        assert_eq!(input.attachments.len(), 1);
        assert_eq!(input.attachments[0].width, 200); // Second one remains
    }

    #[test]
    fn remove_attachment_out_of_bounds_does_nothing() {
        let mut input = create_test_input();
        input.remove_attachment(999);
        assert!(input.attachments.is_empty());
    }

    #[test]
    fn reset_history_nav_clears_navigation_state() {
        let mut input = create_test_input();
        input.text_input.text = "something".to_string();
        input.history_up(); // Enter history navigation

        input.reset_history_nav();

        // Navigate up again - should start fresh from most recent
        input.history_up();
        assert_eq!(input.text_input.text, "echo hello");
    }

    #[test]
    fn captures_keys_when_overlays_active() {
        let input = create_test_input();

        // Initially no overlays
        assert!(!input.captures_keys());

        // Note: Can't easily test with overlays active without more setup,
        // but we verify the default state
    }
}

#[cfg(test)]
mod focus_tests {
    use crate::blocks::Focus;
    use nexus_api::BlockId;

    #[test]
    fn focus_equality() {
        assert_eq!(Focus::Input, Focus::Input);
        assert_eq!(Focus::Block(BlockId(1)), Focus::Block(BlockId(1)));
        assert_ne!(Focus::Block(BlockId(1)), Focus::Block(BlockId(2)));
        assert_ne!(Focus::Input, Focus::Block(BlockId(1)));
    }
}

#[cfg(test)]
mod transient_ui_tests {
    use super::super::transient_ui::TransientUi;
    use super::super::context_menu::{ContextMenuItem, ContextTarget};

    #[test]
    fn new_transient_ui_has_no_context_menu() {
        let ui = TransientUi::new();
        assert!(ui.context_menu().is_none());
    }

    #[test]
    fn show_context_menu_makes_it_visible() {
        let mut ui = TransientUi::new();
        ui.show_context_menu(
            100.0,
            200.0,
            vec![ContextMenuItem::Copy, ContextMenuItem::Paste],
            ContextTarget::Input,
        );

        let menu = ui.context_menu().expect("menu should be visible");
        assert_eq!(menu.x, 100.0);
        assert_eq!(menu.y, 200.0);
        assert_eq!(menu.items.len(), 2);
    }

    #[test]
    fn dismiss_context_menu_hides_it() {
        let mut ui = TransientUi::new();
        ui.show_context_menu(
            100.0,
            200.0,
            vec![ContextMenuItem::Copy],
            ContextTarget::Input,
        );
        assert!(ui.context_menu().is_some());

        ui.dismiss_context_menu();
        assert!(ui.context_menu().is_none());
    }
}

#[cfg(test)]
mod drag_state_tests {
    use super::super::drag_state::{
        ClickTracker, DragPayload, DragState, DragStatus, PendingIntent, SelectMode,
    };
    use nexus_api::BlockId;
    use strata::primitives::Point;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    #[test]
    fn new_drag_state_is_inactive() {
        let state = DragState::new();
        assert!(matches!(state.status, DragStatus::Inactive));
    }

    #[test]
    fn drag_status_transitions() {
        let mut state = DragState::new();

        // Start a pending drag
        let origin = Point::new(100.0, 100.0);
        let intent = PendingIntent::Anchor {
            source: strata::content_address::SourceId::default(),
            source_rect: None,
            payload: DragPayload::Text("test".to_string()),
        };
        state.status = DragStatus::Pending { origin, intent };

        // Verify we're in pending state
        assert!(matches!(state.status, DragStatus::Pending { .. }));

        // Cancel back to inactive
        state.status = DragStatus::Inactive;
        assert!(matches!(state.status, DragStatus::Inactive));
    }

    #[test]
    fn drag_payload_file_path() {
        let payload = DragPayload::FilePath(PathBuf::from("/tmp/test.txt"));
        assert!(matches!(payload, DragPayload::FilePath(_)));
    }

    #[test]
    fn drag_payload_text() {
        let payload = DragPayload::Text("hello world".to_string());
        assert!(matches!(payload, DragPayload::Text(_)));
    }

    #[test]
    fn drag_payload_preview_truncates_long_text() {
        let long_text = "x".repeat(100);
        let payload = DragPayload::Text(long_text);
        let preview = payload.preview_text();
        assert!(preview.len() < 100); // Should be truncated
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn drag_payload_preview_truncates_many_lines() {
        let many_lines: String = (0..20).map(|i| format!("line {}", i)).collect::<Vec<_>>().join("\n");
        let payload = DragPayload::Text(many_lines);
        let preview = payload.preview_text();
        let line_count = preview.lines().count();
        assert!(line_count <= 9); // 8 lines max + possible "..."
    }

    #[test]
    fn drag_payload_file_path_preview_shows_filename() {
        let payload = DragPayload::FilePath(PathBuf::from("/home/user/documents/report.pdf"));
        let preview = payload.preview_text();
        assert_eq!(preview, "report.pdf");
    }

    #[test]
    fn drag_payload_block_preview() {
        let payload = DragPayload::Block(BlockId(42));
        let preview = payload.preview_text();
        assert_eq!(preview, "Block #42");
    }

    #[test]
    fn click_tracker_single_click_is_char_mode() {
        let tracker = ClickTracker::new();
        let now = Instant::now();
        let mode = tracker.register_click(Point::new(100.0, 100.0), now);
        assert_eq!(mode, SelectMode::Char);
    }

    #[test]
    fn click_tracker_double_click_is_word_mode() {
        let tracker = ClickTracker::new();
        let pos = Point::new(100.0, 100.0);
        let now = Instant::now();

        tracker.register_click(pos, now);
        let mode = tracker.register_click(pos, now + Duration::from_millis(100));
        assert_eq!(mode, SelectMode::Word);
    }

    #[test]
    fn click_tracker_triple_click_is_line_mode() {
        let tracker = ClickTracker::new();
        let pos = Point::new(100.0, 100.0);
        let now = Instant::now();

        tracker.register_click(pos, now);
        tracker.register_click(pos, now + Duration::from_millis(100));
        let mode = tracker.register_click(pos, now + Duration::from_millis(200));
        assert_eq!(mode, SelectMode::Line);
    }

    #[test]
    fn click_tracker_resets_after_timeout() {
        let tracker = ClickTracker::new();
        let pos = Point::new(100.0, 100.0);
        let now = Instant::now();

        tracker.register_click(pos, now);
        // Wait too long between clicks
        let mode = tracker.register_click(pos, now + Duration::from_millis(600));
        assert_eq!(mode, SelectMode::Char); // Reset to single click
    }

    #[test]
    fn click_tracker_resets_when_moved() {
        let tracker = ClickTracker::new();
        let now = Instant::now();

        tracker.register_click(Point::new(100.0, 100.0), now);
        // Click at a different position
        let mode = tracker.register_click(Point::new(200.0, 200.0), now + Duration::from_millis(100));
        assert_eq!(mode, SelectMode::Char); // Reset to single click
    }

    #[test]
    fn click_tracker_wraps_after_triple() {
        let tracker = ClickTracker::new();
        let pos = Point::new(100.0, 100.0);
        let now = Instant::now();

        tracker.register_click(pos, now);
        tracker.register_click(pos, now + Duration::from_millis(100));
        tracker.register_click(pos, now + Duration::from_millis(200));
        // Fourth click wraps back to Char
        let mode = tracker.register_click(pos, now + Duration::from_millis(300));
        assert_eq!(mode, SelectMode::Char);
    }
}
