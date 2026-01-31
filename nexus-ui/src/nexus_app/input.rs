//! Input widget — owns text input state, mode, history, attachments, and child widgets.

use std::sync::Arc;

use nexus_api::Value;
use nexus_kernel::Kernel;
use tokio::sync::Mutex;

use strata::content_address::SourceId;
use strata::event_context::{Key, KeyEvent, NamedKey};
use strata::layout_snapshot::HitResult;
use strata::{
    ButtonElement, Column, Command, CrossAxisAlignment, ImageElement, LayoutSnapshot, Length,
    Padding, Row, TextInputAction, TextInputMouseAction, TextInputState,
};

use crate::nexus_widgets::{CompletionPopup, HistorySearchBar, NexusInputBar};

use crate::blocks::InputMode;
use super::completion::{CompletionWidget, CompletionOutput};
use super::history_search::{HistorySearchWidget, HistorySearchOutput};
use super::message::InputMsg;
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
    // Shared reference for completion/search
    kernel: Arc<Mutex<Kernel>>,
}

impl InputWidget {
    pub fn new(command_history: Vec<String>, kernel: Arc<Mutex<Kernel>>) -> Self {
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
            kernel,
        }
    }

    /// Handle a message, returning commands and cross-cutting output.
    pub fn update(&mut self, msg: InputMsg, _ctx: &mut strata::component::Ctx) -> (Command<InputMsg>, InputOutput) {
        let output = match msg {
            InputMsg::Key(event) => self.handle_key(&event),
            InputMsg::Mouse(action) => { self.handle_mouse(action); InputOutput::None }
            InputMsg::Submit(text) => self.submit(text),
            InputMsg::ToggleMode => { self.toggle_mode(); InputOutput::None }
            InputMsg::HistoryUp => { self.history_up(); InputOutput::None }
            InputMsg::HistoryDown => { self.history_down(); InputOutput::None }
            InputMsg::InsertNewline => { self.insert_newline(); InputOutput::None }
            InputMsg::RemoveAttachment(idx) => { self.remove_attachment(idx); InputOutput::None }

            InputMsg::TabComplete => { self.tab_complete(); InputOutput::None }
            InputMsg::CompletionNav(delta) => { self.completion_nav(delta); InputOutput::None }
            InputMsg::CompletionAccept => { self.completion_accept(); InputOutput::None }
            InputMsg::CompletionDismiss => { self.completion_dismiss(); InputOutput::None }
            InputMsg::CompletionDismissAndForward(event) => {
                self.completion_dismiss();
                self.handle_key(&event)
            }
            InputMsg::CompletionSelect(index) => { self.completion_select(index); InputOutput::None }
            InputMsg::CompletionScroll(action) => { self.completion.apply_scroll(action); InputOutput::None }

            InputMsg::HistorySearchToggle => { self.history_search_toggle(); InputOutput::None }
            InputMsg::HistorySearchKey(key_event) => {
                self.history_search_key(key_event);
                InputOutput::None
            }
            InputMsg::HistorySearchAccept => { self.history_search_accept(); InputOutput::None }
            InputMsg::HistorySearchDismiss => { self.history_search_dismiss(); InputOutput::None }
            InputMsg::HistorySearchSelect(index) => { self.history_search_select(index); InputOutput::None }
            InputMsg::HistorySearchAcceptIndex(index) => { self.history_search_accept_index(index); InputOutput::None }
            InputMsg::HistorySearchScroll(action) => { self.history_search.apply_scroll(action); InputOutput::None }
        };
        (Command::none(), output)
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
    pub fn tab_complete(&mut self) {
        let kernel = self.kernel.clone();
        let output = self.completion.tab_complete(&self.text_input.text, self.text_input.cursor, &kernel);
        self.apply_completion_output(output);
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
    pub fn history_search_key(&mut self, key_event: KeyEvent) {
        let kernel = self.kernel.clone();
        self.history_search.handle_key(key_event, &kernel);
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

// =========================================================================
// View contributions
// =========================================================================

impl InputWidget {
    /// Build the overlays section (completion popup, history search bar).
    pub fn layout_overlays(&self, mut col: Column) -> Column {
        if self.completion.is_active() {
            col = col.push(CompletionPopup {
                completions: &self.completion.completions,
                selected_index: self.completion.index,
                hovered_index: self.completion.hovered.get(),
                scroll: &self.completion.scroll,
            });
        }

        if self.history_search.is_active() {
            col = col.push(HistorySearchBar {
                query: &self.history_search.query,
                results: &self.history_search.results,
                result_index: self.history_search.index,
                hovered_index: self.history_search.hovered.get(),
                scroll: &self.history_search.scroll,
            });
        }

        col
    }

    /// Build the attachments section (image thumbnails with remove buttons).
    pub fn layout_attachments(&self, mut col: Column) -> Column {
        if self.attachments.is_empty() {
            return col;
        }

        let mut attach_row = Row::new().spacing(8.0).padding(4.0);
        for (i, attachment) in self.attachments.iter().enumerate() {
            let scale = (60.0_f32 / attachment.width as f32)
                .min(60.0 / attachment.height as f32)
                .min(1.0);
            let w = attachment.width as f32 * scale;
            let h = attachment.height as f32 * scale;
            let remove_id = super::source_ids::remove_attachment(i);
            attach_row = attach_row.push(
                Column::new()
                    .spacing(2.0)
                    .cross_align(CrossAxisAlignment::Center)
                    .image(ImageElement::new(attachment.image_handle, w, h).corner_radius(4.0))
                    .push(
                        ButtonElement::new(remove_id, "\u{2715}")
                            .background(super::colors::BTN_DENY)
                            .corner_radius(4.0),
                    ),
            );
        }
        col = col.push(
            Column::new()
                .padding_custom(Padding::new(2.0, 4.0, 0.0, 4.0))
                .width(Length::Fill)
                .push(attach_row),
        );
        col
    }

    /// Build the input bar section.
    pub fn layout_input_bar(
        &self,
        mut col: Column,
        cwd: &str,
        last_exit_code: Option<i32>,
        cursor_visible: bool,
    ) -> Column {
        let line_count = {
            let count = self.text_input.text.lines().count()
                + if self.text_input.text.ends_with('\n') {
                    1
                } else {
                    0
                };
            count.max(1).min(6)
        };

        col = col.push(
            Column::new()
                .padding_custom(Padding::new(2.0, 4.0, 4.0, 4.0))
                .width(Length::Fill)
                .push(NexusInputBar {
                    input: &self.text_input,
                    mode: self.mode,
                    cwd,
                    last_exit_code,
                    cursor_visible,
                    mode_toggle_id: super::source_ids::mode_toggle(),
                    line_count,
                }),
        );
        col
    }

    /// Sync scroll states for completion, history search, and text input from layout snapshot.
    pub fn sync_scroll_states(&self, snapshot: &mut LayoutSnapshot) {
        self.completion.scroll.sync_from_snapshot(snapshot);
        self.history_search.scroll.sync_from_snapshot(snapshot);
        self.text_input.sync_from_snapshot(snapshot);
    }
}

// =========================================================================
// Event routing
// =========================================================================

impl InputWidget {
    /// Whether this widget wants to intercept all keys (overlay mode).
    pub fn captures_keys(&self) -> bool {
        self.history_search.is_active() || self.completion.is_active()
    }

    /// Handle keyboard events. Returns None if the event is not consumed.
    ///
    /// Handles three modes:
    /// 1. History search active → intercepts most keys
    /// 2. Completion popup active → intercepts navigation keys
    /// 3. Normal input focused → handles Enter, Tab, ArrowUp/Down, regular keys
    pub fn on_key(&self, event: &KeyEvent) -> Option<InputMsg> {
        let KeyEvent::Pressed {
            key,
            modifiers,
            ..
        } = event
        else {
            return None;
        };

        // History search mode intercepts most keys
        if self.history_search.is_active() {
            return self.on_key_history_search(key, modifiers, event);
        }

        // Completion popup intercepts navigation keys
        if self.completion.is_active() {
            return self.on_key_completion(key, modifiers, event);
        }

        // Normal input mode (only when focused)
        if self.text_input.focused {
            return self.on_key_focused(key, modifiers, event);
        }

        None
    }

    fn on_key_history_search(
        &self,
        key: &Key,
        modifiers: &strata::event_context::Modifiers,
        event: &KeyEvent,
    ) -> Option<InputMsg> {
        if modifiers.ctrl {
            if let Key::Character(c) = key {
                if c == "r" {
                    return Some(InputMsg::HistorySearchToggle);
                }
            }
        }
        match key {
            Key::Named(NamedKey::Enter) => Some(InputMsg::HistorySearchAccept),
            Key::Named(NamedKey::Escape) => Some(InputMsg::HistorySearchDismiss),
            Key::Named(NamedKey::ArrowDown) => {
                if !self.history_search.results.is_empty()
                    && self.history_search.index < self.history_search.results.len() - 1
                {
                    Some(InputMsg::HistorySearchSelect(
                        self.history_search.index + 1,
                    ))
                } else {
                    None
                }
            }
            Key::Named(NamedKey::ArrowUp) => {
                if self.history_search.index > 0 {
                    Some(InputMsg::HistorySearchSelect(
                        self.history_search.index - 1,
                    ))
                } else {
                    None
                }
            }
            _ => Some(InputMsg::HistorySearchKey(event.clone())),
        }
    }

    fn on_key_completion(
        &self,
        key: &Key,
        modifiers: &strata::event_context::Modifiers,
        event: &KeyEvent,
    ) -> Option<InputMsg> {
        match key {
            Key::Named(NamedKey::Tab) if modifiers.shift => Some(InputMsg::CompletionNav(-1)),
            Key::Named(NamedKey::Tab) => Some(InputMsg::CompletionNav(1)),
            Key::Named(NamedKey::ArrowDown) => Some(InputMsg::CompletionNav(1)),
            Key::Named(NamedKey::ArrowUp) => Some(InputMsg::CompletionNav(-1)),
            Key::Named(NamedKey::Enter) => Some(InputMsg::CompletionAccept),
            Key::Named(NamedKey::Escape) => Some(InputMsg::CompletionDismiss),
            _ => Some(InputMsg::CompletionDismissAndForward(event.clone())),
        }
    }

    fn on_key_focused(
        &self,
        key: &Key,
        modifiers: &strata::event_context::Modifiers,
        event: &KeyEvent,
    ) -> Option<InputMsg> {
        if matches!(key, Key::Named(NamedKey::Enter)) && modifiers.shift {
            return Some(InputMsg::InsertNewline);
        }
        if matches!(key, Key::Named(NamedKey::Tab)) {
            return Some(InputMsg::TabComplete);
        }
        if matches!(key, Key::Named(NamedKey::ArrowUp)) {
            return Some(InputMsg::HistoryUp);
        }
        if matches!(key, Key::Named(NamedKey::ArrowDown)) {
            return Some(InputMsg::HistoryDown);
        }
        Some(InputMsg::Key(event.clone()))
    }

    /// Handle a widget click within input-owned UI. Returns None if not our widget.
    pub fn on_click(&self, id: SourceId) -> Option<InputMsg> {
        if id == super::source_ids::mode_toggle() {
            return Some(InputMsg::ToggleMode);
        }
        // Completion item clicks
        for i in 0..self.completion.completions.len().min(10) {
            if id == CompletionPopup::item_id(i) {
                return Some(InputMsg::CompletionSelect(i));
            }
        }
        // History search result clicks
        if self.history_search.is_active() {
            for i in 0..self.history_search.results.len().min(10) {
                if id == HistorySearchBar::result_id(i) {
                    return Some(InputMsg::HistorySearchAcceptIndex(i));
                }
            }
        }
        // Attachment remove buttons
        for i in 0..self.attachments.len() {
            if id == super::source_ids::remove_attachment(i) {
                return Some(InputMsg::RemoveAttachment(i));
            }
        }
        None
    }

    /// Handle hover tracking for completion/history popups.
    pub fn on_hover(&self, hit: &Option<HitResult>) {
        if self.completion.is_active() {
            let idx = if let Some(HitResult::Widget(id)) = hit {
                (0..self.completion.completions.len().min(10))
                    .find(|i| *id == CompletionPopup::item_id(*i))
            } else {
                None
            };
            self.completion.hovered.set(idx);
        }
        if self.history_search.is_active() {
            let idx = if let Some(HitResult::Widget(id)) = hit {
                (0..self.history_search.results.len().min(10))
                    .find(|i| *id == HistorySearchBar::result_id(*i))
            } else {
                None
            };
            self.history_search.hovered.set(idx);
        }
    }

    /// Check if a position is within the input area bounds.
    pub fn hit_test(&self, x: f32, y: f32) -> bool {
        let b = self.text_input.bounds();
        x >= b.x && x <= b.x + b.width && y >= b.y && y <= b.y + b.height
    }
}
