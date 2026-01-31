//! Input widget — owns text input state, mode, history, attachments, and child widgets.

use std::sync::Arc;

use nexus_api::Value;
use nexus_kernel::Kernel;
use tokio::sync::Mutex;

use strata::event_context::KeyEvent;
use strata::{TextInputAction, TextInputMouseAction, TextInputState};

use crate::blocks::InputMode;
use super::completion::{CompletionWidget, CompletionOutput};
use super::history_search::{HistorySearchWidget, HistorySearchOutput};
use super::Attachment;

/// Typed output from InputWidget → orchestrator.
pub(crate) enum InputOutput {
    /// Nothing happened.
    None,
    /// User submitted text. Orchestrator decides whether to run shell or agent.
    Submit {
        text: String,
        is_agent: bool,
        attachments: Vec<Value>,
    },
}

/// Manages all input-related state: text, mode, history, attachments, and child widgets.
pub(crate) struct InputWidget {
    pub text_input: TextInputState,
    pub mode: InputMode,
    pub saved_input: String,
    pub attachments: Vec<Attachment>,
    // History (per-mode)
    shell_history: Vec<String>,
    shell_history_index: Option<usize>,
    agent_history: Vec<String>,
    agent_history_index: Option<usize>,
    // Children
    pub completion: CompletionWidget,
    pub history_search: HistorySearchWidget,
}

impl InputWidget {
    pub fn new(command_history: Vec<String>) -> Self {
        Self {
            text_input: TextInputState::new(),
            mode: InputMode::Shell,
            saved_input: String::new(),
            attachments: Vec::new(),
            shell_history: command_history,
            shell_history_index: None,
            agent_history: Vec::new(),
            agent_history_index: None,
            completion: CompletionWidget::new(),
            history_search: HistorySearchWidget::new(),
        }
    }

    /// Handle a key event on the text input. Returns InputOutput if submit occurred.
    pub fn handle_key(&mut self, event: &KeyEvent) -> InputOutput {
        match self.text_input.handle_key(event, false) {
            TextInputAction::Submit(text) => self.process_submit(text),
            _ => InputOutput::None,
        }
    }

    /// Submit text directly (bypassing key event processing).
    pub fn submit(&mut self, text: String) -> InputOutput {
        self.process_submit(text)
    }

    /// Handle a mouse action on the text input.
    pub fn handle_mouse(&mut self, action: TextInputMouseAction) {
        self.text_input.focused = true;
        self.text_input.apply_mouse(action);
    }

    /// Toggle between shell and agent mode.
    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            InputMode::Shell => InputMode::Agent,
            InputMode::Agent => InputMode::Shell,
        };
    }

    /// Navigate to previous history entry.
    pub fn history_up(&mut self) {
        let history = self.current_history();
        let history_len = history.len();
        if history_len == 0 {
            return;
        }
        let idx = self.current_history_index();
        let new_index = match idx {
            None => {
                self.saved_input = self.text_input.text.clone();
                Some(history_len - 1)
            }
            Some(0) => Some(0),
            Some(i) => Some(i - 1),
        };
        self.set_history_index(new_index);
        if let Some(i) = new_index {
            let text = self.current_history()[i].clone();
            self.text_input.text = text;
            self.text_input.cursor = self.text_input.text.len();
        }
    }

    /// Navigate to next history entry.
    pub fn history_down(&mut self) {
        let history_len = self.current_history().len();
        if history_len == 0 {
            return;
        }
        let idx = self.current_history_index();
        match idx {
            None => {}
            Some(i) if i >= history_len - 1 => {
                self.set_history_index(None);
                self.text_input.text = self.saved_input.clone();
                self.text_input.cursor = self.text_input.text.len();
                self.saved_input.clear();
            }
            Some(i) => {
                self.set_history_index(Some(i + 1));
                let text = self.current_history()[i + 1].clone();
                self.text_input.text = text;
                self.text_input.cursor = self.text_input.text.len();
            }
        }
    }

    /// Insert a newline in the text input.
    pub fn insert_newline(&mut self) {
        self.text_input.insert_newline();
    }

    /// Insert text (paste).
    pub fn paste_text(&mut self, text: &str) {
        self.text_input.insert_str(text);
    }

    /// Add a clipboard image attachment.
    pub fn add_attachment(&mut self, attachment: Attachment) {
        self.attachments.push(attachment);
    }

    /// Remove an attachment by index.
    pub fn remove_attachment(&mut self, idx: usize) {
        if idx < self.attachments.len() {
            self.attachments.remove(idx);
        }
    }

    // ---- Completion delegation ----

    /// Trigger tab completion.
    pub fn tab_complete(&mut self, kernel: &Arc<Mutex<Kernel>>) -> InputOutput {
        let output = self.completion.tab_complete(&self.text_input.text, self.text_input.cursor, kernel);
        self.apply_completion_output(output);
        InputOutput::None
    }

    /// Navigate completions by delta.
    pub fn completion_nav(&mut self, delta: isize) {
        self.completion.navigate(delta);
    }

    /// Accept the current completion.
    pub fn completion_accept(&mut self) {
        let output = self.completion.accept(&self.text_input.text, self.text_input.cursor);
        self.apply_completion_output(output);
    }

    /// Dismiss the completion popup.
    pub fn completion_dismiss(&mut self) {
        self.completion.dismiss();
    }

    /// Select a completion by index (click).
    pub fn completion_select(&mut self, index: usize) {
        let output = self.completion.select(index, &self.text_input.text, self.text_input.cursor);
        self.apply_completion_output(output);
    }

    // ---- History search delegation ----

    /// Toggle history search on/off.
    pub fn history_search_toggle(&mut self) {
        self.history_search.toggle();
    }

    /// Handle a key in history search.
    pub fn history_search_key(&mut self, key_event: KeyEvent, kernel: &Arc<Mutex<Kernel>>) {
        self.history_search.handle_key(key_event, kernel);
    }

    /// Accept current history search result.
    pub fn history_search_accept(&mut self) {
        let output = self.history_search.accept();
        self.apply_history_search_output(output);
    }

    /// Dismiss history search.
    pub fn history_search_dismiss(&mut self) {
        self.history_search.dismiss();
    }

    /// Select a history search result.
    pub fn history_search_select(&mut self, index: usize) {
        self.history_search.select(index);
    }

    /// Accept a specific history search result (click).
    pub fn history_search_accept_index(&mut self, index: usize) {
        let output = self.history_search.accept_index(index);
        self.apply_history_search_output(output);
    }

    /// Access shell history (for building agent context).
    pub fn shell_history(&self) -> &[String] {
        &self.shell_history
    }

    /// Reset history navigation state (called after submit).
    pub fn reset_history_nav(&mut self) {
        self.shell_history_index = None;
        self.agent_history_index = None;
        self.saved_input.clear();
    }

    // ---- Internal ----

    fn process_submit(&mut self, submitted_text: String) -> InputOutput {
        let text = submitted_text.trim().to_string();
        if text.is_empty() {
            return InputOutput::None;
        }

        let is_agent = self.mode == InputMode::Agent || text.starts_with("? ");
        let query = if text.starts_with("? ") {
            text[2..].to_string()
        } else {
            text.clone()
        };

        self.push_history(&text);

        let attachments: Vec<Value> = if is_agent {
            self.attachments.drain(..).map(|a| {
                Value::Media {
                    data: a.data,
                    content_type: "image/png".to_string(),
                    metadata: Default::default(),
                }
            }).collect()
        } else {
            self.attachments.clear();
            Vec::new()
        };

        InputOutput::Submit {
            text: query,
            is_agent,
            attachments,
        }
    }

    fn apply_completion_output(&mut self, output: CompletionOutput) {
        match output {
            CompletionOutput::Applied { text, cursor } |
            CompletionOutput::Accepted { text, cursor } => {
                self.text_input.text = text;
                self.text_input.cursor = cursor;
            }
            CompletionOutput::None | CompletionOutput::Dismissed => {}
        }
    }

    fn apply_history_search_output(&mut self, output: HistorySearchOutput) {
        match output {
            HistorySearchOutput::Accepted { text } => {
                self.text_input.cursor = text.len();
                self.text_input.text = text;
            }
            HistorySearchOutput::None | HistorySearchOutput::Dismissed => {}
        }
    }

    fn current_history(&self) -> &[String] {
        match self.mode {
            InputMode::Shell => &self.shell_history,
            InputMode::Agent => &self.agent_history,
        }
    }

    fn current_history_index(&self) -> Option<usize> {
        match self.mode {
            InputMode::Shell => self.shell_history_index,
            InputMode::Agent => self.agent_history_index,
        }
    }

    fn set_history_index(&mut self, idx: Option<usize>) {
        match self.mode {
            InputMode::Shell => self.shell_history_index = idx,
            InputMode::Agent => self.agent_history_index = idx,
        }
    }

    fn push_history(&mut self, text: &str) {
        let history = match self.mode {
            InputMode::Shell => &mut self.shell_history,
            InputMode::Agent => &mut self.agent_history,
        };
        if history.last().map(|s| s.as_str()) != Some(text) {
            history.push(text.to_string());
            if history.len() > 1000 {
                history.remove(0);
            }
        }
    }
}
