//! Input domain handler.
//!
//! Handles text input, completion, history search, and attachments.
//! Pure functions take only `&mut InputState`. Cross-domain effects are
//! returned as `Action` values for the coordinator to process.

use std::sync::Arc;

use iced::keyboard::{self, Key, Modifiers};
use iced::Task;
use tokio::sync::Mutex;

use nexus_kernel::{Completion, Kernel};

use crate::blocks::InputMode;
use crate::msg::{Action, InputMessage, Message};
use crate::state::{InputState, Nexus};

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
/// Returns task and any cross-domain actions needed.
pub fn update(state: &mut Nexus, msg: InputMessage) -> InputResult {
    match msg {
        // Pure input operations
        InputMessage::Changed(val) => changed(&mut state.input, val),
        InputMessage::ToggleMode => toggle_mode(&mut state.input),
        InputMessage::CancelCompletion => cancel_completion(&mut state.input),
        InputMessage::PasteImage(d, w, h) => paste_image(&mut state.input, d, w, h),
        InputMessage::RemoveAttachment(idx) => remove_attachment(&mut state.input, idx),
        InputMessage::HistoryKey(key, mods) => history_key(&mut state.input, key, mods),

        // Operations needing kernel access
        InputMessage::TabCompletion => {
            completion_tab(&mut state.input, &state.terminal.kernel)
        }
        InputMessage::SelectCompletion(idx) => select_completion(&mut state.input, idx),

        // History search (needs kernel)
        InputMessage::HistorySearchStart => {
            super::history::start(&mut state.input, &state.terminal.kernel)
        }
        InputMessage::HistorySearchChanged(q) => {
            super::history::search(&mut state.input, &state.terminal.kernel, q)
        }
        InputMessage::HistorySearchSelect(i) => {
            super::history::select(&mut state.input, i)
        }
        InputMessage::HistorySearchCancel => {
            super::history::cancel(&mut state.input)
        }

        // Cross-domain: submit command
        InputMessage::Submit => submit(&mut state.input),
    }
}

// =============================================================================
// Pure Input Operations
// =============================================================================

/// Handle input text change. Pure operation on InputState.
fn changed(input: &mut InputState, value: String) -> InputResult {
    if input.suppress_next {
        input.suppress_next = false;
        return InputResult::none();
    }

    if input.completion_visible {
        input.completions.clear();
        input.completion_visible = false;
    }
    input.before_event = input.buffer.clone();
    input.buffer = value;

    // Return action to set focus (cross-domain effect)
    InputResult::action(Action::FocusInput)
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
fn history_key(input: &mut InputState, key: Key, _modifiers: Modifiers) -> InputResult {
    match &key {
        Key::Named(keyboard::key::Named::ArrowUp) => {
            if input.history.is_empty() {
                return InputResult::none();
            }

            match input.history_index {
                None => {
                    input.saved_input = input.buffer.clone();
                    input.history_index = Some(input.history.len() - 1);
                }
                Some(0) => {}
                Some(i) => {
                    input.history_index = Some(i - 1);
                }
            }

            if let Some(i) = input.history_index {
                input.buffer = input.history[i].clone();
            }
        }
        Key::Named(keyboard::key::Named::ArrowDown) => match input.history_index {
            None => {}
            Some(i) if i >= input.history.len() - 1 => {
                input.history_index = None;
                input.buffer = input.saved_input.clone();
                input.saved_input.clear();
            }
            Some(i) => {
                input.history_index = Some(i + 1);
                input.buffer = input.history[i + 1].clone();
            }
        },
        _ => {}
    }
    InputResult::none()
}

// =============================================================================
// Completion (needs kernel read access)
// =============================================================================

/// Handle Tab key for completion.
fn completion_tab(input: &mut InputState, kernel: &Arc<Mutex<Kernel>>) -> InputResult {
    let kernel_guard = kernel.blocking_lock();
    let cursor = input.buffer.len();
    let (completions, start) = kernel_guard.complete(&input.buffer, cursor);
    drop(kernel_guard);

    apply_completions(input, completions, start)
}

/// Apply completion results to input state.
fn apply_completions(
    input: &mut InputState,
    completions: Vec<Completion>,
    start: usize,
) -> InputResult {
    if completions.len() == 1 {
        let completion = &completions[0];
        input.buffer = format!("{}{}", &input.buffer[..start], completion.text);
        input.completion_visible = false;
    } else if !completions.is_empty() {
        input.completions = completions;
        input.completion_index = 0;
        input.completion_start = start;
        input.completion_visible = true;
    }
    InputResult::none()
}

/// Select a completion from the popup.
fn select_completion(input: &mut InputState, index: usize) -> InputResult {
    if let Some(completion) = input.completions.get(index) {
        input.buffer = format!(
            "{}{}",
            &input.buffer[..input.completion_start],
            completion.text
        );
    }
    input.completions.clear();
    input.completion_visible = false;
    InputResult::none()
}

// =============================================================================
// Command Submission (cross-domain)
// =============================================================================

/// Submit the current input as a command.
/// Returns an Action for the coordinator to execute.
fn submit(input: &mut InputState) -> InputResult {
    let input_text = input.buffer.trim();
    if input_text.is_empty() {
        return InputResult::none();
    }

    // Check for one-shot agent prefix: "? " or "ai "
    let (is_agent_query, query) = if input_text.starts_with("? ") {
        (true, input_text.strip_prefix("? ").unwrap().to_string())
    } else if input_text.starts_with("ai ") {
        (true, input_text.strip_prefix("ai ").unwrap().to_string())
    } else {
        (input.mode == InputMode::Agent, input_text.to_string())
    };

    let command = input.buffer.clone();
    input.buffer.clear();
    input.history_index = None;
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
pub fn handle_focus_key(input: &mut InputState, key: Key, modifiers: Modifiers) -> Option<InputMessage> {
    // History search mode takes priority
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

    // Tab for completion
    if matches!(key, Key::Named(keyboard::key::Named::Tab)) {
        if input.completion_visible {
            return Some(InputMessage::SelectCompletion(input.completion_index));
        } else {
            return Some(InputMessage::TabCompletion);
        }
    }

    // Escape to cancel completion
    if matches!(key, Key::Named(keyboard::key::Named::Escape)) && input.completion_visible {
        return Some(InputMessage::CancelCompletion);
    }

    // Arrow keys for completion navigation
    if input.completion_visible {
        match &key {
            Key::Named(keyboard::key::Named::ArrowUp) => {
                if input.completion_index > 0 {
                    input.completion_index -= 1;
                }
                return None;
            }
            Key::Named(keyboard::key::Named::ArrowDown) => {
                if input.completion_index < input.completions.len().saturating_sub(1) {
                    input.completion_index += 1;
                }
                return None;
            }
            Key::Named(keyboard::key::Named::Enter) => {
                return Some(InputMessage::SelectCompletion(input.completion_index));
            }
            _ => {}
        }
    }

    // Up/Down for history navigation
    match &key {
        Key::Named(keyboard::key::Named::ArrowUp) if !modifiers.shift() => {
            return Some(InputMessage::HistoryKey(key, modifiers));
        }
        Key::Named(keyboard::key::Named::ArrowDown) if !modifiers.shift() => {
            return Some(InputMessage::HistoryKey(key, modifiers));
        }
        _ => {}
    }

    None
}
