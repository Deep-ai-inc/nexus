//! Event routing — dispatches keyboard and mouse events to the appropriate widgets.

use nexus_api::Value;
use strata::event_context::{
    CaptureState, Key, KeyEvent, MouseButton, MouseEvent, NamedKey,
};
use strata::layout_snapshot::HitResult;
use strata::{MouseResponse, ScrollAction, route_mouse};

use crate::blocks::Focus;
use crate::nexus_widgets::{CompletionPopup, HistorySearchBar, JobBar};

use super::context_menu::{ContextMenuItem, ContextTarget};
use super::source_ids;
use super::{NexusMessage, NexusState};

/// Route keyboard events to the appropriate widget or message.
pub(super) fn on_key(state: &NexusState, event: KeyEvent) -> Option<NexusMessage> {
    // Only handle presses
    if matches!(&event, KeyEvent::Released { .. }) {
        return None;
    }

    if let KeyEvent::Pressed {
        ref key,
        ref modifiers,
        ..
    } = event
    {
        // History search mode intercepts most keys
        if state.input.history_search.is_active() {
            if modifiers.ctrl {
                if let Key::Character(c) = key {
                    if c == "r" {
                        return Some(NexusMessage::HistorySearchToggle);
                    }
                }
            }
            return match key {
                Key::Named(NamedKey::Enter) => Some(NexusMessage::HistorySearchAccept),
                Key::Named(NamedKey::Escape) => Some(NexusMessage::HistorySearchDismiss),
                Key::Named(NamedKey::ArrowDown) => {
                    if !state.input.history_search.results.is_empty()
                        && state.input.history_search.index
                            < state.input.history_search.results.len() - 1
                    {
                        Some(NexusMessage::HistorySearchSelect(
                            state.input.history_search.index + 1,
                        ))
                    } else {
                        None
                    }
                }
                Key::Named(NamedKey::ArrowUp) => {
                    if state.input.history_search.index > 0 {
                        Some(NexusMessage::HistorySearchSelect(
                            state.input.history_search.index - 1,
                        ))
                    } else {
                        None
                    }
                }
                _ => Some(NexusMessage::HistorySearchKey(event)),
            };
        }

        // Completion popup intercepts navigation keys when visible.
        // Non-navigation keys dismiss the popup and fall through to normal input handling.
        if state.input.completion.is_active() {
            match key {
                Key::Named(NamedKey::Tab) if modifiers.shift => {
                    return Some(NexusMessage::CompletionNav(-1));
                }
                Key::Named(NamedKey::Tab) => return Some(NexusMessage::CompletionNav(1)),
                Key::Named(NamedKey::ArrowDown) => return Some(NexusMessage::CompletionNav(1)),
                Key::Named(NamedKey::ArrowUp) => return Some(NexusMessage::CompletionNav(-1)),
                Key::Named(NamedKey::Enter) => return Some(NexusMessage::CompletionAccept),
                Key::Named(NamedKey::Escape) => return Some(NexusMessage::CompletionDismiss),
                _ => {
                    // Dismiss completion but don't consume the key — fall through
                    // to normal input routing so the keystroke is not lost.
                    return Some(NexusMessage::CompletionDismissAndForward(event));
                }
            }
        }

        // Cmd shortcuts (global)
        if modifiers.meta {
            if let Key::Character(c) = key {
                match c.as_str() {
                    "q" => return Some(NexusMessage::CloseWindow),
                    "k" => return Some(NexusMessage::ClearScreen),
                    "w" => return Some(NexusMessage::CloseWindow),
                    "c" => return Some(NexusMessage::Copy),
                    "v" => return Some(NexusMessage::Paste),
                    "." => return Some(NexusMessage::ToggleMode),
                    _ => {}
                }
            }
        }

        // Ctrl shortcuts (global)
        if modifiers.ctrl {
            if let Key::Character(c) = key {
                match c.as_str() {
                    "r" => return Some(NexusMessage::HistorySearchToggle),
                    "c" => {
                        if state.agent.is_active() {
                            return Some(NexusMessage::AgentInterrupt);
                        }
                        return Some(NexusMessage::SendInterrupt);
                    }
                    _ => {}
                }
            }
        }

        // Escape: dismiss overlays, interrupt agent, leave PTY focus, clear selection
        if matches!(key, Key::Named(NamedKey::Escape)) {
            if state.context_menu.is_some() {
                return Some(NexusMessage::DismissContextMenu);
            }
            if state.agent.is_active() {
                return Some(NexusMessage::AgentInterrupt);
            }
            if matches!(state.focus, Focus::Block(_)) {
                return Some(NexusMessage::BlurAll);
            }
            if state.selection.selection.is_some() {
                return Some(NexusMessage::ClearSelection);
            }
        }

        // When a PTY block is focused, forward keys to it
        if let Focus::Block(_) = state.focus {
            return Some(NexusMessage::PtyInput(event));
        }

        // When input is focused, route keys
        if state.input.text_input.focused {
            if matches!(key, Key::Named(NamedKey::Enter)) && modifiers.shift {
                return Some(NexusMessage::InsertNewline);
            }
            if matches!(key, Key::Named(NamedKey::Tab)) {
                return Some(NexusMessage::TabComplete);
            }
            if matches!(key, Key::Named(NamedKey::ArrowUp)) {
                return Some(NexusMessage::HistoryUp);
            }
            if matches!(key, Key::Named(NamedKey::ArrowDown)) {
                return Some(NexusMessage::HistoryDown);
            }
            return Some(NexusMessage::InputKey(event));
        }

        // Global shortcuts when input not focused
        match key {
            Key::Named(NamedKey::PageUp) => {
                return Some(NexusMessage::HistoryScroll(ScrollAction::ScrollBy(300.0)));
            }
            Key::Named(NamedKey::PageDown) => {
                return Some(NexusMessage::HistoryScroll(ScrollAction::ScrollBy(-300.0)));
            }
            _ => {}
        }
    }

    None
}

/// Route mouse events to the appropriate widget or message.
pub(super) fn on_mouse(
    state: &NexusState,
    event: MouseEvent,
    hit: Option<HitResult>,
    capture: &CaptureState,
) -> MouseResponse<NexusMessage> {
    // Composable scroll + input handlers
    route_mouse!(&event, &hit, capture, [
        state.input.completion.scroll       => NexusMessage::CompletionScroll,
        state.input.history_search.scroll   => NexusMessage::HistorySearchScroll,
        state.history_scroll                => NexusMessage::HistoryScroll,
        state.input.text_input              => NexusMessage::InputMouse,
    ]);

    // Right-click → context menu
    if let MouseEvent::ButtonPressed {
        button: MouseButton::Right,
        position,
        ..
    } = &event
    {
        let input_bounds = state.input.text_input.bounds();
        if position.x >= input_bounds.x
            && position.x <= input_bounds.x + input_bounds.width
            && position.y >= input_bounds.y
            && position.y <= input_bounds.y + input_bounds.height
        {
            return MouseResponse::message(NexusMessage::ShowContextMenu(
                position.x,
                position.y,
                vec![
                    ContextMenuItem::Paste,
                    ContextMenuItem::SelectAll,
                    ContextMenuItem::Clear,
                ],
                ContextTarget::Input,
            ));
        }

        if let Some(HitResult::Content(ref addr)) = hit {
            // Match shell block content
            for block in &state.shell.blocks {
                let term_id = source_ids::shell_term(block.id);
                let header_id = source_ids::shell_header(block.id);
                let native_id = source_ids::native(block.id);
                let table_id = source_ids::table(block.id);
                if addr.source_id == term_id
                    || addr.source_id == header_id
                    || addr.source_id == native_id
                    || addr.source_id == table_id
                {
                    return MouseResponse::message(NexusMessage::ShowContextMenu(
                        position.x,
                        position.y,
                        vec![ContextMenuItem::Copy, ContextMenuItem::SelectAll],
                        ContextTarget::Block(block.id),
                    ));
                }
            }
            // Match agent block content
            for block in &state.agent.blocks {
                let query_id = source_ids::agent_query(block.id);
                let thinking_id = source_ids::agent_thinking(block.id);
                let response_id = source_ids::agent_response(block.id);
                if addr.source_id == query_id
                    || addr.source_id == thinking_id
                    || addr.source_id == response_id
                {
                    return MouseResponse::message(NexusMessage::ShowContextMenu(
                        position.x,
                        position.y,
                        vec![ContextMenuItem::Copy, ContextMenuItem::SelectAll],
                        ContextTarget::AgentBlock(block.id),
                    ));
                }
            }
        }

        // Fallback: right-click on non-content area (e.g., widget chrome)
        if hit.is_some() {
            if let Some(block) = state.shell.blocks.last() {
                return MouseResponse::message(NexusMessage::ShowContextMenu(
                    position.x,
                    position.y,
                    vec![ContextMenuItem::Copy, ContextMenuItem::SelectAll],
                    ContextTarget::Block(block.id),
                ));
            }
            if let Some(block) = state.agent.blocks.last() {
                return MouseResponse::message(NexusMessage::ShowContextMenu(
                    position.x,
                    position.y,
                    vec![ContextMenuItem::Copy, ContextMenuItem::SelectAll],
                    ContextTarget::AgentBlock(block.id),
                ));
            }
        }
    }

    // Hover tracking for popups
    if let MouseEvent::CursorMoved { .. } = &event {
        if let Some(ref menu) = state.context_menu {
            let idx = if let Some(HitResult::Widget(id)) = &hit {
                (0..menu.items.len())
                    .find(|i| *id == source_ids::ctx_menu_item(*i))
            } else {
                None
            };
            menu.hovered_item.set(idx);
        }

        if state.input.completion.is_active() {
            let idx = if let Some(HitResult::Widget(id)) = &hit {
                (0..state.input.completion.completions.len().min(10))
                    .find(|i| *id == CompletionPopup::item_id(*i))
            } else {
                None
            };
            state.input.completion.hovered.set(idx);
        }

        if state.input.history_search.is_active() {
            let idx = if let Some(HitResult::Widget(id)) = &hit {
                (0..state.input.history_search.results.len().min(10))
                    .find(|i| *id == HistorySearchBar::result_id(*i))
            } else {
                None
            };
            state.input.history_search.hovered.set(idx);
        }
    }

    // Context menu item clicks
    if let MouseEvent::ButtonPressed {
        button: MouseButton::Left,
        ..
    } = &event
    {
        if let Some(ref menu) = state.context_menu {
            if let Some(HitResult::Widget(id)) = &hit {
                for (i, item) in menu.items.iter().enumerate() {
                    if *id == source_ids::ctx_menu_item(i) {
                        return MouseResponse::message(NexusMessage::ContextMenuAction(*item));
                    }
                }
            }
            return MouseResponse::message(NexusMessage::DismissContextMenu);
        }
    }

    // Button clicks
    if let MouseEvent::ButtonPressed {
        button: MouseButton::Left,
        ..
    } = &event
    {
        if let Some(HitResult::Widget(id)) = &hit {
            // Mode toggle
            if *id == source_ids::mode_toggle() {
                return MouseResponse::message(NexusMessage::ToggleMode);
            }

            // Completion item clicks
            for i in 0..state.input.completion.completions.len().min(10) {
                if *id == CompletionPopup::item_id(i) {
                    return MouseResponse::message(NexusMessage::CompletionSelect(i));
                }
            }

            // History search result clicks
            if state.input.history_search.is_active() {
                for i in 0..state.input.history_search.results.len().min(10) {
                    if *id == HistorySearchBar::result_id(i) {
                        return MouseResponse::message(NexusMessage::HistorySearchAcceptIndex(i));
                    }
                }
            }

            // Attachment remove buttons
            for i in 0..state.input.attachments.len() {
                let remove_id = source_ids::remove_attachment(i);
                if *id == remove_id {
                    return MouseResponse::message(NexusMessage::RemoveAttachment(i));
                }
            }

            // Job pill clicks
            for job in &state.shell.jobs {
                if *id == JobBar::job_pill_id(job.id) {
                    return MouseResponse::message(NexusMessage::ScrollToJob(job.id));
                }
            }

            // Kill buttons
            for block in &state.shell.blocks {
                if block.is_running() {
                    let kill_id = source_ids::kill(block.id);
                    if *id == kill_id {
                        return MouseResponse::message(NexusMessage::KillBlock(block.id));
                    }
                }
            }

            // Agent thinking toggles, stop, tools, permissions
            for block in &state.agent.blocks {
                let thinking_id = source_ids::agent_thinking_toggle(block.id);
                if *id == thinking_id {
                    return MouseResponse::message(NexusMessage::ToggleThinking(block.id));
                }

                let stop_id = source_ids::agent_stop(block.id);
                if *id == stop_id {
                    return MouseResponse::message(NexusMessage::AgentInterrupt);
                }

                for (i, _tool) in block.tools.iter().enumerate() {
                    let toggle_id = source_ids::agent_tool_toggle(block.id, i);
                    if *id == toggle_id {
                        return MouseResponse::message(NexusMessage::ToggleTool(block.id, i));
                    }
                }

                if let Some(ref perm) = block.pending_permission {
                    let deny_id = source_ids::agent_perm_deny(block.id);
                    let allow_id = source_ids::agent_perm_allow(block.id);
                    let always_id = source_ids::agent_perm_always(block.id);

                    if *id == deny_id {
                        return MouseResponse::message(NexusMessage::PermissionDeny(
                            block.id,
                            perm.id.clone(),
                        ));
                    }
                    if *id == allow_id {
                        return MouseResponse::message(NexusMessage::PermissionGrant(
                            block.id,
                            perm.id.clone(),
                        ));
                    }
                    if *id == always_id {
                        return MouseResponse::message(NexusMessage::PermissionGrantSession(
                            block.id,
                            perm.id.clone(),
                        ));
                    }
                }
            }

            // Table sort header clicks
            for block in &state.shell.blocks {
                if let Some(Value::Table { columns, .. }) = &block.native_output {
                    for col_idx in 0..columns.len() {
                        let sort_id = source_ids::table_sort(block.id, col_idx);
                        if *id == sort_id {
                            return MouseResponse::message(NexusMessage::SortTable(
                                block.id, col_idx,
                            ));
                        }
                    }
                }
            }
        }

        // Text content selection
        if let Some(HitResult::Content(addr)) = hit {
            let capture_source = addr.source_id;
            return MouseResponse::message_and_capture(
                NexusMessage::SelectionStart(addr),
                capture_source,
            );
        }

        // Clicked empty space: blur inputs
        if state.input.text_input.focused {
            return MouseResponse::message(NexusMessage::BlurAll);
        }
    }

    // Selection drag
    if let MouseEvent::CursorMoved { .. } = &event {
        if let CaptureState::Captured(_) = capture {
            if let Some(HitResult::Content(addr)) = hit {
                return MouseResponse::message(NexusMessage::SelectionExtend(addr));
            }
        }
    }

    // Selection release
    if let MouseEvent::ButtonReleased {
        button: MouseButton::Left,
        ..
    } = &event
    {
        if let CaptureState::Captured(_) = capture {
            return MouseResponse::message_and_release(NexusMessage::SelectionEnd);
        }
    }

    MouseResponse::none()
}
