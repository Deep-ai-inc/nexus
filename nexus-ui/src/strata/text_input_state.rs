//! Text Input State
//!
//! Encapsulates all text editing state and operations for both single-line
//! and multi-line text inputs. Eliminates the need for applications to
//! manually implement cursor movement, selection, and text manipulation.

use std::cell::Cell;

use crate::strata::app::MouseResponse;
use crate::strata::content_address::SourceId;
use crate::strata::event_context::{
    CaptureState, Key, KeyEvent, MouseButton, MouseEvent, NamedKey,
};
use crate::strata::layout_snapshot::{HitResult, LayoutSnapshot};
use crate::strata::primitives::Rect;

/// Monospace character width (must match containers.rs CHAR_WIDTH).
const CHAR_WIDTH: f32 = 8.4;
/// Line height (must match containers.rs LINE_HEIGHT).
const LINE_HEIGHT: f32 = 18.0;

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
    /// Padding inside the widget (for mouse position calculation).
    padding: f32,
    /// Whether this is a multi-line editor.
    multiline: bool,
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
            padding: 6.0,
            multiline: false,
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
            padding: 6.0,
            multiline: false,
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
            padding: 6.0,
            multiline: false,
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
            padding: 6.0,
            multiline: true,
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
            padding: 6.0,
            multiline: true,
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
    }

    /// Insert a newline at the cursor position (multiline).
    pub fn insert_newline(&mut self) {
        self.delete_selection();
        let byte_pos = char_to_byte(&self.text, self.cursor);
        self.text.insert(byte_pos, '\n');
        self.cursor += 1;
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

    /// Move cursor up one line (multiline). Clears selection.
    pub fn move_up(&mut self) {
        self.selection = None;
        let (line, col) = line_col(&self.text, self.cursor);
        if line > 0 {
            self.cursor = line_col_to_offset(&self.text, line - 1, col);
        }
    }

    /// Move cursor down one line (multiline). Clears selection.
    pub fn move_down(&mut self) {
        self.selection = None;
        let (line, col) = line_col(&self.text, self.cursor);
        let line_count = self.text.split('\n').count();
        if line + 1 < line_count {
            self.cursor = line_col_to_offset(&self.text, line + 1, col);
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
        let line = ((rel_y + self.scroll_offset) / LINE_HEIGHT).floor().max(0.0) as usize;
        let col = (rel_x / CHAR_WIDTH).round().max(0.0) as usize;
        self.cursor = line_col_to_offset(&self.text, line, col);
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
        let line = ((rel_y + self.scroll_offset) / LINE_HEIGHT).floor().max(0.0) as usize;
        let col = (rel_x / CHAR_WIDTH).round().max(0.0) as usize;
        let pos = line_col_to_offset(&self.text, line, col);
        let anchor = self.selection.map(|(a, _)| a).unwrap_or(self.cursor);
        if pos != anchor {
            self.selection = Some((anchor, pos));
            self.cursor = pos;
        }
    }

    /// Scroll the multi-line editor by a delta (positive = scroll content up).
    pub fn scroll_by(&mut self, delta: f32) {
        self.scroll_offset = (self.scroll_offset - delta).max(0.0);
        let line_count = self.text.split('\n').count() as f32;
        let max_scroll = (line_count * LINE_HEIGHT - 80.0).max(0.0);
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
                        let rel_x = (position.x - bounds.x - self.padding).max(0.0);
                        if self.multiline {
                            let rel_y = (position.y - bounds.y - self.padding).max(0.0);
                            return Some(MouseResponse::message_and_capture(
                                TextInputMouseAction::Click2D(rel_x, rel_y),
                                self.id,
                            ));
                        } else {
                            return Some(MouseResponse::message_and_capture(
                                TextInputMouseAction::Click1D(rel_x),
                                self.id,
                            ));
                        }
                    }
                }
                None
            }
            MouseEvent::CursorMoved { position } => {
                if let CaptureState::Captured(id) = capture {
                    if *id == self.id {
                        let bounds = self.bounds.get();
                        let rel_x = (position.x - bounds.x - self.padding).max(0.0);
                        if self.multiline {
                            let rel_y = (position.y - bounds.y - self.padding).max(0.0);
                            return Some(MouseResponse::message(
                                TextInputMouseAction::Drag2D(rel_x, rel_y),
                            ));
                        } else {
                            return Some(MouseResponse::message(
                                TextInputMouseAction::Drag1D(rel_x),
                            ));
                        }
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

        let cmd = modifiers.meta || modifiers.ctrl;

        match (key, modifiers.shift, cmd) {
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
}
