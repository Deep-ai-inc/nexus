//! Window domain handler.
//!
//! Handles window resize, zoom, global shortcuts, and event routing.
//!
//! Keyboard shortcuts are dispatched through the ActionRegistry, which
//! centralizes all user-invokable actions with metadata for the command
//! palette, keybindings, and context menus.

use iced::keyboard;
use iced::{Event, Task};

use crate::actions::{ActionId, ActionRegistry};
use crate::blocks::Focus;
use crate::msg::{GlobalShortcut, Message, WindowMessage, ZoomDirection};
use crate::state::Nexus;

// Lazy-initialized action registry (shared across all handler calls)
use std::sync::OnceLock;
static ACTION_REGISTRY: OnceLock<ActionRegistry> = OnceLock::new();

fn get_registry() -> &'static ActionRegistry {
    ACTION_REGISTRY.get_or_init(ActionRegistry::new)
}

/// Update the window domain state.
pub fn update(state: &mut Nexus, msg: WindowMessage) -> Task<Message> {
    match msg {
        WindowMessage::Event(evt, id) => handle_event(state, evt, id),
        WindowMessage::Resized(w, h) => resize(state, w, h),
        WindowMessage::Shortcut(sc) => dispatch_shortcut(state, sc),
        WindowMessage::Zoom(dir) => dispatch_zoom(state, dir),
        WindowMessage::BackgroundClicked => {
            // Don't steal focus from a running PTY
            if let Focus::Block(block_id) = state.terminal.focus {
                if state.terminal.pty_handles.iter().any(|h| h.block_id == block_id) {
                    return Task::none();
                }
            }
            state.terminal.focus = Focus::Input;
            // Ensure text_editor has iced focus in case user clicked elsewhere
            iced::widget::focus_next()
        }
    }
}

/// Dispatch a shortcut message through the action registry.
fn dispatch_shortcut(state: &mut Nexus, shortcut: GlobalShortcut) -> Task<Message> {
    let action_id = match shortcut {
        GlobalShortcut::ClearScreen => ActionId("clear_screen"),
        GlobalShortcut::CloseWindow => ActionId("close_window"),
        GlobalShortcut::Quit => ActionId("quit"),
        GlobalShortcut::Copy => ActionId("copy"),
        GlobalShortcut::Paste => ActionId("paste"),
    };

    let registry = get_registry();
    if let Some(action) = registry.get(action_id) {
        if action.available(state) {
            return action.run(state);
        }
    }
    Task::none()
}

/// Dispatch a zoom message through the action registry.
fn dispatch_zoom(state: &mut Nexus, direction: ZoomDirection) -> Task<Message> {
    let action_id = match direction {
        ZoomDirection::In => ActionId("zoom_in"),
        ZoomDirection::Out => ActionId("zoom_out"),
        ZoomDirection::Reset => ActionId("zoom_reset"),
    };

    let registry = get_registry();
    if let Some(action) = registry.get(action_id) {
        if action.available(state) {
            return action.run(state);
        }
    }
    Task::none()
}

// =============================================================================
// Event Routing
// =============================================================================

/// Handle all window/keyboard events and route to appropriate handlers.
pub fn handle_event(
    state: &mut Nexus,
    event: Event,
    window_id: iced::window::Id,
) -> Task<Message> {
    // Capture window ID
    if state.window.id.is_none() {
        state.window.id = Some(window_id);
    }

    match event {
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            // Try to dispatch through the action registry first
            let registry = get_registry();
            if let Some(action) = registry.find_by_key(&key, &modifiers) {
                if action.available(state) {
                    return action.run(state);
                }
            }

            // Focus-dependent key handling (not in registry - these are context-specific)
            match state.terminal.focus {
                Focus::Input => {
                    if let Some(input_msg) = super::input::handle_focus_key(
                        &mut state.input,
                        key,
                        modifiers,
                    ) {
                        return Task::done(Message::Input(input_msg));
                    }
                }
                Focus::Block(_) => {
                    return super::terminal::handle_key(state, key, modifiers);
                }
            }
        }
        Event::Window(iced::window::Event::Resized(size)) => {
            return resize(state, size.width as u32, size.height as u32);
        }
        _ => {}
    }
    Task::none()
}

// =============================================================================
// Window Resize
// =============================================================================

/// Handle window resize.
pub fn resize(state: &mut Nexus, width: u32, height: u32) -> Task<Message> {
    state.window.dims = (width as f32, height as f32);
    let old_cols = state.terminal.terminal_size.0;
    let (cols, rows) = state.recalculate_terminal_size();
    if cols != old_cols {
        state.apply_resize(cols, rows);
    }
    Task::none()
}

// =============================================================================
// Action Registry Access
// =============================================================================

/// Get the global action registry (for command palette, etc.)
pub fn action_registry() -> &'static ActionRegistry {
    get_registry()
}
