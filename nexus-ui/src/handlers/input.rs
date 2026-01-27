//! Input domain handler.
//!
//! Handles text input, completion, history search, and attachments.
//! Pure functions take only `&mut InputState`. Cross-domain effects are
//! returned as `Action` values for the coordinator to process.

use std::sync::Arc;

use iced::keyboard::{self, Key, Modifiers};
use iced::widget::text_editor;
use iced::Task;
use tokio::sync::Mutex;

use nexus_kernel::{Completion, Kernel};

use crate::blocks::InputMode;
use crate::msg::{Action, InputMessage, Message};
use crate::state::InputState;

/// Result of processing an input message.
pub struct InputResult {
    pub task: Task<Message>,
    pub actions: Vec<Action>,
}

impl InputResult {
    /// No task, no actions.
    pub fn none() -> Self {
        Self {
            task: Task::none(),
            actions: Vec::new(),
        }
    }

    /// Task only, no actions.
    pub fn task(task: Task<Message>) -> Self {
        Self {
            task,
            actions: Vec::new(),
        }
    }

    /// Single action, no task.
    pub fn action(action: Action) -> Self {
        Self {
            task: Task::none(),
            actions: vec![action],
        }
    }
}

/// Update the input domain state.
/// Takes only InputState and a read-only kernel reference for completion.
/// Returns task and any cross-domain actions needed.
pub fn update(
    input: &mut InputState,
    kernel: &Arc<Mutex<Kernel>>,
    msg: InputMessage,
) -> InputResult {
    // Clear suppress_char on any non-EditorAction message (window has passed)
    if !matches!(msg, InputMessage::EditorAction(_)) {
        input.suppress_char = None;
    }

    match msg {
        // Editor actions (typing, cursor movement, etc.)
        InputMessage::EditorAction(action) => editor_action(input, action),
        InputMessage::ToggleMode => toggle_mode(input),
        InputMessage::CancelCompletion => cancel_completion(input),
        InputMessage::PasteImage(d, w, h) => paste_image(input, d, w, h),
        InputMessage::RemoveAttachment(idx) => remove_attachment(input, idx),
        InputMessage::HistoryKey(key, mods) => history_key(input, key, mods),

        // Operations needing kernel access
        InputMessage::TabCompletion => completion_tab(input, kernel),
        InputMessage::TabCompletionPrev => completion_tab_prev(input),
        InputMessage::SelectCompletion(idx) => select_completion(input, idx),
        InputMessage::CompletionNavigate(idx) => completion_navigate(input, idx),

        // History search (needs kernel)
        InputMessage::HistorySearchStart => super::history::start(input, kernel),
        InputMessage::HistorySearchChanged(q) => super::history::search(input, kernel, q),
        InputMessage::HistorySearchSelect(i) => super::history::select(input, i),
        InputMessage::HistorySearchCancel => super::history::cancel(input),

        // Cross-domain: submit command
        InputMessage::Submit => submit(input),
    }
}

// =============================================================================
// Pure Input Operations
// =============================================================================

/// Handle editor action (typing, cursor movement, etc.).
fn editor_action(input: &mut InputState, action: text_editor::Action) -> InputResult {
    // suppress_char catches shortcut characters that arrive AFTER the KeyPressed handler
    // Only suppress Edit actions that would insert the expected character
    if let Some(ch) = input.suppress_char.take() {
        if let text_editor::Action::Edit(text_editor::Edit::Insert(c)) = &action {
            if *c == ch {
                return InputResult::none();
            }
        }
        // Didn't match - continue with normal processing
    }

    // Clear completion on edit actions
    if matches!(action, text_editor::Action::Edit(_)) {
        if input.completion_visible {
            input.completions.clear();
            input.completion_visible = false;
        }
        input.before_event = input.text();
    }

    input.content.perform(action);
    InputResult::none()
}

/// Toggle between Shell and Agent input modes.
fn toggle_mode(input: &mut InputState) -> InputResult {
    input.mode = match input.mode {
        InputMode::Shell => InputMode::Agent,
        InputMode::Agent => InputMode::Shell,
    };
    InputResult::none()
}

/// Cancel the completion popup.
fn cancel_completion(input: &mut InputState) -> InputResult {
    input.completions.clear();
    input.completion_visible = false;
    InputResult::none()
}

/// Handle pasted image attachment.
fn paste_image(input: &mut InputState, data: Vec<u8>, width: u32, height: u32) -> InputResult {
    let metadata = nexus_api::MediaMetadata {
        width: Some(width),
        height: Some(height),
        duration_secs: None,
        filename: None,
        size: Some(data.len() as u64),
    };
    input.attachments.push(nexus_api::Value::Media {
        data,
        content_type: "image/png".to_string(),
        metadata,
    });
    InputResult::none()
}

/// Remove an attachment by index.
fn remove_attachment(input: &mut InputState, index: usize) -> InputResult {
    if index < input.attachments.len() {
        input.attachments.remove(index);
    }
    InputResult::none()
}

/// Handle arrow keys for history navigation.
/// Uses the appropriate history (shell or agent) based on current mode.
fn history_key(input: &mut InputState, key: Key, _modifiers: Modifiers) -> InputResult {
    // Get history length and index upfront to avoid borrow issues
    let history_len = input.current_history().len();
    let history_index = input.current_history_index();

    match &key {
        Key::Named(keyboard::key::Named::ArrowUp) => {
            if history_len == 0 {
                return InputResult::none();
            }

            let new_index = match history_index {
                None => {
                    input.saved_input = input.text();
                    Some(history_len - 1)
                }
                Some(0) => Some(0),
                Some(i) => Some(i - 1),
            };

            input.set_history_index(new_index);
            if let Some(i) = new_index {
                let text = input.current_history()[i].clone();
                input.set_text(&text);
            }
        }
        Key::Named(keyboard::key::Named::ArrowDown) => {
            match history_index {
                None => {}
                Some(i) if i >= history_len - 1 => {
                    input.set_history_index(None);
                    let saved = input.saved_input.clone();
                    input.set_text(&saved);
                    input.saved_input.clear();
                }
                Some(i) => {
                    input.set_history_index(Some(i + 1));
                    let text = input.current_history()[i + 1].clone();
                    input.set_text(&text);
                }
            }
        }
        _ => {}
    }
    InputResult::none()
}

// =============================================================================
// Completion (needs kernel read access)
// =============================================================================

/// Handle Tab key for completion.
fn completion_tab(input: &mut InputState, kernel: &Arc<Mutex<Kernel>>) -> InputResult {
    // If completion popup is already visible, cycle forward
    if input.completion_visible && !input.completions.is_empty() {
        input.completion_index = (input.completion_index + 1) % input.completions.len();
        return InputResult::none();
    }

    let kernel_guard = kernel.blocking_lock();
    let text = input.text();
    let cursor = text.len();
    let (completions, start) = kernel_guard.complete(&text, cursor);
    drop(kernel_guard);

    apply_completions(input, completions, start)
}

/// Cycle completion backwards (Shift+Tab).
fn completion_tab_prev(input: &mut InputState) -> InputResult {
    if input.completion_visible && !input.completions.is_empty() {
        if input.completion_index == 0 {
            input.completion_index = input.completions.len() - 1;
        } else {
            input.completion_index -= 1;
        }
    }
    InputResult::none()
}

/// Apply completion results to input state.
fn apply_completions(
    input: &mut InputState,
    completions: Vec<Completion>,
    start: usize,
) -> InputResult {
    let text = input.text();
    if completions.len() == 1 {
        let completion = &completions[0];
        let new_text = format!("{}{}", &text[..start], completion.text);
        input.set_text(&new_text);
        input.completion_visible = false;
    } else if !completions.is_empty() {
        input.completions = completions;
        input.completion_index = 0;
        input.completion_start = start;
        input.completion_visible = true;
    }
    InputResult::none()
}

/// Select a completion from the popup (applies it and closes popup).
fn select_completion(input: &mut InputState, index: usize) -> InputResult {
    if let Some(completion) = input.completions.get(index) {
        let text = input.text();
        let new_text = format!("{}{}", &text[..input.completion_start], completion.text);
        input.set_text(&new_text);
    }
    input.completions.clear();
    input.completion_visible = false;
    InputResult::none()
}

/// Navigate to a completion item (changes selection without applying).
fn completion_navigate(input: &mut InputState, index: usize) -> InputResult {
    if input.completion_visible && index < input.completions.len() {
        input.completion_index = index;
    }
    InputResult::none()
}

// =============================================================================
// Command Submission (cross-domain)
// =============================================================================

/// Submit the current input as a command.
/// Returns an Action for the coordinator to execute.
fn submit(input: &mut InputState) -> InputResult {
    // Get text and trim to handle any trailing newlines from paste
    let text = input.text();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return InputResult::none();
    }

    // Check for one-shot agent prefix: "? " or "ai "
    let (is_agent_query, query) = if trimmed.starts_with("? ") {
        (true, trimmed.strip_prefix("? ").unwrap().to_string())
    } else if trimmed.starts_with("ai ") {
        (true, trimmed.strip_prefix("ai ").unwrap().to_string())
    } else {
        (input.mode == InputMode::Agent, trimmed.to_string())
    };

    // Use trimmed text for execution (strips trailing \n from pastes)
    let command = trimmed.to_string();

    // Add to appropriate history based on query type
    if is_agent_query {
        input.push_agent_history(query.trim());
    } else {
        input.push_shell_history(&command);
    }

    input.clear();
    input.shell_history_index = None;
    input.agent_history_index = None;
    input.saved_input.clear();

    // Return action for coordinator
    if is_agent_query {
        InputResult::action(Action::SpawnAgentQuery(query))
    } else {
        InputResult::action(Action::ExecuteCommand(command))
    }
}

// =============================================================================
// Focus Key Handler (called from window handler)
// =============================================================================

/// Handle keys when the input field is focused.
/// Called from the window handler, returns InputMessage to process.
/// Note: Most key handling is now done by text_editor's key_binding.
/// This function only handles history search mode (special overlay).
pub fn handle_focus_key(
    input: &mut InputState,
    key: Key,
    _modifiers: Modifiers,
) -> Option<InputMessage> {
    // History search mode takes priority - this is a special overlay mode
    // where the text_editor is not the primary focus
    if input.search_active {
        match &key {
            Key::Named(keyboard::key::Named::Escape) => {
                return Some(InputMessage::HistorySearchCancel);
            }
            Key::Named(keyboard::key::Named::Enter) => {
                return Some(InputMessage::HistorySearchSelect(input.search_index));
            }
            Key::Named(keyboard::key::Named::ArrowUp) => {
                if input.search_index > 0 {
                    input.search_index -= 1;
                }
                return None;
            }
            Key::Named(keyboard::key::Named::ArrowDown) => {
                if input.search_index < input.search_results.len().saturating_sub(1) {
                    input.search_index += 1;
                }
                return None;
            }
            _ => return None,
        }
    }

    // All other key handling is done by text_editor's key_binding:
    // - Tab for completion
    // - Escape to cancel completion
    // - Arrow keys for completion navigation
    // - Up/Down for history navigation
    // - Enter for submit/newline
    None
}
