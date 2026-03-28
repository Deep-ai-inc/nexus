//! Completion popup widget — owns completion state and handles all completion logic.

use std::cell::Cell;
use std::sync::Arc;

use nexus_kernel::{Completion, Kernel, longest_common_prefix};
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

/// Append a trailing space to a completion if it's a terminal match (not a directory).
/// Bash does this so the user can immediately start typing the next argument.
fn with_trailing_space(text: &str) -> String {
    if text.ends_with('/') {
        text.to_string()
    } else {
        format!("{} ", text)
    }
}

/// Snap a byte offset to the nearest valid char boundary (rounding down).
fn snap_to_char_boundary(s: &str, n: usize) -> usize {
    let n = n.min(s.len());
    let mut i = n;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Completion popup state and logic.
pub(crate) struct CompletionWidget {
    pub completions: Vec<Completion>,
    pub index: Option<usize>,
    pub anchor: usize,
    pub scroll: ScrollState,
    pub hovered: Cell<Option<usize>>,
    /// Stashed completions from the first Tab press (LCP was inserted, popup not yet shown).
    /// Second Tab will open the popup from these.
    pub(crate) pending_completions: Vec<Completion>,
    pending_anchor: usize,
}

impl CompletionWidget {
    pub fn new() -> Self {
        Self {
            completions: Vec::new(),
            index: None,
            anchor: 0,
            scroll: ScrollState::new(),
            hovered: Cell::new(None),
            pending_completions: Vec::new(),
            pending_anchor: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        !self.completions.is_empty()
    }

    /// Trigger tab completion. Implements Bash-style LCP + double-tab:
    /// - First Tab: insert longest common prefix (no popup).
    /// - Second Tab (or first Tab if LCP adds nothing): open popup.
    /// - Single match: apply immediately.
    pub fn tab_complete(
        &mut self,
        input_text: &str,
        input_cursor: usize,
        kernel: &Arc<Mutex<Kernel>>,
    ) -> CompletionOutput {
        // Double-tab: if we have pending completions from a previous LCP insertion, show popup now
        if !self.pending_completions.is_empty() {
            // Re-filter pending completions against current input
            let anchor = snap_to_char_boundary(input_text, self.pending_anchor);
            let current_word = &input_text[anchor..input_cursor];
            let filtered: Vec<Completion> = self.pending_completions.drain(..)
                .filter(|c| c.text.starts_with(current_word))
                .collect();
            if filtered.len() <= 1 {
                // Down to one or zero — apply if one, else nothing
                if let Some(comp) = filtered.first() {
                    let completed = with_trailing_space(&comp.text);
                    let mut t = input_text.to_string();
                    let end = snap_to_char_boundary(&t, input_cursor);
                    t.replace_range(anchor..end, &completed);
                    let cursor = anchor + completed.len();
                    return CompletionOutput::Applied { text: t, cursor };
                }
                return CompletionOutput::None;
            }
            self.completions = filtered;
            self.index = Some(0);
            self.anchor = anchor;
            self.scroll.offset = 0.0;
            return CompletionOutput::None;
        }

        let (completions, anchor) = kernel.blocking_lock().complete(input_text, input_cursor);
        if completions.len() == 1 {
            // Single completion: apply immediately with trailing space (like Bash)
            let comp = &completions[0];
            let completed = with_trailing_space(&comp.text);
            let mut t = input_text.to_string();
            let a = snap_to_char_boundary(&t, anchor);
            let end = snap_to_char_boundary(&t, input_cursor);
            t.replace_range(a..end, &completed);
            let cursor = a + completed.len();
            self.completions.clear();
            self.index = None;
            CompletionOutput::Applied { text: t, cursor }
        } else if !completions.is_empty() {
            // Multiple completions: compute LCP and try to extend
            let a = snap_to_char_boundary(input_text, anchor);
            let current_word = &input_text[a..input_cursor.min(input_text.len())];
            let lcp = longest_common_prefix(&completions);

            if lcp.len() > current_word.len() {
                // LCP extends beyond what user typed — insert it, stash completions for double-tab
                let mut t = input_text.to_string();
                let end = snap_to_char_boundary(&t, input_cursor);
                t.replace_range(a..end, lcp);
                let cursor = a + lcp.len();
                self.pending_completions = completions;
                self.pending_anchor = anchor;
                CompletionOutput::Applied { text: t, cursor }
            } else {
                // LCP adds nothing — show popup immediately (no point making user double-tap)
                self.completions = completions;
                self.index = Some(0);
                self.anchor = a;
                self.scroll.offset = 0.0;
                CompletionOutput::None
            }
        } else {
            self.completions.clear();
            self.index = None;
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
                let completed = with_trailing_space(&comp.text);
                let mut t = input_text.to_string();
                let a = snap_to_char_boundary(&t, self.anchor);
                let end = snap_to_char_boundary(&t, input_cursor);
                t.replace_range(a..end, &completed);
                let cursor = a + completed.len();
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

    /// Dismiss the completion popup (and any pending double-tab state).
    pub fn dismiss(&mut self) -> CompletionOutput {
        self.completions.clear();
        self.index = None;
        self.pending_completions.clear();
        CompletionOutput::Dismissed
    }

    /// Accept a specific completion by index (click).
    pub fn select(&mut self, index: usize, input_text: &str, input_cursor: usize) -> CompletionOutput {
        let output = if let Some(comp) = self.completions.get(index) {
            let completed = with_trailing_space(&comp.text);
            let mut t = input_text.to_string();
            let a = snap_to_char_boundary(&t, self.anchor);
            let end = snap_to_char_boundary(&t, input_cursor);
            t.replace_range(a..end, &completed);
            let cursor = a + completed.len();
            CompletionOutput::Accepted { text: t, cursor }
        } else {
            CompletionOutput::Dismissed
        };
        self.completions.clear();
        self.index = None;
        output
    }

    /// Apply pre-fetched results from remote tab completion.
    /// Uses the same LCP + double-tab logic as local completion.
    pub fn apply_remote_result(
        &mut self,
        completions: Vec<Completion>,
        anchor: usize,
        input_text: &str,
        input_cursor: usize,
    ) -> CompletionOutput {
        if completions.len() == 1 {
            let comp = &completions[0];
            let completed = with_trailing_space(&comp.text);
            let mut t = input_text.to_string();
            let a = snap_to_char_boundary(&t, anchor);
            let end = snap_to_char_boundary(&t, input_cursor);
            t.replace_range(a..end, &completed);
            let cursor = a + completed.len();
            self.completions.clear();
            self.index = None;
            CompletionOutput::Applied { text: t, cursor }
        } else if !completions.is_empty() {
            let a = snap_to_char_boundary(input_text, anchor);
            let current_word = &input_text[a..input_cursor.min(input_text.len())];
            let lcp = longest_common_prefix(&completions);

            if lcp.len() > current_word.len() {
                let mut t = input_text.to_string();
                let end = snap_to_char_boundary(&t, input_cursor);
                t.replace_range(a..end, lcp);
                let cursor = a + lcp.len();
                self.pending_completions = completions;
                self.pending_anchor = anchor;
                CompletionOutput::Applied { text: t, cursor }
            } else {
                self.completions = completions;
                self.index = Some(0);
                self.anchor = a;
                self.scroll.offset = 0.0;
                CompletionOutput::None
            }
        } else {
            self.completions.clear();
            self.index = None;
            CompletionOutput::None
        }
    }

    /// Handle scroll action on the completion popup.
    pub fn apply_scroll(&mut self, action: ScrollAction) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_kernel::CompletionKind;

    fn make_completion(text: &str) -> Completion {
        Completion {
            text: text.to_string(),
            display: text.to_string(),
            kind: CompletionKind::File,
            score: 0,
        }
    }

    #[test]
    fn test_completion_widget_new() {
        let widget = CompletionWidget::new();
        assert!(widget.completions.is_empty());
        assert!(widget.index.is_none());
        assert_eq!(widget.anchor, 0);
        assert!(widget.hovered.get().is_none());
    }

    #[test]
    fn test_completion_widget_is_active_when_empty() {
        let widget = CompletionWidget::new();
        assert!(!widget.is_active());
    }

    #[test]
    fn test_completion_widget_is_active_with_completions() {
        let mut widget = CompletionWidget::new();
        widget.completions.push(make_completion("test"));
        assert!(widget.is_active());
    }

    #[test]
    fn test_completion_widget_navigate_empty() {
        let mut widget = CompletionWidget::new();
        // Navigating with no completions should do nothing
        let output = widget.navigate(1);
        assert!(matches!(output, CompletionOutput::None));
    }

    #[test]
    fn test_completion_widget_navigate_down() {
        let mut widget = CompletionWidget::new();
        widget.completions = vec![
            make_completion("a"),
            make_completion("b"),
            make_completion("c"),
        ];
        widget.index = Some(0);

        widget.navigate(1);
        assert_eq!(widget.index, Some(1));

        widget.navigate(1);
        assert_eq!(widget.index, Some(2));
    }

    #[test]
    fn test_completion_widget_navigate_wraps() {
        let mut widget = CompletionWidget::new();
        widget.completions = vec![
            make_completion("a"),
            make_completion("b"),
        ];
        widget.index = Some(1);

        // Navigate down from last item should wrap to first
        widget.navigate(1);
        assert_eq!(widget.index, Some(0));

        // Navigate up from first item should wrap to last
        widget.navigate(-1);
        assert_eq!(widget.index, Some(1));
    }

    #[test]
    fn test_completion_widget_accept_no_selection() {
        let mut widget = CompletionWidget::new();
        widget.index = None;
        let output = widget.accept("test", 4);
        assert!(matches!(output, CompletionOutput::Dismissed));
    }

    #[test]
    fn test_completion_widget_accept_invalid_index() {
        let mut widget = CompletionWidget::new();
        widget.index = Some(5); // Out of bounds
        let output = widget.accept("test", 4);
        assert!(matches!(output, CompletionOutput::Dismissed));
    }

    #[test]
    fn test_completion_widget_accept_valid() {
        let mut widget = CompletionWidget::new();
        widget.completions = vec![make_completion("hello")];
        widget.index = Some(0);
        widget.anchor = 0;

        let output = widget.accept("h", 1);
        if let CompletionOutput::Accepted { text, cursor } = output {
            assert_eq!(text, "hello ");
            assert_eq!(cursor, 6);
        } else {
            panic!("Expected CompletionOutput::Accepted");
        }

        // After accepting, completions should be cleared
        assert!(widget.completions.is_empty());
        assert!(widget.index.is_none());
    }

    #[test]
    fn test_completion_widget_dismiss() {
        let mut widget = CompletionWidget::new();
        widget.completions = vec![make_completion("test")];
        widget.index = Some(0);

        let output = widget.dismiss();
        assert!(matches!(output, CompletionOutput::Dismissed));
        assert!(widget.completions.is_empty());
        assert!(widget.index.is_none());
    }

    #[test]
    fn test_completion_widget_select_valid() {
        let mut widget = CompletionWidget::new();
        widget.completions = vec![
            make_completion("foo"),
            make_completion("bar"),
        ];
        widget.anchor = 0;

        let output = widget.select(1, "b", 1);
        if let CompletionOutput::Accepted { text, cursor } = output {
            assert_eq!(text, "bar ");
            assert_eq!(cursor, 4);
        } else {
            panic!("Expected CompletionOutput::Accepted");
        }
    }

    #[test]
    fn test_completion_widget_select_invalid() {
        let mut widget = CompletionWidget::new();
        let output = widget.select(0, "test", 4);
        assert!(matches!(output, CompletionOutput::Dismissed));
    }

    #[test]
    fn test_scroll_to_index_no_scroll_needed() {
        let mut scroll = ScrollState::new();
        scroll.offset = 0.0;
        scroll_to_index(&mut scroll, 0, 26.0, 300.0);
        assert_eq!(scroll.offset, 0.0);
    }

    #[test]
    fn test_scroll_to_index_scroll_down() {
        let mut scroll = ScrollState::new();
        scroll.offset = 0.0;
        // Item at index 15 (top = 390, bottom = 416) is below viewport (300px)
        scroll_to_index(&mut scroll, 15, 26.0, 300.0);
        // Should scroll so item bottom is at viewport bottom
        assert_eq!(scroll.offset, 15.0 * 26.0 + 26.0 - 300.0);
    }

    #[test]
    fn test_scroll_to_index_scroll_up() {
        let mut scroll = ScrollState::new();
        scroll.offset = 300.0;
        // Item at index 5 (top = 130) is above scroll offset
        scroll_to_index(&mut scroll, 5, 26.0, 300.0);
        assert_eq!(scroll.offset, 5.0 * 26.0);
    }
}
