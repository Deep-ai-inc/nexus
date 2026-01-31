//! Completion popup widget — owns completion state and handles all completion logic.

use std::cell::Cell;
use std::sync::Arc;

use nexus_kernel::{Completion, Kernel};
use tokio::sync::Mutex;

use strata::{ScrollAction, ScrollState};

/// Typed output from CompletionWidget → parent.
pub(crate) enum CompletionOutput {
    /// Nothing happened.
    None,
    /// Single match was auto-applied. Parent should update text+cursor.
    Applied { text: String, cursor: usize },
    /// User accepted a completion. Parent should update text+cursor.
    Accepted { text: String, cursor: usize },
    /// Popup was dismissed, no text change.
    Dismissed,
}

/// Completion popup state and logic.
pub(crate) struct CompletionWidget {
    pub completions: Vec<Completion>,
    pub index: Option<usize>,
    pub anchor: usize,
    pub scroll: ScrollState,
    pub hovered: Cell<Option<usize>>,
}

impl CompletionWidget {
    pub fn new() -> Self {
        Self {
            completions: Vec::new(),
            index: None,
            anchor: 0,
            scroll: ScrollState::new(),
            hovered: Cell::new(None),
        }
    }

    pub fn is_active(&self) -> bool {
        !self.completions.is_empty()
    }

    /// Trigger tab completion. Needs current input text, cursor, and kernel.
    pub fn tab_complete(
        &mut self,
        input_text: &str,
        input_cursor: usize,
        kernel: &Arc<Mutex<Kernel>>,
    ) -> CompletionOutput {
        let (completions, anchor) = kernel.blocking_lock().complete(input_text, input_cursor);
        if completions.len() == 1 {
            // Single completion: apply immediately
            let comp = &completions[0];
            let mut t = input_text.to_string();
            let end = input_cursor.min(t.len());
            t.replace_range(anchor..end, &comp.text);
            let cursor = anchor + comp.text.len();
            self.completions.clear();
            self.index = None;
            CompletionOutput::Applied { text: t, cursor }
        } else if !completions.is_empty() {
            self.completions = completions;
            self.index = Some(0);
            self.anchor = anchor;
            self.scroll.offset = 0.0;
            CompletionOutput::None
        } else {
            CompletionOutput::None
        }
    }

    /// Navigate by delta (+1 = down, -1 = up).
    pub fn navigate(&mut self, delta: isize) -> CompletionOutput {
        if !self.completions.is_empty() {
            let len = self.completions.len() as isize;
            let current = self.index.unwrap_or(0) as isize;
            let new_idx = ((current + delta) % len + len) % len;
            self.index = Some(new_idx as usize);
            scroll_to_index(&mut self.scroll, new_idx as usize, 26.0, 300.0);
        }
        CompletionOutput::None
    }

    /// Accept the currently selected completion.
    pub fn accept(&mut self, input_text: &str, input_cursor: usize) -> CompletionOutput {
        let output = if let Some(idx) = self.index {
            if let Some(comp) = self.completions.get(idx) {
                let mut t = input_text.to_string();
                let end = input_cursor.min(t.len());
                t.replace_range(self.anchor..end, &comp.text);
                let cursor = self.anchor + comp.text.len();
                CompletionOutput::Accepted { text: t, cursor }
            } else {
                CompletionOutput::Dismissed
            }
        } else {
            CompletionOutput::Dismissed
        };
        self.completions.clear();
        self.index = None;
        output
    }

    /// Dismiss the completion popup.
    pub fn dismiss(&mut self) -> CompletionOutput {
        self.completions.clear();
        self.index = None;
        CompletionOutput::Dismissed
    }

    /// Accept a specific completion by index (click).
    pub fn select(&mut self, index: usize, input_text: &str, input_cursor: usize) -> CompletionOutput {
        let output = if let Some(comp) = self.completions.get(index) {
            let mut t = input_text.to_string();
            let end = input_cursor.min(t.len());
            t.replace_range(self.anchor..end, &comp.text);
            let cursor = self.anchor + comp.text.len();
            CompletionOutput::Accepted { text: t, cursor }
        } else {
            CompletionOutput::Dismissed
        };
        self.completions.clear();
        self.index = None;
        output
    }

    /// Handle scroll action on the completion popup.
    pub fn scroll(&mut self, action: ScrollAction) {
        self.scroll.apply(action);
    }
}

fn scroll_to_index(scroll: &mut ScrollState, index: usize, item_height: f32, viewport_height: f32) {
    let item_top = index as f32 * item_height;
    let item_bottom = item_top + item_height;
    if item_top < scroll.offset {
        scroll.offset = item_top;
    } else if item_bottom > scroll.offset + viewport_height {
        scroll.offset = item_bottom - viewport_height;
    }
}
