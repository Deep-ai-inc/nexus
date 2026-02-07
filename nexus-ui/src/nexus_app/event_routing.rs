//! Event routing — dispatches keyboard and mouse events to the appropriate widgets.

use strata::event_context::{
    CaptureState, Key, KeyEvent, MouseButton, MouseEvent, NamedKey,
};
use strata::layout_snapshot::HitResult;
use strata::{MouseResponse, ScrollAction, route_mouse};

use crate::blocks::Focus;
use crate::widgets::JobBar;

use super::drag_state::PendingIntent;
use super::message::{
    AgentMsg, ContextMenuMsg, DragMsg, InputMsg, NexusMessage, SelectionMsg, ShellMsg, ViewerMsg,
};
use super::source_ids;
use super::NexusState;

// =========================================================================
// Keyboard routing
// =========================================================================

/// Route keyboard events to the appropriate widget or message.
///
/// When a PTY block is focused (and no viewer is active), the terminal gets
/// "first right of refusal" on all keys.  Only a small set of GUI chrome
/// shortcuts (`Cmd`+key on macOS) are carved out before the PTY sees them.
/// This ensures Escape, Ctrl+C, Ctrl+R, Ctrl+Z, etc. all reach the terminal
/// exactly as they would in iTerm2, Alacritty, or Kitty.
pub(super) fn on_key(state: &NexusState, event: KeyEvent) -> Option<NexusMessage> {
    if matches!(&event, KeyEvent::Released { .. }) {
        return None;
    }

    let KeyEvent::Pressed {
        ref key,
        ref modifiers,
        ..
    } = event
    else {
        return None;
    };

    // Phase 0: Active drag — Escape cancels (always, regardless of focus)
    if state.drag.is_active() && matches!(key, Key::Named(NamedKey::Escape)) {
        return Some(NexusMessage::Drag(DragMsg::Cancel));
    }

    // Phase 1: Cmd-key chrome shortcuts (window management, copy/paste).
    // These are intercepted regardless of focus — they control the GUI, not
    // the terminal.
    if modifiers.meta {
        // Cmd+Shift+D: Toggle debug layout visualization (debug builds only)
        #[cfg(debug_assertions)]
        if modifiers.shift {
            if let Key::Character(c) = key {
                if c == "d" || c == "D" {
                    return Some(NexusMessage::ToggleDebugLayout);
                }
            }
        }

        if let Some(msg) = route_cmd_shortcut(state, key) {
            return Some(msg);
        }
    }

    // Phase 2: Focused block — viewer → PTY → static block navigation.
    if let Focus::Block(id) = state.focus {
        // Ctrl+C always interrupts/exits, even when a viewer is active.
        // (Viewer handle_key only sees Key, not modifiers, so it would
        // misinterpret Ctrl+C as bare "c" — e.g. "sort by CPU" in top.)
        if modifiers.ctrl {
            if let Key::Character(c) = key {
                if c == "c" {
                    if state.block_has_active_pty(id) {
                        return Some(NexusMessage::Shell(ShellMsg::PtyInput(id, event)));
                    }
                    // No PTY — exit the viewer (calls cancel_block for native commands)
                    if let Some(block) = state.shell.block_by_id(id) {
                        if block.view_state.is_some() {
                            return Some(NexusMessage::Viewer(ViewerMsg::Exit(id)));
                        }
                    }
                    // Static block, no PTY, no viewer — return to input
                    return Some(NexusMessage::BlurAll);
                }
            }
        }

        // If a viewer is active on this block, let the viewer handle keys.
        if let Some(block) = state.shell.block_by_id(id) {
            if let Some(ref view_state) = block.view_state {
                if let Some(viewer_msg) = view_state.handle_key(id, key) {
                    return Some(NexusMessage::Viewer(viewer_msg));
                }
                // Viewer consumed focus; don't pass to PTY or fall through.
                return None;
            }
        }
        // Block has active PTY — forward directly to the terminal.
        if state.block_has_active_pty(id) {
            return Some(NexusMessage::Shell(ShellMsg::PtyInput(id, event)));
        }
        // Static block (no PTY, no viewer) — block navigation keys.
        return route_block_navigation(state, id, key, modifiers, event.clone());
    }

    // ── Below here: focus is Input or AgentInput ─────────────────────

    // Phase 3: Input overlay intercepts (history search, completion)
    if state.input.captures_keys() {
        return state.input.on_key(&event).map(NexusMessage::Input);
    }

    // Phase 4: Non-PTY global shortcuts (Ctrl+R for history, Ctrl+C for
    // agent interrupt, etc.)
    if let Some(msg) = route_global_shortcut(state, key, modifiers) {
        return Some(msg);
    }

    // Phase 5: Escape cascade (context menu dismiss, agent interrupt, etc.)
    if matches!(key, Key::Named(NamedKey::Escape)) {
        return route_escape(state);
    }

    // Phase 6: Focus-based routing for non-PTY focuses
    match state.focus {
        Focus::AgentInput => {
            return Some(NexusMessage::Agent(AgentMsg::QuestionInputKey(event)));
        }
        Focus::Input => {} // fall through to input widget
        Focus::Block(_) => unreachable!(), // handled above
    }

    // Phase 6b: Alt+Up/Down for prev/next block (works from Input focus too)
    if modifiers.alt && !modifiers.meta && !modifiers.ctrl {
        match key {
            Key::Named(NamedKey::ArrowUp) => return Some(NexusMessage::FocusPrevBlock),
            Key::Named(NamedKey::ArrowDown) => return Some(NexusMessage::FocusNextBlock),
            _ => {}
        }
    }

    // Phase 6c: Context-aware Tab — empty input cycles focus to agent question
    if matches!(key, Key::Named(NamedKey::Tab))
        && matches!(state.focus, Focus::Input)
        && state.input.text_input.text.trim().is_empty()
    {
        if state.agent.has_pending_question() {
            return Some(NexusMessage::FocusAgentInput);
        }
        // No agent question — fall through to InputWidget (TabComplete)
    }

    // Phase 7: Input-focused keys (delegated to InputWidget)
    if let Some(msg) = state.input.on_key(&event) {
        return Some(NexusMessage::Input(msg));
    }

    // Phase 8: Global fallback
    route_global_fallback(key)
}

/// Cmd+key shortcuts — GUI chrome that is always intercepted, even when a PTY
/// is focused.  These are the macOS standard window/edit shortcuts.
fn route_cmd_shortcut(_state: &NexusState, key: &Key) -> Option<NexusMessage> {
    if let Key::Character(c) = key {
        match c.as_str() {
            "n" => return Some(NexusMessage::NewWindow),
            "q" => return Some(NexusMessage::QuitApp),
            "w" => return Some(NexusMessage::CloseWindow),
            "k" => return Some(NexusMessage::ClearScreen),
            "c" => return Some(NexusMessage::Copy),
            "v" => return Some(NexusMessage::Paste),
            "." => return Some(NexusMessage::Input(InputMsg::ToggleMode)),
            "=" | "+" => return Some(NexusMessage::ZoomIn),
            "-" => return Some(NexusMessage::ZoomOut),
            "0" => return Some(NexusMessage::ZoomReset),
            _ => {}
        }
    }
    // Cmd+Arrow for first/last block (macOS top/bottom convention)
    match key {
        Key::Named(NamedKey::ArrowUp) => return Some(NexusMessage::FocusFirstBlock),
        Key::Named(NamedKey::ArrowDown) => return Some(NexusMessage::FocusLastBlock),
        _ => {}
    }
    None
}

/// Global shortcuts that only apply when a PTY block is NOT focused.
/// (Cmd+key shortcuts are handled earlier by `route_cmd_shortcut`.)
fn route_global_shortcut(
    state: &NexusState,
    key: &Key,
    modifiers: &strata::event_context::Modifiers,
) -> Option<NexusMessage> {
    if modifiers.ctrl {
        if let Key::Character(c) = key {
            match c.as_str() {
                "r" => return Some(NexusMessage::Input(InputMsg::HistorySearchToggle)),
                "l" => return Some(NexusMessage::ClearScreen),
                "o" => {
                    // Expand collapsed tools in the most recent agent block
                    if !state.agent.blocks.is_empty() {
                        return Some(NexusMessage::Agent(AgentMsg::ExpandAllTools));
                    }
                }
                "c" => {
                    if state.agent.is_active() {
                        return Some(NexusMessage::Agent(AgentMsg::Interrupt));
                    }
                    // Fallback: exit any active viewer even when focus is Input
                    if let Some(id) = state.shell.active_viewer_block() {
                        return Some(NexusMessage::Viewer(ViewerMsg::Exit(id)));
                    }
                    // Try to interrupt the most relevant running process
                    if let Some(id) = state.shell.interrupt_target(None) {
                        return Some(NexusMessage::Shell(ShellMsg::SendInterrupt(id)));
                    }
                    return None;
                }
                _ => {}
            }
        }
    }

    None
}

/// Escape handler for non-PTY focuses (Input, AgentInput).
/// When a PTY block is focused, Escape is forwarded to the terminal directly
/// (handled in Phase 2 of `on_key`).
fn route_escape(state: &NexusState) -> Option<NexusMessage> {
    strata::platform::close_quicklook();

    if state.transient.context_menu().is_some() {
        return Some(NexusMessage::ContextMenu(ContextMenuMsg::Dismiss));
    }
    if state.agent.is_active() {
        return Some(NexusMessage::Agent(AgentMsg::Interrupt));
    }
    // Fallback: exit any active viewer even when focus is Input
    if let Some(id) = state.shell.active_viewer_block() {
        return Some(NexusMessage::Viewer(ViewerMsg::Exit(id)));
    }
    if state.selection.selection.is_some() {
        return Some(NexusMessage::Selection(SelectionMsg::Clear));
    }
    // Navigate to last block when input is empty (avoids surprise mode switch mid-command)
    if matches!(state.focus, Focus::Input)
        && state.input.text_input.text.is_empty()
        && state.has_blocks()
    {
        return Some(NexusMessage::FocusLastBlock);
    }
    None
}

/// Key routing for a focused static block (no active PTY, no viewer).
/// Provides arrow-key navigation between blocks, Escape to return to input,
/// and type-through for character keys.
fn route_block_navigation(
    _state: &NexusState,
    _id: nexus_api::BlockId,
    key: &Key,
    modifiers: &strata::event_context::Modifiers,
    event: KeyEvent,
) -> Option<NexusMessage> {
    // Alt+Arrow for prev/next block (also handled from Input focus below)
    if modifiers.alt && !modifiers.meta && !modifiers.ctrl {
        match key {
            Key::Named(NamedKey::ArrowUp) => return Some(NexusMessage::FocusPrevBlock),
            Key::Named(NamedKey::ArrowDown) => return Some(NexusMessage::FocusNextBlock),
            _ => {}
        }
    }

    match key {
        Key::Named(NamedKey::Escape) => Some(NexusMessage::BlurAll),
        Key::Named(NamedKey::ArrowUp) => Some(NexusMessage::FocusPrevBlock),
        Key::Named(NamedKey::ArrowDown) => Some(NexusMessage::FocusNextBlock),
        Key::Named(NamedKey::Enter) => Some(NexusMessage::BlurAll),
        // Character key or Space without modifiers → type-through to input
        Key::Character(_) | Key::Named(NamedKey::Space)
            if !modifiers.ctrl && !modifiers.meta && !modifiers.alt =>
        {
            Some(NexusMessage::TypeThrough(event))
        }
        _ => None,
    }
}

fn route_global_fallback(key: &Key) -> Option<NexusMessage> {
    match key {
        Key::Named(NamedKey::PageUp) => {
            Some(NexusMessage::Scroll(ScrollAction::ScrollBy(300.0)))
        }
        Key::Named(NamedKey::PageDown) => {
            Some(NexusMessage::Scroll(ScrollAction::ScrollBy(-300.0)))
        }
        _ => None,
    }
}

// =========================================================================
// Mouse routing
// =========================================================================

/// Route mouse events to the appropriate widget or message.
pub(super) fn on_mouse(
    state: &NexusState,
    event: MouseEvent,
    hit: Option<HitResult>,
    capture: &CaptureState,
) -> MouseResponse<NexusMessage> {
    // ── Drag state machine intercept ──────────────────────────────
    if let Some(resp) = super::drag_state::route_drag_mouse(
        &state.drag.status,
        &event,
        hit.clone(),
        &state.drag.auto_scroll,
        state.scroll.state.bounds.get(),
    ) {
        return resp;
    }

    // Composable scroll + input handlers
    route_mouse!(&event, &hit, capture, [
        state.input.completion.scroll       => |a| NexusMessage::Input(InputMsg::CompletionScroll(a)),
        state.input.history_search.scroll   => |a| NexusMessage::Input(InputMsg::HistorySearchScroll(a)),
        state.scroll.state                  => NexusMessage::Scroll,
        state.agent.question_input          => |a| NexusMessage::Agent(AgentMsg::QuestionInputMouse(a)),
        state.input.text_input              => |a| NexusMessage::Input(InputMsg::Mouse(a)),
    ]);

    // Right-click → context menu
    if let MouseEvent::ButtonPressed {
        button: MouseButton::Right,
        position,
        ..
    } = &event
    {
        return route_right_click(state, position, &hit);
    }

    // Hover tracking (delegated to children + context menu)
    if let MouseEvent::CursorMoved { .. } = &event {
        route_hover(state, &hit);
    }

    // Context menu item clicks (root handles — transient UI)
    if let MouseEvent::ButtonPressed {
        button: MouseButton::Left,
        ..
    } = &event
    {
        if let Some(msg) = route_context_menu_click(state, &hit) {
            return MouseResponse::message(msg);
        }
    }

    // Widget clicks → delegate to children
    if let MouseEvent::ButtonPressed {
        button: MouseButton::Left,
        position,
        ..
    } = &event
    {
        eprintln!("[DEBUG click] hit={:?}", hit);
        // Z-order: Selection > Anchor > Text
        // If clicking inside an existing non-collapsed selection, start a selection drag.
        if let Some((source, origin_addr)) = state.selection.hit_in_selection(
            &hit,
            &state.shell.blocks,
            &state.agent.blocks,
        ) {
            let text = state
                .selection
                .extract_selected_text(&state.shell.blocks, &state.agent.blocks)
                .unwrap_or_default();
            let intent = PendingIntent::SelectionDrag {
                source,
                text,
                origin_addr,
            };
            return MouseResponse::message_and_capture(
                NexusMessage::Drag(DragMsg::Start(intent, *position)),
                source,
            );
        }

        if let Some(HitResult::Widget(id)) = &hit {
            // Viewer exit buttons (cross-cutting: shell block → ViewerMsg)
            for block in &state.shell.blocks {
                if block.view_state.is_some() && *id == source_ids::viewer_exit(block.id) {
                    return MouseResponse::message(NexusMessage::Viewer(ViewerMsg::Exit(block.id)));
                }
            }

            // Try each child in order
            if let Some(msg) = state.input.on_click(*id) {
                return MouseResponse::message(NexusMessage::Input(msg));
            }
            if let Some(msg) = state.shell.on_click(*id) {
                return MouseResponse::message(NexusMessage::Shell(msg));
            }
            if let Some(msg) = state.agent.on_click(*id) {
                return MouseResponse::message(NexusMessage::Agent(msg));
            }

            // Anchor clicks → start pending drag (click fires on release if <5px)
            if let Some(payload) = state.shell.drag_payload_for_anchor(*id) {
                // No source_rect - Quick Look will appear without zoom animation
                // (zoom animation is designed for thumbnails, not text)
                let intent = PendingIntent::Anchor { source: *id, payload, source_rect: None };
                return MouseResponse::message_and_capture(
                    NexusMessage::Drag(DragMsg::Start(intent, *position)),
                    *id,
                );
            }

            // Job pills (cross-cutting: shell data → root scroll action)
            for job in &state.shell.jobs {
                if *id == JobBar::job_pill_id(job.id) {
                    return MouseResponse::message(NexusMessage::ScrollToJob(job.id));
                }
            }
        }

        // Image output — start a pending drag (native OS drag on threshold)
        {
            let image_source = match &hit {
                Some(HitResult::Widget(id)) => Some(*id),
                Some(HitResult::Content(addr)) => Some(addr.source_id),
                None => None,
            };
            if let Some(src) = image_source {
                if let Some(payload) = state.shell.image_drag_payload(src) {
                    let intent = PendingIntent::Anchor { source: src, payload, source_rect: None };
                    return MouseResponse::message_and_capture(
                        NexusMessage::Drag(DragMsg::Start(intent, *position)),
                        src,
                    );
                }
            }
        }

        // Text content selection — immediate Active(Selecting), no hysteresis
        if let Some(HitResult::Content(addr)) = hit {
            let mode = state.drag.click_tracker.register_click(
                *position,
                std::time::Instant::now(),
            );
            let capture_source = addr.source_id;
            return MouseResponse::message_and_capture(
                NexusMessage::Drag(DragMsg::StartSelecting(addr, mode)),
                capture_source,
            );
        }

        // Click landed on a shell block area (e.g. between rows, header,
        // viewer background) but didn't match any specific widget handler
        // above.  Focus the block so keyboard input routes to its PTY.
        let hit_source = match &hit {
            Some(HitResult::Widget(id)) => Some(*id),
            Some(HitResult::Content(addr)) => Some(addr.source_id),
            None => None,
        };
        if let Some(src) = hit_source {
            if let Some(block_id) = state.shell.block_for_source(src) {
                return MouseResponse::message(NexusMessage::FocusBlock(block_id));
            }
        }

        // Clicked empty space: blur inputs
        return MouseResponse::message(NexusMessage::BlurAll);
    }

    MouseResponse::none()
}

// =========================================================================
// Mouse routing helpers
// =========================================================================

fn route_right_click(
    state: &NexusState,
    position: &strata::primitives::Point,
    hit: &Option<HitResult>,
) -> MouseResponse<NexusMessage> {
    let (x, y) = (position.x, position.y);

    // Input area right-click
    if let Some(msg) = state.input.context_menu(x, y) {
        return MouseResponse::message(NexusMessage::ContextMenu(msg));
    }

    // Content area right-click — delegate to children
    if let Some(HitResult::Content(addr)) = hit {
        if let Some(msg) = state.shell.context_menu_for_source(addr.source_id, x, y) {
            return MouseResponse::message(NexusMessage::ContextMenu(msg));
        }
        if let Some(msg) = state.agent.context_menu_for_source(addr.source_id, x, y) {
            return MouseResponse::message(NexusMessage::ContextMenu(msg));
        }
    }

    // Fallback: right-click on non-content area
    if hit.is_some() {
        if let Some(msg) = state.shell.fallback_context_menu(x, y) {
            return MouseResponse::message(NexusMessage::ContextMenu(msg));
        }
        if let Some(msg) = state.agent.fallback_context_menu(x, y) {
            return MouseResponse::message(NexusMessage::ContextMenu(msg));
        }
    }

    MouseResponse::none()
}

fn route_hover(state: &NexusState, hit: &Option<HitResult>) {
    // Context menu hover (transient UI — stays at root)
    if let Some(menu) = state.transient.context_menu() {
        let idx = if let Some(HitResult::Widget(id)) = hit {
            (0..menu.items.len()).find(|i| *id == source_ids::ctx_menu_item(*i))
        } else {
            None
        };
        menu.hovered_item.set(idx);
    }

    // Input-owned hover tracking (completion, history search)
    state.input.on_hover(hit);
}

fn route_context_menu_click(state: &NexusState, hit: &Option<HitResult>) -> Option<NexusMessage> {
    let menu = state.transient.context_menu()?;
    if let Some(HitResult::Widget(id)) = hit {
        for (i, item) in menu.items.iter().enumerate() {
            if *id == source_ids::ctx_menu_item(i) {
                return Some(NexusMessage::ContextMenu(ContextMenuMsg::Action(item.clone())));
            }
        }
    }
    Some(NexusMessage::ContextMenu(ContextMenuMsg::Dismiss))
}

