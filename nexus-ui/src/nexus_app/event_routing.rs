//! Event routing — dispatches keyboard and mouse events to the appropriate widgets.

use strata::event_context::{
    CaptureState, Key, KeyEvent, MouseButton, MouseEvent, NamedKey,
};
use strata::layout_snapshot::HitResult;
use strata::{MouseResponse, ScrollAction, route_mouse};

use crate::blocks::Focus;
use crate::nexus_widgets::JobBar;

use super::drag_state::{ActiveKind, DragStatus, PendingIntent, DRAG_THRESHOLD_SQ};
use super::message::{
    AgentMsg, ContextMenuMsg, DragMsg, InputMsg, NexusMessage, SelectionMsg, ShellMsg,
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

    // Phase 4: Focused PTY block → forward keys to shell
    if let Focus::Block(id) = state.focus {
        return Some(NexusMessage::Shell(ShellMsg::PtyInput(id, event)));
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
    if state.transient.context_menu().is_some() {
        return Some(NexusMessage::ContextMenu(ContextMenuMsg::Dismiss));
    }
    if state.agent.is_active() {
        return Some(NexusMessage::Agent(AgentMsg::Interrupt));
    }
    if matches!(state.focus, Focus::Block(_)) {
        return Some(NexusMessage::BlurAll);
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
    // ── Drag state machine intercept ──────────────────────────────
    // When a drag is Pending or Active, all mouse events are routed here.
    match &state.drag.status {
        DragStatus::Active(kind) => {
            return match kind {
                ActiveKind::Drag(_) => match &event {
                    MouseEvent::CursorMoved { position, .. } => {
                        update_auto_scroll(state, position);
                        MouseResponse::message(NexusMessage::Drag(DragMsg::Move(*position)))
                    }
                    MouseEvent::ButtonReleased {
                        button: MouseButton::Left,
                        ..
                    } => {
                        state.drag.auto_scroll.set(None);
                        let zone = super::file_drop::resolve_drop_zone(state, &hit);
                        MouseResponse::message_and_release(NexusMessage::Drag(DragMsg::Drop(zone)))
                    }
                    MouseEvent::CursorLeft => {
                        state.drag.auto_scroll.set(None);
                        // Cursor left the window during an active drag → hand off to OS
                        MouseResponse::message_and_release(NexusMessage::Drag(DragMsg::GoOutbound))
                    }
                    _ => MouseResponse::none(),
                },
                ActiveKind::Selecting { .. } => match &event {
                    MouseEvent::CursorMoved { position, .. } => {
                        update_auto_scroll(state, position);
                        if let Some(HitResult::Content(addr)) = hit {
                            MouseResponse::message(NexusMessage::Selection(SelectionMsg::Extend(addr)))
                        } else {
                            MouseResponse::none()
                        }
                    }
                    MouseEvent::ButtonReleased {
                        button: MouseButton::Left,
                        ..
                    } => {
                        state.drag.auto_scroll.set(None);
                        // Cancel resets drag status to Inactive; dispatch_drag emits SelectionMsg::End
                        MouseResponse::message_and_release(NexusMessage::Drag(DragMsg::Cancel))
                    }
                    MouseEvent::CursorLeft => {
                        state.drag.auto_scroll.set(None);
                        MouseResponse::message_and_release(NexusMessage::Drag(DragMsg::Cancel))
                    }
                    _ => MouseResponse::none(),
                },
            };
        }
        DragStatus::Pending { origin, .. } => {
            return match &event {
                MouseEvent::CursorMoved { position, .. } => {
                    let dx = position.x - origin.x;
                    let dy = position.y - origin.y;
                    if dx * dx + dy * dy > DRAG_THRESHOLD_SQ {
                        MouseResponse::message(NexusMessage::Drag(DragMsg::Activate(*position)))
                    } else {
                        MouseResponse::none()
                    }
                }
                MouseEvent::ButtonReleased {
                    button: MouseButton::Left,
                    ..
                } => {
                    // Cancel pending drag → the handler re-dispatches the original click
                    MouseResponse::message(NexusMessage::Drag(DragMsg::Cancel))
                }
                _ => MouseResponse::none(),
            };
        }
        DragStatus::Inactive => {} // Fall through to normal routing
    }

    // Composable scroll + input handlers
    route_mouse!(&event, &hit, capture, [
        state.input.completion.scroll       => |a| NexusMessage::Input(InputMsg::CompletionScroll(a)),
        state.input.history_search.scroll   => |a| NexusMessage::Input(InputMsg::HistorySearchScroll(a)),
        state.scroll.state                  => NexusMessage::Scroll,
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
        // This applies to both Content hits (raw text) and Widget hits (anchors inside
        // selection) — clicking a link within selected text should drag the selection,
        // not open the link.
        if let Some(ref sel) = state.selection.selection {
            if !sel.is_collapsed() {
                let hit_in_selection = match &hit {
                    Some(HitResult::Content(addr)) => {
                        let ordering = super::selection::build_source_ordering(
                            &state.shell.blocks,
                            &state.agent.blocks,
                        );
                        if sel.contains(addr, &ordering) {
                            Some((addr.source_id, addr.clone()))
                        } else {
                            None
                        }
                    }
                    Some(HitResult::Widget(id)) => {
                        let ordering = super::selection::build_source_ordering(
                            &state.shell.blocks,
                            &state.agent.blocks,
                        );
                        // Widget's SourceId within selection range → treat as selection drag
                        if sel.sources(&ordering).contains(id) {
                            Some((*id, strata::content_address::ContentAddress::start_of(*id)))
                        } else {
                            None
                        }
                    }
                    None => None,
                };

                if let Some((source, origin_addr)) = hit_in_selection {
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
            }
        }

        if let Some(HitResult::Widget(id)) = &hit {
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
                let intent = PendingIntent::Anchor { source: *id, payload };
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
        if state.input.text_input.focused {
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

/// Compute auto-scroll speed based on cursor distance from scroll container edges.
///
/// 40px edge zone, proportional speed up to 8px per tick (~480px/s at 60fps).
/// Negative = scroll up (toward top of content), positive = scroll down.
fn update_auto_scroll(state: &NexusState, pos: &strata::primitives::Point) {
    let bounds = state.scroll.state.bounds.get();
    let edge = 40.0;
    let max_speed = 8.0;

    let speed = if pos.y < bounds.y + edge {
        // Near top → scroll up (negative offset = toward top)
        let dist = bounds.y + edge - pos.y;
        -(dist / edge) * max_speed
    } else if pos.y > bounds.y + bounds.height - edge {
        // Near bottom → scroll down (positive offset = toward bottom)
        let dist = pos.y - (bounds.y + bounds.height - edge);
        (dist / edge) * max_speed
    } else {
        0.0
    };

    if speed.abs() > 0.1 {
        state.drag.auto_scroll.set(Some(speed));
    } else {
        state.drag.auto_scroll.set(None);
    }
}
