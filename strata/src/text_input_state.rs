//! Text Input State
//!
//! Encapsulates all text editing state and operations for both single-line
//! and multi-line text inputs. Eliminates the need for applications to
//! manually implement cursor movement, selection, and text manipulation.

use std::cell::Cell;

use unicode_width::UnicodeWidthChar;

use crate::app::MouseResponse;
use crate::content_address::SourceId;
use crate::event_context::{
    CaptureState, Key, KeyEvent, MouseButton, MouseEvent, NamedKey,
};
use crate::layout::containers::Padding;
use crate::layout_snapshot::{HitResult, LayoutSnapshot};
use crate::primitives::Rect;

/// Monospace character width (must match containers.rs CHAR_WIDTH).
const CHAR_WIDTH: f32 = 8.4;
/// Line height (must match containers.rs LINE_HEIGHT).
const LINE_HEIGHT: f32 = 18.0;

// =========================================================================
// Visual line wrapping
// =========================================================================

/// A visual line produced by soft-wrapping a logical line.
#[derive(Debug, Clone)]
pub struct VisualLine {
    /// Index of the logical line (from `text.split('\n')`).
    pub logical_line: usize,
    /// Byte offset of the start within the logical line (for O(1) slicing).
    pub start_byte: usize,
    /// Byte offset of the end within the logical line.
    pub end_byte: usize,
    /// Char offset of the start within the logical line.
    pub start_col: usize,
    /// Number of chars in this visual line.
    pub char_count: usize,
    /// Display width in columns (accounting for unicode widths).
    pub display_width: usize,
    /// Absolute char offset from the start of the full text.
    pub global_char_start: usize,
}

/// Compute visual lines by greedy word-wrapping text at `max_cols` display columns.
///
/// Uses unicode display width for column calculations. Words that exceed
/// `max_cols` are character-broken as a fallback. When `max_cols == usize::MAX`,
/// wrapping is effectively disabled.
pub fn compute_visual_lines(text: &str, max_cols: usize) -> Vec<VisualLine> {
    let mut result = Vec::new();
    let mut global_char = 0usize;

    for (line_idx, logical_line) in text.split('\n').enumerate() {
        if logical_line.is_empty() {
            result.push(VisualLine {
                logical_line: line_idx,
                start_byte: 0,
                end_byte: 0,
                start_col: 0,
                char_count: 0,
                display_width: 0,
                global_char_start: global_char,
            });
            global_char += 1; // account for the '\n'
            continue;
        }

        // Greedy word wrapping
        let mut vis_start_byte = 0usize;
        let mut vis_start_col = 0usize;
        let mut vis_width = 0usize;
        let mut vis_chars = 0usize;
        let mut word_start_byte = 0usize;
        let mut word_start_col = 0usize;
        let mut word_width = 0usize;
        let mut word_chars = 0usize;
        let mut in_whitespace = false;

        let chars: Vec<(usize, char)> = logical_line.char_indices().collect();
        let line_byte_len = logical_line.len();

        for &(byte_idx, ch) in &chars {
            let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
            let is_ws = ch == ' ' || ch == '\t';

            if is_ws && !in_whitespace {
                // End of a word — commit the word to the current visual line
                // (word was already accumulated into vis_width)
                in_whitespace = true;
            }
            if !is_ws && in_whitespace {
                // Start of a new word after whitespace
                in_whitespace = false;
                word_start_byte = byte_idx;
                word_start_col = vis_start_col + vis_chars;
                word_width = 0;
                word_chars = 0;
            }

            // Check if adding this char would exceed max_cols
            if vis_width + cw > max_cols && vis_chars > 0 {
                if !is_ws && word_chars > 0 && word_chars < vis_chars {
                    // Wrap at word boundary: push visual line up to word start
                    let vis_end_byte = word_start_byte;
                    let chars_before_word = vis_chars - word_chars;
                    let width_before_word = vis_width - word_width;
                    result.push(VisualLine {
                        logical_line: line_idx,
                        start_byte: vis_start_byte,
                        end_byte: vis_end_byte,
                        start_col: vis_start_col,
                        char_count: chars_before_word,
                        display_width: width_before_word,
                        global_char_start: global_char + vis_start_col,
                    });
                    // Start new visual line from the word
                    vis_start_byte = word_start_byte;
                    vis_start_col = word_start_col;
                    vis_width = word_width + cw;
                    vis_chars = word_chars + 1;
                    word_width += cw;
                    word_chars += 1;
                } else {
                    // Character-break: current char starts a new line
                    let vis_end_byte = byte_idx;
                    result.push(VisualLine {
                        logical_line: line_idx,
                        start_byte: vis_start_byte,
                        end_byte: vis_end_byte,
                        start_col: vis_start_col,
                        char_count: vis_chars,
                        display_width: vis_width,
                        global_char_start: global_char + vis_start_col,
                    });
                    vis_start_byte = byte_idx;
                    vis_start_col += vis_chars;
                    vis_width = cw;
                    vis_chars = 1;
                    if !is_ws {
                        word_start_byte = byte_idx;
                        word_start_col = vis_start_col;
                        word_width = cw;
                        word_chars = 1;
                    } else {
                        word_width = 0;
                        word_chars = 0;
                    }
                }
            } else {
                vis_width += cw;
                vis_chars += 1;
                if !is_ws {
                    word_width += cw;
                    word_chars += 1;
                }
            }
        }

        // Push the last visual line
        result.push(VisualLine {
            logical_line: line_idx,
            start_byte: vis_start_byte,
            end_byte: line_byte_len,
            start_col: vis_start_col,
            char_count: vis_chars,
            display_width: vis_width,
            global_char_start: global_char + vis_start_col,
        });

        global_char += logical_line.chars().count() + 1; // +1 for '\n'
    }

    result
}

/// Convert a char offset to (visual_line_index, col_within_visual_line).
///
/// Uses binary search on `global_char_start` for O(log N) lookup.
/// At wrap boundaries, prefers the end of the current visual line (cursor affinity).
pub fn offset_to_visual(visual_lines: &[VisualLine], char_offset: usize) -> (usize, usize) {
    if visual_lines.is_empty() {
        return (0, 0);
    }
    // Binary search: find the last visual line whose global_char_start <= char_offset
    let idx = match visual_lines.binary_search_by_key(&char_offset, |vl| vl.global_char_start) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    };
    let vl = &visual_lines[idx];
    let col = char_offset.saturating_sub(vl.global_char_start);
    // Clamp col to char_count (cursor can sit at end of visual line)
    (idx, col.min(vl.char_count))
}

/// Convert (visual_line_index, col_within_visual_line) to a char offset.
///
/// Clamps vis_line and vis_col to valid ranges.
pub fn visual_to_offset(visual_lines: &[VisualLine], vis_line: usize, vis_col: usize) -> usize {
    if visual_lines.is_empty() {
        return 0;
    }
    let vis_line = vis_line.min(visual_lines.len() - 1);
    let vl = &visual_lines[vis_line];
    let col = vis_col.min(vl.char_count);
    vl.global_char_start + col
}

/// Result of a text input key/mouse interaction.
#[derive(Debug, Clone)]
pub enum TextInputAction {
    /// Text or cursor was modified.
    Changed,
    /// Enter pressed in single-line mode. Contains the submitted text.
    Submit(String),
    /// Escape pressed — request blur.
    Blur,
    /// No action taken.
    Noop,
}

/// Result of a text input mouse interaction.
///
/// Returned by `handle_mouse()` and consumed by `apply_mouse()`.
#[derive(Debug, Clone)]
pub enum TextInputMouseAction {
    /// Single-line click at relative x position.
    Click1D(f32),
    /// Multi-line click at relative (x, y) position.
    Click2D(f32, f32),
    /// Single-line drag to relative x position.
    Drag1D(f32),
    /// Multi-line drag to relative (x, y) position.
    Drag2D(f32, f32),
}

/// Encapsulates all text editing state and operations.
///
/// Use this in your app state instead of managing separate `text`, `cursor`,
/// `selection`, and `scroll_offset` fields.
///
/// # Example
/// ```ignore
/// struct MyState {
///     input: TextInputState,
///     editor: TextInputState,
/// }
/// ```
pub struct TextInputState {
    pub text: String,
    pub cursor: usize,
    pub selection: Option<(usize, usize)>,
    pub scroll_offset: f32,
    pub focused: bool,
    /// Widget ID for hit-testing (used by handle_mouse).
    id: SourceId,
    /// Widget bounds (synced from layout snapshot each frame).
    bounds: Cell<Rect>,
    /// Padding inside the widget (for mouse position and wrapping calculation).
    padding: Padding,
    /// Whether this is a multi-line editor.
    multiline: bool,
    /// Incremented on every text mutation for cache invalidation.
    text_revision: usize,
    /// Cached visual lines from soft-wrapping.
    cached_visual_lines: Vec<VisualLine>,
    /// The text_revision when the cache was last built.
    cache_revision: usize,
    /// The max_cols used when the cache was last built.
    cache_max_cols: usize,
}

impl TextInputState {
    /// Create an empty text input state with auto-generated ID.
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            selection: None,
            scroll_offset: 0.0,
            focused: false,
            id: SourceId::new(),
            bounds: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            padding: Padding::all(6.0),
            multiline: false,
            text_revision: 0,
            cached_visual_lines: Vec::new(),
            cache_revision: usize::MAX,
            cache_max_cols: 0,
        }
    }

    /// Create a text input state with initial text and auto-generated ID.
    pub fn with_text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cursor: 0,
            selection: None,
            scroll_offset: 0.0,
            focused: false,
            id: SourceId::new(),
            bounds: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            padding: Padding::all(6.0),
            multiline: false,
            text_revision: 0,
            cached_visual_lines: Vec::new(),
            cache_revision: usize::MAX,
            cache_max_cols: 0,
        }
    }

    /// Create a single-line text input with a named ID.
    pub fn single_line(name: &str) -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            selection: None,
            scroll_offset: 0.0,
            focused: false,
            id: SourceId::named(name),
            bounds: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            padding: Padding::all(6.0),
            multiline: false,
            text_revision: 0,
            cached_visual_lines: Vec::new(),
            cache_revision: usize::MAX,
            cache_max_cols: 0,
        }
    }

    /// Create a multi-line text editor with a named ID.
    pub fn multi_line(name: &str) -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            selection: None,
            scroll_offset: 0.0,
            focused: false,
            id: SourceId::named(name),
            bounds: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            padding: Padding::all(6.0),
            multiline: true,
            text_revision: 0,
            cached_visual_lines: Vec::new(),
            cache_revision: usize::MAX,
            cache_max_cols: 0,
        }
    }

    /// Create a multi-line text editor with a named ID and initial text.
    pub fn multi_line_with_text(name: &str, text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cursor: 0,
            selection: None,
            scroll_offset: 0.0,
            focused: false,
            id: SourceId::named(name),
            bounds: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            padding: Padding::all(6.0),
            multiline: true,
            text_revision: 0,
            cached_visual_lines: Vec::new(),
            cache_revision: usize::MAX,
            cache_max_cols: 0,
        }
    }

    /// Get the widget SourceId.
    pub fn id(&self) -> SourceId {
        self.id
    }

    /// Whether this input is multi-line.
    pub fn is_multiline(&self) -> bool {
        self.multiline
    }

    /// Set the padding (must match the element's padding for correct mouse
    /// and wrapping alignment).
    pub fn set_padding(&mut self, padding: Padding) {
        self.padding = padding;
    }

    /// Get the current bounds (set from layout snapshot).
    pub fn bounds(&self) -> Rect {
        self.bounds.get()
    }

    /// Sync widget bounds from the layout snapshot after layout.
    ///
    /// Call this in `view()` after `.layout()`. Uses `Cell` for interior
    /// mutability since `view()` takes `&Self::State`.
    pub fn sync_from_snapshot(&self, snapshot: &LayoutSnapshot) {
        if let Some(bounds) = snapshot.widget_bounds(&self.id) {
            self.bounds.set(bounds);
        }
    }

    /// Compute the max display columns for wrapping based on current bounds.
    fn max_display_cols(&self) -> usize {
        let avail = self.bounds.get().width - self.padding.horizontal();
        if avail <= CHAR_WIDTH {
            return usize::MAX;
        }
        (avail / CHAR_WIDTH).floor().max(1.0) as usize
    }

    /// Ensure cached visual lines are up-to-date. Returns the max_cols used.
    fn ensure_visual_lines(&mut self) -> usize {
        let max_cols = self.max_display_cols();
        if self.cache_revision != self.text_revision || self.cache_max_cols != max_cols {
            self.cached_visual_lines = compute_visual_lines(&self.text, max_cols);
            self.cache_revision = self.text_revision;
            self.cache_max_cols = max_cols;
        }
        max_cols
    }

    /// Get the total number of visual lines (after wrapping).
    pub fn visual_line_count(&mut self) -> usize {
        self.ensure_visual_lines();
        self.cached_visual_lines.len().max(1)
    }

    /// Compute the visual line count for a given available pixel width.
    ///
    /// This is a pure computation (no caching, no `&mut self`) so it can be
    /// called from `view()` where only `&self` is available.
    pub fn visual_line_count_for_width(&self, avail_width: f32) -> usize {
        let max_cols = if avail_width <= CHAR_WIDTH {
            usize::MAX
        } else {
            (avail_width / CHAR_WIDTH).floor().max(1.0) as usize
        };
        compute_visual_lines(&self.text, max_cols).len().max(1)
    }

    /// Mark text as changed (invalidates visual line cache).
    fn bump_revision(&mut self) {
        self.text_revision = self.text_revision.wrapping_add(1);
    }

    /// Focus this input.
    pub fn focus(&mut self) {
        self.focused = true;
    }

    /// Blur (unfocus) this input, clearing selection.
    pub fn blur(&mut self) {
        self.focused = false;
        self.selection = None;
    }

    // =====================================================================
    // Editing operations
    // =====================================================================

    /// Delete the current selection, if any. Returns true if a selection existed.
    pub fn delete_selection(&mut self) -> bool {
        if let Some((s, e)) = self.selection.take() {
            let (lo, hi) = (s.min(e), s.max(e));
            let lo_byte = char_to_byte(&self.text, lo);
            let hi_byte = char_to_byte(&self.text, hi);
            self.text.replace_range(lo_byte..hi_byte, "");
            self.cursor = lo;
            self.bump_revision();
            true
        } else {
            false
        }
    }

    /// Insert a string at the cursor position (deletes selection first).
    pub fn insert_str(&mut self, s: &str) {
        self.delete_selection();
        let byte_pos = char_to_byte(&self.text, self.cursor);
        self.text.insert_str(byte_pos, s);
        self.cursor += s.chars().count();
        self.bump_revision();
    }

    /// Insert a newline at the cursor position (multiline).
    pub fn insert_newline(&mut self) {
        self.delete_selection();
        let byte_pos = char_to_byte(&self.text, self.cursor);
        self.text.insert(byte_pos, '\n');
        self.cursor += 1;
        self.bump_revision();
    }

    /// Delete the character before the cursor (Backspace).
    pub fn backspace(&mut self) {
        if self.delete_selection() {
            return;
        }
        if self.cursor > 0 {
            self.cursor -= 1;
            let byte_pos = char_to_byte(&self.text, self.cursor);
            let next_byte = char_to_byte(&self.text, self.cursor + 1);
            self.text.replace_range(byte_pos..next_byte, "");
            self.bump_revision();
        }
    }

    /// Delete the character at the cursor (Delete key).
    pub fn delete(&mut self) {
        if self.delete_selection() {
            return;
        }
        let char_count = self.text.chars().count();
        if self.cursor < char_count {
            let byte_pos = char_to_byte(&self.text, self.cursor);
            let next_byte = char_to_byte(&self.text, self.cursor + 1);
            self.text.replace_range(byte_pos..next_byte, "");
            self.bump_revision();
        }
    }

    // =====================================================================
    // Cursor movement
    // =====================================================================

    /// Move cursor left, clearing selection.
    pub fn move_left(&mut self) {
        self.selection = None;
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move cursor right, clearing selection.
    pub fn move_right(&mut self) {
        self.selection = None;
        let len = self.text.chars().count();
        if self.cursor < len {
            self.cursor += 1;
        }
    }

    /// Move cursor up one visual line (multiline). Clears selection.
    pub fn move_up(&mut self) {
        self.selection = None;
        self.ensure_visual_lines();
        let (vis_line, vis_col) = offset_to_visual(&self.cached_visual_lines, self.cursor);
        if vis_line > 0 {
            self.cursor = visual_to_offset(&self.cached_visual_lines, vis_line - 1, vis_col);
        }
    }

    /// Move cursor down one visual line (multiline). Clears selection.
    pub fn move_down(&mut self) {
        self.selection = None;
        self.ensure_visual_lines();
        let (vis_line, vis_col) = offset_to_visual(&self.cached_visual_lines, self.cursor);
        if vis_line + 1 < self.cached_visual_lines.len() {
            self.cursor = visual_to_offset(&self.cached_visual_lines, vis_line + 1, vis_col);
        }
    }

    /// Move cursor to start of line. Clears selection.
    pub fn move_home(&mut self) {
        self.selection = None;
        // Walk backwards to find the start of the current line
        let mut offset = 0;
        for (i, ch) in self.text.chars().enumerate() {
            if i == self.cursor {
                break;
            }
            if ch == '\n' {
                offset = i + 1;
            }
        }
        self.cursor = offset;
    }

    /// Move cursor to end of line. Clears selection.
    pub fn move_end(&mut self) {
        self.selection = None;
        let mut pos = self.cursor;
        for ch in self.text.chars().skip(self.cursor) {
            if ch == '\n' {
                break;
            }
            pos += 1;
        }
        self.cursor = pos;
    }

    // =====================================================================
    // Word-level operations
    // =====================================================================

    /// Move cursor one word to the left. Clears selection.
    pub fn move_word_left(&mut self) {
        self.selection = None;
        self.cursor = word_boundary_left(&self.text, self.cursor);
    }

    /// Move cursor one word to the right. Clears selection.
    pub fn move_word_right(&mut self) {
        self.selection = None;
        self.cursor = word_boundary_right(&self.text, self.cursor);
    }

    /// Delete one word backwards (Ctrl+W / Alt+Backspace).
    pub fn delete_word_back(&mut self) {
        if self.delete_selection() {
            return;
        }
        let target = word_boundary_left(&self.text, self.cursor);
        if target < self.cursor {
            let lo_byte = char_to_byte(&self.text, target);
            let hi_byte = char_to_byte(&self.text, self.cursor);
            self.text.replace_range(lo_byte..hi_byte, "");
            self.cursor = target;
            self.bump_revision();
        }
    }

    /// Delete one word forward (Alt+D).
    pub fn delete_word_forward(&mut self) {
        if self.delete_selection() {
            return;
        }
        let target = word_boundary_right(&self.text, self.cursor);
        if target > self.cursor {
            let lo_byte = char_to_byte(&self.text, self.cursor);
            let hi_byte = char_to_byte(&self.text, target);
            self.text.replace_range(lo_byte..hi_byte, "");
            self.bump_revision();
        }
    }

    /// Delete from cursor to start of line (Ctrl+U).
    pub fn kill_to_line_start(&mut self) {
        if self.delete_selection() {
            return;
        }
        // Find line start (same logic as move_home)
        let mut line_start = 0;
        for (i, ch) in self.text.chars().enumerate() {
            if i == self.cursor {
                break;
            }
            if ch == '\n' {
                line_start = i + 1;
            }
        }
        if line_start < self.cursor {
            let lo_byte = char_to_byte(&self.text, line_start);
            let hi_byte = char_to_byte(&self.text, self.cursor);
            self.text.replace_range(lo_byte..hi_byte, "");
            self.cursor = line_start;
            self.bump_revision();
        }
    }

    /// Delete from cursor to end of line (Ctrl+K).
    pub fn kill_to_line_end(&mut self) {
        if self.delete_selection() {
            return;
        }
        // Find line end (same logic as move_end)
        let mut line_end = self.cursor;
        for ch in self.text.chars().skip(self.cursor) {
            if ch == '\n' {
                break;
            }
            line_end += 1;
        }
        if line_end > self.cursor {
            let lo_byte = char_to_byte(&self.text, self.cursor);
            let hi_byte = char_to_byte(&self.text, line_end);
            self.text.replace_range(lo_byte..hi_byte, "");
            self.bump_revision();
        }
    }

    // =====================================================================
    // Selection
    // =====================================================================

    /// Extend selection one character to the left.
    pub fn select_left(&mut self) {
        let anchor = self.selection.map(|(a, _)| a).unwrap_or(self.cursor);
        if self.cursor > 0 {
            self.cursor -= 1;
            self.selection = Some((anchor, self.cursor));
        }
    }

    /// Extend selection one character to the right.
    pub fn select_right(&mut self) {
        let anchor = self.selection.map(|(a, _)| a).unwrap_or(self.cursor);
        let len = self.text.chars().count();
        if self.cursor < len {
            self.cursor += 1;
            self.selection = Some((anchor, self.cursor));
        }
    }

    /// Select all text.
    pub fn select_all(&mut self) {
        let len = self.text.chars().count();
        self.selection = Some((0, len));
        self.cursor = len;
    }

    // =====================================================================
    // Mouse interaction
    // =====================================================================

    /// Handle a single-line click at a relative x position.
    pub fn click_at(&mut self, rel_x: f32) {
        let char_count = self.text.chars().count();
        let pos = (rel_x / CHAR_WIDTH).round().max(0.0) as usize;
        self.cursor = pos.min(char_count);
        self.selection = None;
    }

    /// Handle a multi-line click at a relative (x, y) position.
    pub fn click_at_2d(&mut self, rel_x: f32, rel_y: f32) {
        self.ensure_visual_lines();
        let vis_line = ((rel_y + self.scroll_offset) / LINE_HEIGHT).floor().max(0.0) as usize;
        let vis_col = (rel_x / CHAR_WIDTH).round().max(0.0) as usize;
        self.cursor = visual_to_offset(&self.cached_visual_lines, vis_line, vis_col);
        self.selection = None;
    }

    /// Handle single-line drag to a relative x position.
    pub fn drag_to(&mut self, rel_x: f32) {
        let len = self.text.chars().count();
        let pos = (rel_x / CHAR_WIDTH).round().max(0.0) as usize;
        let pos = pos.min(len);
        let anchor = self.selection.map(|(a, _)| a).unwrap_or(self.cursor);
        if pos != anchor {
            self.selection = Some((anchor, pos));
            self.cursor = pos;
        }
    }

    /// Handle multi-line drag to a relative (x, y) position.
    pub fn drag_to_2d(&mut self, rel_x: f32, rel_y: f32) {
        self.ensure_visual_lines();
        let vis_line = ((rel_y + self.scroll_offset) / LINE_HEIGHT).floor().max(0.0) as usize;
        let vis_col = (rel_x / CHAR_WIDTH).round().max(0.0) as usize;
        let pos = visual_to_offset(&self.cached_visual_lines, vis_line, vis_col);
        let anchor = self.selection.map(|(a, _)| a).unwrap_or(self.cursor);
        if pos != anchor {
            self.selection = Some((anchor, pos));
            self.cursor = pos;
        }
    }

    /// Scroll the multi-line editor by a delta (positive = scroll content up).
    pub fn scroll_by(&mut self, delta: f32) {
        self.scroll_offset = (self.scroll_offset - delta).max(0.0);
        let vis_line_count = self.visual_line_count() as f32;
        let visible_h = self.bounds.get().height - self.padding.vertical();
        let max_scroll = (vis_line_count * LINE_HEIGHT - visible_h).max(0.0);
        self.scroll_offset = self.scroll_offset.min(max_scroll);
    }

    // =====================================================================
    // Composable mouse handler
    // =====================================================================

    /// Handle a mouse event for this text input.
    ///
    /// Returns `Some(MouseResponse<TextInputMouseAction>)` if this input
    /// consumed the event, `None` otherwise. Use with `MouseResponse::map()`
    /// to convert to your app's message type:
    ///
    /// ```ignore
    /// if let Some(r) = state.input.handle_mouse(&event, &hit, capture) {
    ///     return r.map(AppMessage::InputMouse);
    /// }
    /// ```
    pub fn handle_mouse(
        &self,
        event: &MouseEvent,
        hit: &Option<HitResult>,
        capture: &CaptureState,
    ) -> Option<MouseResponse<TextInputMouseAction>> {
        match event {
            MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                position,
            } => {
                if let Some(HitResult::Widget(id)) = hit {
                    if *id == self.id {
                        let bounds = self.bounds.get();
                        let rel_x = (position.x - bounds.x - self.padding.left).max(0.0);
                        let rel_y = (position.y - bounds.y - self.padding.top).max(0.0);
                        return Some(MouseResponse::message_and_capture(
                            TextInputMouseAction::Click2D(rel_x, rel_y),
                            self.id,
                        ));
                    }
                }
                None
            }
            MouseEvent::CursorMoved { position } => {
                if let CaptureState::Captured(id) = capture {
                    if *id == self.id {
                        let bounds = self.bounds.get();
                        let rel_x = (position.x - bounds.x - self.padding.left).max(0.0);
                        let rel_y = (position.y - bounds.y - self.padding.top).max(0.0);
                        return Some(MouseResponse::message(
                            TextInputMouseAction::Drag2D(rel_x, rel_y),
                        ));
                    }
                }
                None
            }
            MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            } => {
                if let CaptureState::Captured(id) = capture {
                    if *id == self.id {
                        return Some(MouseResponse::release());
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Apply a mouse action from `handle_mouse()`.
    ///
    /// Call this from `update()`. Automatically focuses the input and
    /// dispatches to the appropriate click/drag method.
    pub fn apply_mouse(&mut self, action: TextInputMouseAction) {
        self.focus();
        match action {
            TextInputMouseAction::Click1D(x) => self.click_at(x),
            TextInputMouseAction::Click2D(x, y) => self.click_at_2d(x, y),
            TextInputMouseAction::Drag1D(x) => self.drag_to(x),
            TextInputMouseAction::Drag2D(x, y) => self.drag_to_2d(x, y),
        }
    }

    // =====================================================================
    // High-level key handler
    // =====================================================================

    /// Handle a key event, performing the appropriate mutation.
    ///
    /// Call this from `update()` (not `on_key()`, since this mutates state).
    /// Returns a `TextInputAction` indicating what happened.
    ///
    /// When `multiline` is true:
    /// - Enter inserts a newline instead of submitting
    /// - Up/Down navigate lines
    ///
    /// When `multiline` is false:
    /// - Enter triggers `TextInputAction::Submit`
    /// - Up/Down are ignored
    pub fn handle_key(&mut self, event: &KeyEvent, multiline: bool) -> TextInputAction {
        let (key, modifiers, text) = match event {
            KeyEvent::Pressed { key, modifiers, text } => (key, modifiers, text.as_deref()),
            KeyEvent::Released { .. } => return TextInputAction::Noop,
        };

        let cmd = modifiers.meta;
        let ctrl = modifiers.ctrl;
        let alt = modifiers.alt;

        // Ctrl+key — readline/Emacs editing (before cmd matching)
        if ctrl && !cmd {
            match key {
                Key::Character(c) if c == "a" => {
                    self.move_home();
                    return TextInputAction::Changed;
                }
                Key::Character(c) if c == "e" => {
                    self.move_end();
                    return TextInputAction::Changed;
                }
                Key::Character(c) if c == "w" => {
                    self.delete_word_back();
                    return TextInputAction::Changed;
                }
                Key::Character(c) if c == "u" => {
                    self.kill_to_line_start();
                    return TextInputAction::Changed;
                }
                Key::Character(c) if c == "k" => {
                    self.kill_to_line_end();
                    return TextInputAction::Changed;
                }
                _ => {}
            }
        }

        // Alt+key — word-level navigation and deletion
        if alt && !cmd {
            match key {
                Key::Character(c) if c == "b" => {
                    self.move_word_left();
                    return TextInputAction::Changed;
                }
                Key::Character(c) if c == "f" => {
                    self.move_word_right();
                    return TextInputAction::Changed;
                }
                Key::Character(c) if c == "d" => {
                    self.delete_word_forward();
                    return TextInputAction::Changed;
                }
                Key::Named(NamedKey::Backspace) => {
                    self.delete_word_back();
                    return TextInputAction::Changed;
                }
                _ => {}
            }
        }

        // Cmd+Arrow — macOS-style word/line jump
        if cmd && !ctrl {
            match key {
                Key::Named(NamedKey::ArrowLeft) => {
                    self.move_home();
                    return TextInputAction::Changed;
                }
                Key::Named(NamedKey::ArrowRight) => {
                    self.move_end();
                    return TextInputAction::Changed;
                }
                Key::Named(NamedKey::Backspace) => {
                    self.kill_to_line_start();
                    return TextInputAction::Changed;
                }
                _ => {}
            }
        }

        // Alt+Arrow — word jump (macOS Option+Arrow convention)
        if alt && !cmd && !ctrl {
            match key {
                Key::Named(NamedKey::ArrowLeft) => {
                    self.move_word_left();
                    return TextInputAction::Changed;
                }
                Key::Named(NamedKey::ArrowRight) => {
                    self.move_word_right();
                    return TextInputAction::Changed;
                }
                _ => {}
            }
        }

        let cmd_or_ctrl = cmd || ctrl;

        match (key, modifiers.shift, cmd_or_ctrl) {
            (Key::Named(NamedKey::Escape), _, _) => {
                self.blur();
                TextInputAction::Blur
            }
            (Key::Named(NamedKey::Enter), _, _) => {
                if multiline {
                    self.insert_newline();
                    TextInputAction::Changed
                } else {
                    let t = self.text.clone();
                    self.text.clear();
                    self.cursor = 0;
                    self.selection = None;
                    self.bump_revision();
                    TextInputAction::Submit(t)
                }
            }
            (Key::Named(NamedKey::Backspace), _, _) => {
                self.backspace();
                TextInputAction::Changed
            }
            (Key::Named(NamedKey::Delete), _, _) => {
                self.delete();
                TextInputAction::Changed
            }
            (Key::Named(NamedKey::ArrowLeft), true, _) => {
                self.select_left();
                TextInputAction::Changed
            }
            (Key::Named(NamedKey::ArrowRight), true, _) => {
                self.select_right();
                TextInputAction::Changed
            }
            (Key::Named(NamedKey::ArrowLeft), _, _) => {
                self.move_left();
                TextInputAction::Changed
            }
            (Key::Named(NamedKey::ArrowRight), _, _) => {
                self.move_right();
                TextInputAction::Changed
            }
            (Key::Named(NamedKey::ArrowUp), _, _) if multiline => {
                self.move_up();
                TextInputAction::Changed
            }
            (Key::Named(NamedKey::ArrowDown), _, _) if multiline => {
                self.move_down();
                TextInputAction::Changed
            }
            (Key::Named(NamedKey::Home), _, _) => {
                self.move_home();
                TextInputAction::Changed
            }
            (Key::Named(NamedKey::End), _, _) => {
                self.move_end();
                TextInputAction::Changed
            }
            (Key::Character(c), _, true) if c == "a" => {
                self.select_all();
                TextInputAction::Changed
            }
            // Use OS-provided text for character insertion (handles shift, compose, dead keys)
            (Key::Character(_), _, false) | (Key::Named(NamedKey::Space), _, false) => {
                if let Some(t) = text {
                    if !t.is_empty() {
                        self.insert_str(t);
                        return TextInputAction::Changed;
                    }
                }
                // Fallback: use logical key directly
                if let Key::Character(c) = key {
                    self.insert_str(c);
                } else {
                    self.insert_str(" ");
                }
                TextInputAction::Changed
            }
            _ => TextInputAction::Noop,
        }
    }
}

impl Default for TextInputState {
    fn default() -> Self {
        Self::new()
    }
}

// =========================================================================
// Helper functions
// =========================================================================

/// Convert a char offset to a byte offset in the string.
fn char_to_byte(text: &str, char_offset: usize) -> usize {
    text.char_indices()
        .nth(char_offset)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

/// Convert a char offset to (line, col) in newline-delimited text.
pub fn line_col(text: &str, char_offset: usize) -> (usize, usize) {
    let mut line = 0;
    let mut col = 0;
    for (i, ch) in text.chars().enumerate() {
        if i == char_offset {
            return (line, col);
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Convert (line, col) back to a char offset, clamping col to line length.
pub fn line_col_to_offset(text: &str, target_line: usize, target_col: usize) -> usize {
    let mut offset = 0;
    for (line_idx, line) in text.split('\n').enumerate() {
        if line_idx == target_line {
            let line_len = line.chars().count();
            return offset + target_col.min(line_len);
        }
        offset += line.chars().count() + 1; // +1 for '\n'
    }
    text.chars().count() // past end
}

/// Find the word boundary to the left of `pos`.
///
/// Skips whitespace/punctuation, then skips word characters.
/// Matches readline/Emacs `backward-word` behavior.
fn word_boundary_left(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut i = pos;
    // Skip whitespace/punctuation
    while i > 0 && !chars[i - 1].is_alphanumeric() && chars[i - 1] != '_' {
        i -= 1;
    }
    // Skip word characters
    while i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_') {
        i -= 1;
    }
    i
}

/// Find the word boundary to the right of `pos`.
///
/// Skips word characters, then skips whitespace/punctuation.
/// Matches readline/Emacs `forward-word` behavior.
fn word_boundary_right(text: &str, pos: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = pos;
    // Skip word characters
    while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
        i += 1;
    }
    // Skip whitespace/punctuation
    while i < len && !chars[i].is_alphanumeric() && chars[i] != '_' {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_and_cursor() {
        let mut state = TextInputState::new();
        state.insert_str("hello");
        assert_eq!(state.text, "hello");
        assert_eq!(state.cursor, 5);

        state.cursor = 2;
        state.insert_str("XY");
        assert_eq!(state.text, "heXYllo");
        assert_eq!(state.cursor, 4);
    }

    #[test]
    fn backspace_and_delete() {
        let mut state = TextInputState::with_text("abcde");
        state.cursor = 3;

        state.backspace();
        assert_eq!(state.text, "abde");
        assert_eq!(state.cursor, 2);

        state.delete();
        assert_eq!(state.text, "abe");
        assert_eq!(state.cursor, 2);
    }

    #[test]
    fn selection_delete() {
        let mut state = TextInputState::with_text("hello world");
        state.selection = Some((2, 7)); // "llo w"
        state.delete_selection();
        assert_eq!(state.text, "heorld");
        assert_eq!(state.cursor, 2);
        assert_eq!(state.selection, None);
    }

    #[test]
    fn selection_replace() {
        let mut state = TextInputState::with_text("hello world");
        state.selection = Some((5, 11)); // " world"
        state.insert_str("!");
        assert_eq!(state.text, "hello!");
        assert_eq!(state.cursor, 6);
    }

    #[test]
    fn line_col_conversion() {
        let text = "abc\ndef\nghi";
        assert_eq!(line_col(text, 0), (0, 0)); // 'a'
        assert_eq!(line_col(text, 3), (0, 3)); // '\n'
        assert_eq!(line_col(text, 4), (1, 0)); // 'd'
        assert_eq!(line_col(text, 8), (2, 0)); // 'g'

        assert_eq!(line_col_to_offset(text, 0, 0), 0);
        assert_eq!(line_col_to_offset(text, 1, 0), 4);
        assert_eq!(line_col_to_offset(text, 2, 2), 10);
        // Clamped col
        assert_eq!(line_col_to_offset(text, 0, 100), 3);
    }

    #[test]
    fn move_up_down() {
        let mut state = TextInputState::with_text("abc\ndef\nghi");
        state.cursor = 5; // 'e' on line 1, col 1
        state.move_up();
        assert_eq!(state.cursor, 1); // 'b' on line 0, col 1

        state.move_down();
        assert_eq!(state.cursor, 5); // 'e' on line 1, col 1

        state.move_down();
        assert_eq!(state.cursor, 9); // 'h'... col 1 on line 2
    }

    // =================================================================
    // Word boundary tests
    // =================================================================

    #[test]
    fn word_boundary_left_basic() {
        let text = "hello world";
        assert_eq!(word_boundary_left(text, 11), 6); // end → start of "world"
        assert_eq!(word_boundary_left(text, 6), 0);  // start of "world" → start of "hello"
        assert_eq!(word_boundary_left(text, 3), 0);  // mid-"hello" → start
        assert_eq!(word_boundary_left(text, 0), 0);  // already at start
    }

    #[test]
    fn word_boundary_right_basic() {
        let text = "hello world";
        assert_eq!(word_boundary_right(text, 0), 6);  // start → past "hello " to "world"
        assert_eq!(word_boundary_right(text, 6), 11); // "world" → end
        assert_eq!(word_boundary_right(text, 11), 11); // already at end
    }

    #[test]
    fn word_boundary_with_punctuation() {
        let text = "foo--bar baz";
        // From end of "foo": skip "--" then land at start of "foo"
        assert_eq!(word_boundary_left(text, 5), 0);
        // From 0: skip "foo", skip "--" → at "bar"
        assert_eq!(word_boundary_right(text, 0), 5);
    }

    #[test]
    fn word_boundary_underscores() {
        let text = "foo_bar baz";
        // foo_bar is one word (underscores are word chars)
        assert_eq!(word_boundary_left(text, 7), 0);
        assert_eq!(word_boundary_right(text, 0), 8); // past "foo_bar "
    }

    // =================================================================
    // Word navigation tests
    // =================================================================

    #[test]
    fn move_word_left_right() {
        let mut state = TextInputState::with_text("hello world foo");
        state.cursor = 15; // end
        state.move_word_left();
        assert_eq!(state.cursor, 12); // start of "foo"
        state.move_word_left();
        assert_eq!(state.cursor, 6); // start of "world"
        state.move_word_left();
        assert_eq!(state.cursor, 0); // start of "hello"

        state.move_word_right();
        assert_eq!(state.cursor, 6); // past "hello " → at "world"
        state.move_word_right();
        assert_eq!(state.cursor, 12); // past "world " → at "foo"
        assert!(state.selection.is_none());
    }

    // =================================================================
    // Word/line deletion tests
    // =================================================================

    #[test]
    fn delete_word_back_basic() {
        let mut state = TextInputState::with_text("hello world");
        state.cursor = 11;
        state.delete_word_back();
        assert_eq!(state.text, "hello ");
        assert_eq!(state.cursor, 6);

        state.delete_word_back();
        assert_eq!(state.text, "");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn delete_word_back_with_selection() {
        let mut state = TextInputState::with_text("hello world");
        state.selection = Some((2, 7));
        state.delete_word_back();
        assert_eq!(state.text, "heorld");
        assert_eq!(state.cursor, 2);
    }

    #[test]
    fn delete_word_forward_basic() {
        let mut state = TextInputState::with_text("hello world");
        state.cursor = 0;
        state.delete_word_forward();
        assert_eq!(state.text, "world");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn kill_to_line_start_basic() {
        let mut state = TextInputState::with_text("hello world");
        state.cursor = 7;
        state.kill_to_line_start();
        assert_eq!(state.text, "orld");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn kill_to_line_start_multiline() {
        let mut state = TextInputState::with_text("abc\ndef\nghi");
        state.cursor = 6; // 'f' on line 1
        state.kill_to_line_start();
        assert_eq!(state.text, "abc\nf\nghi");
        assert_eq!(state.cursor, 4); // start of line 1
    }

    #[test]
    fn kill_to_line_end_basic() {
        let mut state = TextInputState::with_text("hello world");
        state.cursor = 5;
        state.kill_to_line_end();
        assert_eq!(state.text, "hello");
        assert_eq!(state.cursor, 5);
    }

    #[test]
    fn kill_to_line_end_multiline() {
        let mut state = TextInputState::with_text("abc\ndef\nghi");
        state.cursor = 4; // 'd' on line 1
        state.kill_to_line_end();
        assert_eq!(state.text, "abc\n\nghi");
        assert_eq!(state.cursor, 4);
    }

    #[test]
    fn move_home_end_multiline() {
        let mut state = TextInputState::with_text("abc\ndef\nghi");
        state.cursor = 5; // 'e' on line 1
        state.move_home();
        assert_eq!(state.cursor, 4); // start of line 1
        state.move_end();
        assert_eq!(state.cursor, 7); // end of line 1 (before '\n')
    }

    #[test]
    fn click_and_drag() {
        let mut state = TextInputState::with_text("hello");
        state.click_at(CHAR_WIDTH * 2.6); // rounds to 3
        assert_eq!(state.cursor, 3);
        assert_eq!(state.selection, None);

        state.drag_to(CHAR_WIDTH * 4.4); // rounds to 4
        assert_eq!(state.cursor, 4);
        assert_eq!(state.selection, Some((3, 4)));
    }

    // =================================================================
    // Visual line wrapping tests
    // =================================================================

    #[test]
    fn compute_visual_lines_no_wrap() {
        let text = "hello";
        let vls = compute_visual_lines(text, 80);
        assert_eq!(vls.len(), 1);
        assert_eq!(vls[0].logical_line, 0);
        assert_eq!(vls[0].char_count, 5);
        assert_eq!(vls[0].global_char_start, 0);
    }

    #[test]
    fn compute_visual_lines_char_break() {
        // 10 chars, max_cols=4 → 3 visual lines: 4, 4, 2
        let text = "abcdefghij";
        let vls = compute_visual_lines(text, 4);
        assert_eq!(vls.len(), 3);
        assert_eq!(vls[0].char_count, 4);
        assert_eq!(vls[0].global_char_start, 0);
        assert_eq!(vls[1].char_count, 4);
        assert_eq!(vls[1].global_char_start, 4);
        assert_eq!(vls[2].char_count, 2);
        assert_eq!(vls[2].global_char_start, 8);
    }

    #[test]
    fn compute_visual_lines_word_wrap() {
        // "hello world" with max_cols=7 → wraps at word boundary
        // "hello " (6 cols) fits, "w" would make 7 → "hello w" fits (7 <= 7)
        // Actually "hello world" = 11 chars. At max_cols=7: "hello w" = 7, then "orld" = 4
        // Wait — word wrap should keep "world" together.
        // "hello " = 6 display cols. Adding "world" = 5 → 11 > 7. So wrap before "world".
        let text = "hello world";
        let vls = compute_visual_lines(text, 7);
        assert_eq!(vls.len(), 2);
        // First visual line: "hello " (6 chars)
        assert_eq!(&text[vls[0].start_byte..vls[0].end_byte], "hello ");
        // Second visual line: "world" (5 chars)
        assert_eq!(&text[vls[1].start_byte..vls[1].end_byte], "world");
    }

    #[test]
    fn compute_visual_lines_multiline() {
        let text = "abc\ndefgh";
        let vls = compute_visual_lines(text, 4);
        // Line 0: "abc" → 1 visual line
        // Line 1: "defgh" → "defg" + "h" → 2 visual lines
        assert_eq!(vls.len(), 3);
        assert_eq!(vls[0].logical_line, 0);
        assert_eq!(vls[0].char_count, 3);
        assert_eq!(vls[0].global_char_start, 0);
        assert_eq!(vls[1].logical_line, 1);
        assert_eq!(vls[1].char_count, 4);
        assert_eq!(vls[1].global_char_start, 4); // "abc\n" = 4 chars
        assert_eq!(vls[2].logical_line, 1);
        assert_eq!(vls[2].char_count, 1);
        assert_eq!(vls[2].global_char_start, 8);
    }

    #[test]
    fn compute_visual_lines_empty() {
        let text = "";
        let vls = compute_visual_lines(text, 80);
        assert_eq!(vls.len(), 1);
        assert_eq!(vls[0].char_count, 0);
    }

    #[test]
    fn offset_to_visual_basic() {
        let text = "abcdefghij";
        let vls = compute_visual_lines(text, 4);
        // vls: [0..4], [4..8], [8..10]
        assert_eq!(offset_to_visual(&vls, 0), (0, 0));
        assert_eq!(offset_to_visual(&vls, 3), (0, 3));
        assert_eq!(offset_to_visual(&vls, 4), (1, 0));
        assert_eq!(offset_to_visual(&vls, 9), (2, 1));
    }

    #[test]
    fn visual_to_offset_basic() {
        let text = "abcdefghij";
        let vls = compute_visual_lines(text, 4);
        assert_eq!(visual_to_offset(&vls, 0, 0), 0);
        assert_eq!(visual_to_offset(&vls, 1, 0), 4);
        assert_eq!(visual_to_offset(&vls, 2, 1), 9);
        // Clamped col
        assert_eq!(visual_to_offset(&vls, 2, 100), 10);
    }

    #[test]
    fn visual_to_offset_clamped_line() {
        let text = "abc";
        let vls = compute_visual_lines(text, 80);
        // Line 100 doesn't exist, clamp to last
        assert_eq!(visual_to_offset(&vls, 100, 2), 2);
    }
}
