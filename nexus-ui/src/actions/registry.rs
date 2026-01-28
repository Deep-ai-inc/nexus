//! Action Registry - central store for all actions.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers as IcedModifiers};
use iced::Task;

use crate::blocks::{Focus, InputMode};
use crate::msg::{AgentMessage, InputMessage, Message};
use crate::state::Nexus;

use super::types::{Action, ActionContext, ActionId, KeyCombo, KeySpec, Modifiers};

/// Central registry of all actions.
pub struct ActionRegistry {
    actions: Vec<Action>,
    by_id: HashMap<&'static str, usize>,
}

impl ActionRegistry {
    /// Create a new registry with all built-in actions.
    pub fn new() -> Self {
        let mut registry = Self {
            actions: Vec::new(),
            by_id: HashMap::new(),
        };
        registry.register_builtin_actions();
        registry
    }

    /// Register an action.
    fn register(&mut self, action: Action) {
        let idx = self.actions.len();
        self.by_id.insert(action.id.0, idx);
        self.actions.push(action);
    }

    /// Get an action by ID.
    pub fn get(&self, id: ActionId) -> Option<&Action> {
        self.by_id.get(id.0).map(|&idx| &self.actions[idx])
    }

    /// Get all actions.
    pub fn all(&self) -> &[Action] {
        &self.actions
    }

    /// Find actions matching a search query (fuzzy).
    pub fn search(&self, query: &str, state: &Nexus) -> Vec<&Action> {
        let query_lower = query.to_lowercase();
        let mut matches: Vec<_> = self.actions
            .iter()
            .filter(|a| a.available(state))
            .filter(|a| a.search_text().contains(&query_lower))
            .collect();

        // Sort by relevance: exact name match first, then by name length
        matches.sort_by(|a, b| {
            let a_exact = a.name.to_lowercase() == query_lower;
            let b_exact = b.name.to_lowercase() == query_lower;
            match (a_exact, b_exact) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.len().cmp(&b.name.len()),
            }
        });

        matches
    }

    /// Find action matching a key combo.
    pub fn find_by_key(&self, key: &Key, modifiers: &IcedModifiers) -> Option<&Action> {
        self.actions.iter().find(|a| {
            a.keybinding.as_ref().map_or(false, |kb| kb.matches(key, modifiers))
        })
    }

    /// Get actions available in a given context.
    pub fn for_context(&self, context: ActionContext, state: &Nexus) -> Vec<&Action> {
        self.actions
            .iter()
            .filter(|a| a.context == context || a.context == ActionContext::Global)
            .filter(|a| a.available(state))
            .collect()
    }

    /// Register all built-in actions.
    fn register_builtin_actions(&mut self) {
        // =====================================================================
        // Global Actions
        // =====================================================================

        self.register(Action {
            id: ActionId("clear_screen"),
            name: "Clear Screen",
            description: "Clear all blocks and reset the session",
            keywords: &["reset", "clean", "wipe"],
            keybinding: Some(KeyCombo::new(Modifiers::CMD, KeySpec::char('k'))),
            context: ActionContext::Global,
            is_enabled: |_| true,
            execute: action_clear_screen,
        });

        self.register(Action {
            id: ActionId("quit"),
            name: "Quit",
            description: "Exit the application",
            keywords: &["exit", "close", "bye"],
            keybinding: Some(KeyCombo::new(Modifiers::CMD, KeySpec::char('q'))),
            context: ActionContext::Global,
            is_enabled: |_| true,
            execute: |_| iced::exit(),
        });

        self.register(Action {
            id: ActionId("close_window"),
            name: "Close Window",
            description: "Close the current window",
            keywords: &["exit"],
            keybinding: Some(KeyCombo::new(Modifiers::CMD, KeySpec::char('w'))),
            context: ActionContext::Global,
            is_enabled: |_| true,
            execute: |_| iced::exit(),
        });

        self.register(Action {
            id: ActionId("zoom_in"),
            name: "Zoom In",
            description: "Increase font size",
            keywords: &["bigger", "larger", "font"],
            keybinding: Some(KeyCombo::new(Modifiers::CMD, KeySpec::char('='))),
            context: ActionContext::Global,
            is_enabled: |_| true,
            execute: action_zoom_in,
        });

        self.register(Action {
            id: ActionId("zoom_out"),
            name: "Zoom Out",
            description: "Decrease font size",
            keywords: &["smaller", "font"],
            keybinding: Some(KeyCombo::new(Modifiers::CMD, KeySpec::char('-'))),
            context: ActionContext::Global,
            is_enabled: |_| true,
            execute: action_zoom_out,
        });

        self.register(Action {
            id: ActionId("zoom_reset"),
            name: "Reset Zoom",
            description: "Reset font size to default",
            keywords: &["font", "default"],
            keybinding: Some(KeyCombo::new(Modifiers::CMD, KeySpec::char('0'))),
            context: ActionContext::Global,
            is_enabled: |_| true,
            execute: action_zoom_reset,
        });

        // =====================================================================
        // Input Mode Actions
        // =====================================================================

        self.register(Action {
            id: ActionId("toggle_mode"),
            name: "Toggle Mode",
            description: "Switch between Shell and Agent input modes",
            keywords: &["shell", "agent", "ai", "switch"],
            keybinding: Some(KeyCombo::new(Modifiers::CMD, KeySpec::char('.'))),
            context: ActionContext::Global,
            is_enabled: |_| true,
            execute: action_toggle_mode,
        });

        self.register(Action {
            id: ActionId("command_palette"),
            name: "Command Palette",
            description: "Open the command palette to search and run actions",
            keywords: &["search", "find", "actions", "commands"],
            keybinding: Some(KeyCombo::new(Modifiers::CMD, KeySpec::char('p'))),
            context: ActionContext::Global,
            is_enabled: |state| !state.input.palette_visible,
            execute: |_| Task::done(Message::Input(InputMessage::PaletteOpen)),
        });

        self.register(Action {
            id: ActionId("history_search"),
            name: "Search History",
            description: "Search command history",
            keywords: &["find", "previous", "ctrl-r"],
            keybinding: Some(KeyCombo::new(Modifiers::CTRL, KeySpec::char('r'))),
            context: ActionContext::InputFocused,
            is_enabled: |state| matches!(state.terminal.focus, Focus::Input),
            execute: |_| Task::done(Message::Input(InputMessage::HistorySearchStart)),
        });

        // =====================================================================
        // Clipboard Actions
        // =====================================================================

        self.register(Action {
            id: ActionId("copy"),
            name: "Copy",
            description: "Copy selected text to clipboard",
            keywords: &["clipboard", "yank"],
            keybinding: Some(KeyCombo::new(Modifiers::CMD, KeySpec::char('c'))),
            context: ActionContext::Global,
            is_enabled: |_| true,
            execute: action_copy,
        });

        self.register(Action {
            id: ActionId("paste"),
            name: "Paste",
            description: "Paste from clipboard",
            keywords: &["clipboard"],
            keybinding: Some(KeyCombo::new(Modifiers::CMD, KeySpec::char('v'))),
            context: ActionContext::Global,
            is_enabled: |_| true,
            execute: action_paste,
        });

        // =====================================================================
        // Agent Actions
        // =====================================================================

        self.register(Action {
            id: ActionId("interrupt_agent"),
            name: "Interrupt Agent",
            description: "Stop the running AI agent",
            keywords: &["stop", "cancel", "halt", "abort"],
            keybinding: Some(KeyCombo::new(Modifiers::NONE, KeySpec::named(Named::Escape))),
            context: ActionContext::AgentRunning,
            is_enabled: |state| state.agent.active_block.is_some(),
            execute: |_| Task::done(Message::Agent(AgentMessage::Interrupt)),
        });

        self.register(Action {
            id: ActionId("cancel_input"),
            name: "Cancel",
            description: "Clear the input line",
            keywords: &["clear", "reset"],
            keybinding: Some(KeyCombo::new(Modifiers::CTRL, KeySpec::char('c'))),
            context: ActionContext::InputFocused,
            is_enabled: |state| {
                matches!(state.terminal.focus, Focus::Input)
                    && state.agent.active_block.is_none()
            },
            execute: action_cancel_input,
        });

        // =====================================================================
        // Terminal Actions
        // =====================================================================

        self.register(Action {
            id: ActionId("retry_sudo"),
            name: "Retry with Sudo",
            description: "Re-run the last command with sudo",
            keywords: &["permission", "root", "admin", "elevate"],
            keybinding: Some(KeyCombo::new(Modifiers::CTRL, KeySpec::char('s'))),
            context: ActionContext::InputFocused,
            is_enabled: |state| state.terminal.permission_denied_command.is_some(),
            execute: action_retry_sudo,
        });

        // =====================================================================
        // Long-Tail Utility Actions
        // =====================================================================

        self.register(Action {
            id: ActionId("copy_last_output"),
            name: "Copy Last Output",
            description: "Copy the output of the last command to clipboard",
            keywords: &["clipboard", "stdout", "result", "previous"],
            keybinding: None, // Palette-only action
            context: ActionContext::Global,
            is_enabled: |state| {
                // Enabled if there's at least one completed block
                state.terminal.blocks.iter().any(|b| !b.is_running())
            },
            execute: action_copy_last_output,
        });

        self.register(Action {
            id: ActionId("restart_session"),
            name: "Restart Session",
            description: "Clear all blocks and reset the session without closing the window",
            keywords: &["reset", "fresh", "new", "clear", "wipe"],
            keybinding: None, // Palette-only action
            context: ActionContext::Global,
            is_enabled: |_| true,
            execute: action_restart_session,
        });

        self.register(Action {
            id: ActionId("find_in_buffer"),
            name: "Find in Buffer",
            description: "Search terminal output for text",
            keywords: &["search", "grep", "filter", "text", "find"],
            keybinding: Some(KeyCombo::new(Modifiers::CMD, KeySpec::char('f'))),
            context: ActionContext::Global,
            is_enabled: |_| true,
            execute: action_find_in_buffer,
        });
    }
}

impl Default for ActionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Action Implementations
// =============================================================================

fn action_clear_screen(state: &mut Nexus) -> Task<Message> {
    state.agent.cancel_flag.store(true, Ordering::SeqCst);
    state.terminal.blocks.clear();
    state.terminal.block_index.clear();
    state.agent.blocks.clear();
    state.agent.block_index.clear();
    state.agent.active_block = None;
    state.agent.session_id = None;
    Task::none()
}

fn action_toggle_mode(state: &mut Nexus) -> Task<Message> {
    state.input.mode = match state.input.mode {
        InputMode::Shell => InputMode::Agent,
        InputMode::Agent => InputMode::Shell,
    };
    state.input.suppress_char = Some('.');
    Task::none()
}

fn action_zoom_in(state: &mut Nexus) -> Task<Message> {
    let new_size = (state.window.font_size + 1.0).min(32.0);
    state.input.suppress_char = Some('=');
    apply_zoom(state, new_size)
}

fn action_zoom_out(state: &mut Nexus) -> Task<Message> {
    let new_size = (state.window.font_size - 1.0).max(8.0);
    state.input.suppress_char = Some('-');
    apply_zoom(state, new_size)
}

fn action_zoom_reset(state: &mut Nexus) -> Task<Message> {
    use crate::constants::DEFAULT_FONT_SIZE;
    state.input.suppress_char = Some('0');
    apply_zoom(state, DEFAULT_FONT_SIZE)
}

/// Helper to apply zoom and resize window.
fn apply_zoom(state: &mut Nexus, new_size: f32) -> Task<Message> {
    use crate::constants::{CHAR_WIDTH_RATIO, LINE_HEIGHT_FACTOR};

    state.window.font_size = new_size;

    let (cols, rows) = state.terminal.terminal_size;
    let new_width = (cols as f32 * new_size * CHAR_WIDTH_RATIO) + 16.0;
    let new_height = (rows as f32 * new_size * LINE_HEIGHT_FACTOR) + 60.0;
    state.window.dims = (new_width, new_height);

    if let Some(window_id) = state.window.id {
        return iced::window::resize(window_id, iced::Size::new(new_width, new_height));
    }
    Task::none()
}

fn action_copy(state: &mut Nexus) -> Task<Message> {
    // TODO: Once selection model exists, copy selected text
    // For now, copy input text
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(&state.input.text());
    }
    state.input.suppress_char = Some('c');
    Task::none()
}

fn action_paste(state: &mut Nexus) -> Task<Message> {
    use image::{ImageBuffer, RgbaImage};

    state.input.suppress_char = Some('v');

    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        // Try clipboard image data (screenshots)
        if let Ok(img) = clipboard.get_image() {
            let width = img.width as u32;
            let height = img.height as u32;

            let mut png_data = Vec::new();
            {
                let img_buf: RgbaImage =
                    ImageBuffer::from_raw(width, height, img.bytes.into_owned())
                        .unwrap_or_else(|| ImageBuffer::new(1, 1));

                img_buf
                    .write_to(
                        &mut std::io::Cursor::new(&mut png_data),
                        image::ImageFormat::Png,
                    )
                    .ok();
            }

            if !png_data.is_empty() {
                return Task::done(Message::Input(InputMessage::PasteImage(
                    png_data, width, height,
                )));
            }
        }
        // Text paste is handled by iced's TextInput natively
    }
    Task::none()
}

fn action_cancel_input(state: &mut Nexus) -> Task<Message> {
    // If agent is running, interrupt it instead
    if state.agent.active_block.is_some() {
        return Task::done(Message::Agent(AgentMessage::Interrupt));
    }

    state.input.clear();
    state.input.shell_history_index = None;
    state.input.agent_history_index = None;
    state.input.saved_input.clear();
    state.input.search_active = false;
    state.terminal.permission_denied_command = None;
    Task::none()
}

fn action_retry_sudo(state: &mut Nexus) -> Task<Message> {
    if let Some(ref cmd) = state.terminal.permission_denied_command.clone() {
        state.terminal.permission_denied_command = None;
        // Execute command with sudo
        return Task::done(Message::Input(InputMessage::SetText(format!("sudo {}", cmd))))
            .chain(Task::done(Message::Input(InputMessage::Submit)));
    }
    Task::none()
}

fn action_copy_last_output(state: &mut Nexus) -> Task<Message> {
    // Find the last completed block (reverse search)
    if let Some(block) = state.terminal.blocks.iter().rev().find(|b| !b.is_running()) {
        // Extract text from the terminal grid (includes scrollback)
        let output = block.parser.grid_with_scrollback().to_string();
        // Trim trailing whitespace/newlines
        let output = output.trim_end();

        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(output);
        }
    }
    Task::none()
}

fn action_restart_session(state: &mut Nexus) -> Task<Message> {
    // Cancel any running agent
    state.agent.cancel_flag.store(true, Ordering::SeqCst);

    // Clear terminal blocks
    state.terminal.blocks.clear();
    state.terminal.block_index.clear();

    // Clear agent blocks
    state.agent.blocks.clear();
    state.agent.block_index.clear();
    state.agent.active_block = None;
    state.agent.session_id = None;

    // Reset input state
    state.input.clear();
    state.input.shell_history_index = None;
    state.input.agent_history_index = None;
    state.input.saved_input.clear();
    state.input.search_active = false;
    state.input.completion_visible = false;

    // Reset terminal error states
    state.terminal.permission_denied_command = None;
    state.terminal.command_not_found = None;
    state.terminal.last_exit_code = None;

    Task::none()
}

fn action_find_in_buffer(state: &mut Nexus) -> Task<Message> {
    state.input.suppress_char = Some('f');
    Task::done(Message::Input(InputMessage::BufferSearchOpen))
}
