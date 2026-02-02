//! Event routing — dispatches keyboard and mouse events to the appropriate widgets.

use strata::event_context::{
    CaptureState, Key, KeyEvent, MouseButton, MouseEvent, NamedKey,
};
use strata::layout_snapshot::HitResult;
use strata::{MouseResponse, ScrollAction, route_mouse};

use crate::blocks::Focus;
use crate::nexus_widgets::JobBar;

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

    // Phase 0: Active drag — Escape cancels
    if state.drag.is_active() && matches!(key, Key::Named(NamedKey::Escape)) {
        return Some(NexusMessage::Drag(DragMsg::Cancel));
    }

    // Phase 1: Input overlay intercepts (history search, completion)
    if state.input.captures_keys() {
        return state.input.on_key(&event).map(NexusMessage::Input);
    }

    // Phase 2: Global shortcuts
    if let Some(msg) = route_global_shortcut(state, key, modifiers) {
        return Some(msg);
    }

    // Phase 3: Escape cascade
    if matches!(key, Key::Named(NamedKey::Escape)) {
        return route_escape(state);
    }

    // Phase 4: Focus-based routing
    match state.focus {
        Focus::Block(id) => {
            // If a viewer is active on this block, dispatch to viewer keybindings
            if let Some(block) = state.shell.block_by_id(id) {
                if let Some(ref view_state) = block.view_state {
                    if let Some(viewer_msg) = view_state.handle_key(id, key) {
                        return Some(NexusMessage::Viewer(viewer_msg));
                    }
                    // Viewer consumed the focus; don't pass to PTY
                    return None;
                }
            }
            return Some(NexusMessage::Shell(ShellMsg::PtyInput(id, event)));
        }
        Focus::AgentInput => {
            return Some(NexusMessage::Agent(AgentMsg::QuestionInputKey(event)));
        }
        Focus::Input => {} // fall through to input widget
    }

    // Phase 5: Input-focused keys (delegated to InputWidget)
    if let Some(msg) = state.input.on_key(&event) {
        return Some(NexusMessage::Input(msg));
    }

    // Phase 6: Global fallback
    route_global_fallback(key)
}

fn route_global_shortcut(
    state: &NexusState,
    key: &Key,
    modifiers: &strata::event_context::Modifiers,
) -> Option<NexusMessage> {
    if modifiers.meta {
        if let Key::Character(c) = key {
            match c.as_str() {
                "q" | "w" => return Some(NexusMessage::CloseWindow),
                "k" => return Some(NexusMessage::ClearScreen),
                "c" => return Some(NexusMessage::Copy),
                "v" => return Some(NexusMessage::Paste),
                "." => return Some(NexusMessage::Input(InputMsg::ToggleMode)),
                _ => {}
            }
        }
    }

    if modifiers.ctrl {
        if let Key::Character(c) = key {
            match c.as_str() {
                "r" => return Some(NexusMessage::Input(InputMsg::HistorySearchToggle)),
                "c" => {
                    if state.agent.is_active() {
                        return Some(NexusMessage::Agent(AgentMsg::Interrupt));
                    }
                    // Ctrl+C exits active viewers (top, less, man, tree)
                    if let Focus::Block(id) = state.focus {
                        if let Some(block) = state.shell.block_by_id(id) {
                            if block.view_state.is_some() {
                                return Some(NexusMessage::Viewer(ViewerMsg::Exit(id)));
                            }
                        }
                    }
                    // Fallback: exit any active viewer even when focus is Input
                    if let Some(id) = state.shell.active_viewer_block() {
                        return Some(NexusMessage::Viewer(ViewerMsg::Exit(id)));
                    }
                    let focused = match state.focus {
                        Focus::Block(id) => Some(id),
                        _ => None,
                    };
                    if let Some(id) = state.shell.interrupt_target(focused) {
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

fn route_escape(state: &NexusState) -> Option<NexusMessage> {
    // Close Quick Look on Esc (always, before other handlers)
    strata::platform::close_quicklook();

    if state.transient.context_menu().is_some() {
        return Some(NexusMessage::ContextMenu(ContextMenuMsg::Dismiss));
    }
    if state.agent.is_active() {
        return Some(NexusMessage::Agent(AgentMsg::Interrupt));
    }
    // Escape exits active viewers (top, less, man, tree)
    if let Focus::Block(id) = state.focus {
        if let Some(block) = state.shell.block_by_id(id) {
            if block.view_state.is_some() {
                return Some(NexusMessage::Viewer(ViewerMsg::Exit(id)));
            }
        }
        return Some(NexusMessage::BlurAll);
    }
    // Fallback: exit any active viewer even when focus is Input
    if let Some(id) = state.shell.active_viewer_block() {
        return Some(NexusMessage::Viewer(ViewerMsg::Exit(id)));
    }
    if state.selection.selection.is_some() {
        return Some(NexusMessage::Selection(SelectionMsg::Clear));
    }
    None
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
    // Close Quick Look on any mouse click in the app window
    if matches!(event, MouseEvent::ButtonPressed { .. }) {
        strata::platform::close_quicklook();
    }

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

        // Clicked empty space: blur inputs
        if matches!(state.focus, Focus::Input) {
            return MouseResponse::message(NexusMessage::BlurAll);
        }
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

