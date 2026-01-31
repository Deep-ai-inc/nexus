//! History search widget (Ctrl+R) — owns search state and handles all search logic.

use std::cell::Cell;
use std::sync::Arc;

use nexus_kernel::Kernel;
use tokio::sync::Mutex;

use strata::event_context::{Key, KeyEvent, NamedKey};
use strata::{ScrollAction, ScrollState};

/// Typed output from HistorySearchWidget → parent.
pub(crate) enum HistorySearchOutput {
    /// Nothing happened.
    None,
    /// User accepted a result. Parent should update text+cursor.
    Accepted { text: String },
    /// Search was dismissed, no text change.
    Dismissed,
}

/// History search (Ctrl+R) modal state and logic.
pub(crate) struct HistorySearchWidget {
    pub active: bool,
    pub query: String,
    pub results: Vec<String>,
    pub index: usize,
    pub scroll: ScrollState,
    pub hovered: Cell<Option<usize>>,
}

impl HistorySearchWidget {
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            results: Vec::new(),
            index: 0,
            scroll: ScrollState::new(),
            hovered: Cell::new(None),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Toggle search on/off. If already active, cycles to next result.
    pub fn toggle(&mut self) -> HistorySearchOutput {
        if self.active {
            // Cycle to next result
            if !self.results.is_empty() {
                self.index = (self.index + 1) % self.results.len();
                scroll_to_index(&mut self.scroll, self.index, 30.0, 300.0);
            }
        } else {
            self.active = true;
            self.query.clear();
            self.results.clear();
            self.index = 0;
            self.scroll.offset = 0.0;
        }
        HistorySearchOutput::None
    }

    /// Handle a key event while history search is active.
    pub fn handle_key(&mut self, key_event: KeyEvent, kernel: &Arc<Mutex<Kernel>>) -> HistorySearchOutput {
        if let KeyEvent::Pressed { key, .. } = key_event {
            match key {
                Key::Character(c) => {
                    self.query.push_str(&c);
                }
                Key::Named(NamedKey::Backspace) => {
                    self.query.pop();
                }
                _ => {}
            }
            // Re-search
            if self.query.is_empty() {
                self.results.clear();
            } else {
                let results = kernel.blocking_lock()
                    .search_history(&self.query, 50);
                self.results = results.into_iter().map(|e| e.command).collect();
            }
            self.index = 0;
        }
        HistorySearchOutput::None
    }

    /// Accept the currently highlighted result.
    pub fn accept(&mut self) -> HistorySearchOutput {
        let output = if let Some(result) = self.results.get(self.index) {
            HistorySearchOutput::Accepted { text: result.clone() }
        } else {
            HistorySearchOutput::Dismissed
        };
        self.close();
        output
    }

    /// Dismiss the search without applying.
    pub fn dismiss(&mut self) -> HistorySearchOutput {
        self.close();
        HistorySearchOutput::Dismissed
    }

    /// Select (highlight) a specific result by index.
    pub fn select(&mut self, index: usize) -> HistorySearchOutput {
        if index < self.results.len() {
            self.index = index;
            scroll_to_index(&mut self.scroll, index, 30.0, 300.0);
        }
        HistorySearchOutput::None
    }

    /// Accept a specific result by index (click).
    pub fn accept_index(&mut self, index: usize) -> HistorySearchOutput {
        let output = if let Some(result) = self.results.get(index) {
            HistorySearchOutput::Accepted { text: result.clone() }
        } else {
            HistorySearchOutput::Dismissed
        };
        self.close();
        output
    }

    /// Handle scroll action on the results list.
    pub fn scroll(&mut self, action: ScrollAction) {
        self.scroll.apply(action);
    }

    fn close(&mut self) {
        self.active = false;
        self.query.clear();
        self.results.clear();
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
