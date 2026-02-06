//! TextInputElement - Editable text field.
//!
//! Renders single-line or multiline text inputs with cursor, selection,
//! and focus handling. All state is external - passed in via the element.

use unicode_width::UnicodeWidthChar;

use crate::content_address::SourceId;
use crate::layout_snapshot::{CursorIcon, LayoutSnapshot};
use crate::primitives::{Color, Rect, Size};
use crate::text_input_state::{TextInputState, compute_visual_lines, offset_to_visual};

use super::elements::{unicode_display_width, hash_text};
use super::length::{Length, Padding, CHAR_WIDTH, LINE_HEIGHT, BASE_FONT_SIZE};

// =========================================================================
// Helper Functions
// =========================================================================

/// Get the X offset in cell-width units for a given column index in a string.
/// Accounts for CJK (2-wide), combining marks (0-wide), etc.
fn unicode_col_x(text: &str, col: usize) -> f32 {
    text.chars()
        .take(col)
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0) as f32)
        .sum()
}

// =========================================================================
// TextInputElement
// =========================================================================

/// A text input element descriptor.
///
/// Renders an editable text field with cursor and optional selection highlight.
/// All state is external — the app passes text, cursor, selection, and focus.
pub struct TextInputElement {
    pub id: SourceId,
    pub text: String,
    pub cursor: usize,
    pub selection: Option<(usize, usize)>,
    pub focused: bool,
    pub placeholder: String,
    pub text_color: Color,
    pub placeholder_color: Color,
    pub background: Color,
    pub border_color: Color,
    pub focus_border_color: Color,
    pub border_width: f32,
    pub corner_radius: f32,
    pub padding: Padding,
    pub width: Length,
    pub multiline: bool,
    pub height: Length,
    pub scroll_offset: f32,
    pub cursor_visible: bool,
    pub(crate) cache_key: u64,
}

impl TextInputElement {
    pub fn new(id: SourceId, text: impl Into<String>) -> Self {
        let text = text.into();
        let cache_key = hash_text(&text);
        Self {
            id,
            text,
            cursor: 0,
            selection: None,
            focused: false,
            placeholder: String::new(),
            text_color: Color::WHITE,
            placeholder_color: Color::rgba(0.4, 0.4, 0.45, 1.0),
            background: Color::rgba(0.10, 0.10, 0.13, 1.0),
            border_color: Color::rgba(1.0, 1.0, 1.0, 0.12),
            focus_border_color: Color::rgba(0.3, 0.5, 0.8, 0.6),
            border_width: 1.0,
            corner_radius: 6.0,
            padding: Padding::new(8.0, 12.0, 8.0, 12.0),
            width: Length::Fill,
            multiline: false,
            height: Length::Shrink,
            scroll_offset: 0.0,
            cursor_visible: true,
            cache_key,
        }
    }

    /// Create from a `TextInputState`, copying all state-driven fields.
    ///
    /// This pulls id, text, cursor, selection, focused, scroll_offset, and
    /// multiline from the state, so you only need to chain visual overrides.
    pub fn from_state(state: &TextInputState) -> Self {
        let mut el = Self::new(state.id(), &state.text);
        el.cursor = state.cursor;
        el.selection = state.selection;
        el.focused = state.focused;
        el.scroll_offset = state.scroll_offset;
        el.multiline = state.is_multiline();
        el
    }

    pub fn cursor(mut self, pos: usize) -> Self { self.cursor = pos; self }
    pub fn selection(mut self, range: Option<(usize, usize)>) -> Self { self.selection = range; self }
    pub fn focused(mut self, focused: bool) -> Self { self.focused = focused; self }
    pub fn placeholder(mut self, text: impl Into<String>) -> Self { self.placeholder = text.into(); self }
    pub fn text_color(mut self, color: Color) -> Self { self.text_color = color; self }
    pub fn background(mut self, color: Color) -> Self { self.background = color; self }
    pub fn border_color(mut self, color: Color) -> Self { self.border_color = color; self }
    pub fn focus_border_color(mut self, color: Color) -> Self { self.focus_border_color = color; self }
    pub fn corner_radius(mut self, radius: f32) -> Self { self.corner_radius = radius; self }
    pub fn padding(mut self, padding: Padding) -> Self { self.padding = padding; self }
    pub fn width(mut self, width: Length) -> Self { self.width = width; self }
    pub fn multiline(mut self, multiline: bool) -> Self { self.multiline = multiline; self }
    pub fn height(mut self, height: Length) -> Self { self.height = height; self }
    pub fn scroll_offset(mut self, offset: f32) -> Self { self.scroll_offset = offset; self }
    pub fn cursor_visible(mut self, visible: bool) -> Self { self.cursor_visible = visible; self }

    pub(crate) fn estimate_size(&self) -> Size {
        let text_w = unicode_display_width(&self.text).max(20.0) * CHAR_WIDTH;
        if self.multiline {
            let line_count = self.text.lines().count().max(1) as f32;
            let content_h = line_count * LINE_HEIGHT + self.padding.vertical();
            let h = match self.height {
                Length::Fixed(px) => px,
                _ => content_h,
            };
            Size::new(text_w + self.padding.horizontal(), h)
        } else {
            Size::new(
                text_w + self.padding.horizontal(),
                LINE_HEIGHT + self.padding.vertical(),
            )
        }
    }
}

// =========================================================================
// Rendering Functions
// =========================================================================

/// Render a TextInputElement at the given position and size.
pub(crate) fn render_text_input(
    snapshot: &mut LayoutSnapshot,
    input: TextInputElement,
    x: f32, y: f32, w: f32, h: f32,
) {
    use crate::primitives::Point;

    let input_rect = Rect::new(x, y, w, h);

    // Background
    snapshot.primitives_mut().add_rounded_rect(input_rect, input.corner_radius, input.background);

    // Border
    let border_color = if input.focused { input.focus_border_color } else { input.border_color };
    snapshot.primitives_mut().add_border(input_rect, input.corner_radius, input.border_width, border_color);

    // Clip content
    snapshot.primitives_mut().push_clip(input_rect);

    let text_x = x + input.padding.left;
    let text_y = y + input.padding.top;

    // Pre-compute cursor X position before text is moved
    let cursor_x_offset = unicode_col_x(&input.text, input.cursor) * CHAR_WIDTH;

    // Selection highlight
    if let Some((sel_start, sel_end)) = input.selection {
        let s = sel_start.min(sel_end);
        let e = sel_start.max(sel_end);
        let sel_x = text_x + unicode_col_x(&input.text, s) * CHAR_WIDTH;
        let sel_w = (unicode_col_x(&input.text, e) - unicode_col_x(&input.text, s)) * CHAR_WIDTH;
        snapshot.primitives_mut().add_solid_rect(
            Rect::new(sel_x, text_y, sel_w, LINE_HEIGHT),
            Color::rgba(0.3, 0.5, 0.8, 0.4),
        );
    }

    // Text or placeholder
    if input.text.is_empty() && !input.focused {
        snapshot.primitives_mut().add_text_cached(
            input.placeholder.clone(),
            Point::new(text_x, text_y),
            input.placeholder_color,
            BASE_FONT_SIZE,
            hash_text(&input.placeholder),
        );
    } else {
        snapshot.primitives_mut().add_text_cached(
            input.text,
            Point::new(text_x, text_y),
            input.text_color,
            BASE_FONT_SIZE,
            input.cache_key,
        );
    }

    // Cursor (blinking)
    if input.focused && input.cursor_visible {
        let cursor_x = text_x + cursor_x_offset;
        snapshot.primitives_mut().add_solid_rect(
            Rect::new(cursor_x, text_y, 2.0, LINE_HEIGHT),
            Color::rgba(0.85, 0.85, 0.88, 0.8),
        );
    }

    snapshot.primitives_mut().pop_clip();

    // Register for hit-testing
    snapshot.register_widget(input.id, input_rect);
    snapshot.set_cursor_hint(input.id, CursorIcon::Text);
}

/// Render a multiline text input element (code editor style).
///
/// Supports vertical scrolling, per-line cursor positioning, and
/// per-line selection highlights. Only visible lines are rendered
/// (virtualized).
pub(crate) fn render_text_input_multiline(
    snapshot: &mut LayoutSnapshot,
    input: TextInputElement,
    x: f32, y: f32, w: f32, h: f32,
) {
    use crate::primitives::Point;

    let input_rect = Rect::new(x, y, w, h);

    // Background
    snapshot.primitives_mut().add_rounded_rect(input_rect, input.corner_radius, input.background);

    // Border
    let border_color = if input.focused { input.focus_border_color } else { input.border_color };
    snapshot.primitives_mut().add_border(input_rect, input.corner_radius, input.border_width, border_color);

    // Clip content area
    snapshot.primitives_mut().push_clip(input_rect);

    let text_x = x + input.padding.left;
    let text_y = y + input.padding.top;
    let visible_h = h - input.padding.vertical();
    let avail_w = w - input.padding.horizontal();

    // Compute max display columns for wrapping
    let max_cols = if avail_w <= CHAR_WIDTH {
        usize::MAX
    } else {
        (avail_w / CHAR_WIDTH).floor().max(1.0) as usize
    };

    // Compute visual lines (soft-wrapped)
    let visual_lines = compute_visual_lines(&input.text, max_cols);
    let logical_lines: Vec<&str> = if input.text.is_empty() {
        vec![""]
    } else {
        input.text.split('\n').collect()
    };

    // Compute visible line range (virtualized rendering)
    let first_visible = (input.scroll_offset / LINE_HEIGHT).floor().max(0.0) as usize;
    let visible_count = (visible_h / LINE_HEIGHT).ceil() as usize + 1;
    let last_visible = (first_visible + visible_count).min(visual_lines.len());

    // Compute cursor visual position
    let (cursor_vis_line, cursor_vis_col) = offset_to_visual(&visual_lines, input.cursor);

    // Selection highlight (per visual line)
    if let Some((sel_start, sel_end)) = input.selection {
        let s = sel_start.min(sel_end);
        let e = sel_start.max(sel_end);
        let (s_vis_line, s_vis_col) = offset_to_visual(&visual_lines, s);
        let (e_vis_line, e_vis_col) = offset_to_visual(&visual_lines, e);

        for vis_idx in s_vis_line..=e_vis_line {
            if vis_idx < first_visible || vis_idx >= last_visible { continue; }
            let vl = &visual_lines[vis_idx];
            let ll = logical_lines.get(vl.logical_line).copied().unwrap_or("");
            let vis_text = &ll[vl.start_byte..vl.end_byte];

            let col_start = if vis_idx == s_vis_line { s_vis_col } else { 0 };
            let col_end = if vis_idx == e_vis_line { e_vis_col } else { vl.char_count };

            let sel_x = text_x + unicode_col_x(vis_text, col_start) * CHAR_WIDTH;
            let sel_w = (unicode_col_x(vis_text, col_end) - unicode_col_x(vis_text, col_start)).max(1.0) * CHAR_WIDTH;
            let sel_y = text_y + vis_idx as f32 * LINE_HEIGHT - input.scroll_offset;
            snapshot.primitives_mut().add_solid_rect(
                Rect::new(sel_x, sel_y, sel_w, LINE_HEIGHT),
                Color::rgba(0.3, 0.5, 0.8, 0.4),
            );
        }
    }

    // Render visible visual lines
    if input.text.is_empty() && !input.focused {
        snapshot.primitives_mut().add_text_cached(
            input.placeholder.clone(),
            Point::new(text_x, text_y),
            input.placeholder_color,
            BASE_FONT_SIZE,
            hash_text(&input.placeholder),
        );
    } else {
        for vis_idx in first_visible..last_visible {
            let vl = &visual_lines[vis_idx];
            let ll = logical_lines.get(vl.logical_line).copied().unwrap_or("");
            let vis_text = &ll[vl.start_byte..vl.end_byte];
            let ly = text_y + vis_idx as f32 * LINE_HEIGHT - input.scroll_offset;
            if !vis_text.is_empty() {
                snapshot.primitives_mut().add_text_cached(
                    vis_text.to_string(),
                    Point::new(text_x, ly),
                    input.text_color,
                    BASE_FONT_SIZE,
                    hash_text(vis_text).wrapping_add(vis_idx as u64),
                );
            }
        }
    }

    // Cursor (blinking)
    if input.focused && input.cursor_visible {
        if let Some(vl) = visual_lines.get(cursor_vis_line) {
            let ll = logical_lines.get(vl.logical_line).copied().unwrap_or("");
            let vis_text = &ll[vl.start_byte..vl.end_byte];
            let cursor_x = text_x + unicode_col_x(vis_text, cursor_vis_col) * CHAR_WIDTH;
            let cursor_y = text_y + cursor_vis_line as f32 * LINE_HEIGHT - input.scroll_offset;
            snapshot.primitives_mut().add_solid_rect(
                Rect::new(cursor_x, cursor_y, 2.0, LINE_HEIGHT),
                Color::rgba(0.85, 0.85, 0.88, 0.8),
            );
        }
    }

    snapshot.primitives_mut().pop_clip();

    // Register for hit-testing
    snapshot.register_widget(input.id, input_rect);
    snapshot.set_cursor_hint(input.id, CursorIcon::Text);
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unicode_col_x_ascii() {
        assert_eq!(unicode_col_x("hello", 0), 0.0);
        assert_eq!(unicode_col_x("hello", 3), 3.0);
        assert_eq!(unicode_col_x("hello", 5), 5.0);
    }

    #[test]
    fn test_unicode_col_x_cjk() {
        // Each CJK char is 2 wide
        assert_eq!(unicode_col_x("中文字", 0), 0.0);
        assert_eq!(unicode_col_x("中文字", 1), 2.0);
        assert_eq!(unicode_col_x("中文字", 2), 4.0);
    }

    #[test]
    fn test_unicode_col_x_mixed() {
        // "a中b" = positions: a=0, 中=1, b=3
        assert_eq!(unicode_col_x("a中b", 0), 0.0);
        assert_eq!(unicode_col_x("a中b", 1), 1.0);
        assert_eq!(unicode_col_x("a中b", 2), 3.0);
    }

    #[test]
    fn test_unicode_col_x_beyond_length() {
        // Asking for column beyond string length returns full width
        assert_eq!(unicode_col_x("ab", 10), 2.0);
    }
}
