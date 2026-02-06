//! Layout Containers
//!
//! Flexbox-inspired layout containers that compute child positions.
//! The layout computation happens ONCE when `layout()` is called,
//! not during widget construction.

use unicode_width::UnicodeWidthChar;

use crate::content_address::SourceId;
use crate::layout_snapshot::{CursorIcon, LayoutSnapshot};
use crate::primitives::{Color, Rect, Size};
use crate::scroll_state::ScrollState;
use crate::text_input_state::{TextInputState, compute_visual_lines, offset_to_visual};

// Import and re-export core layout types from length module
pub use super::length::{Length, Alignment, CrossAxisAlignment, Padding};
use super::length::{CHAR_WIDTH, LINE_HEIGHT, BASE_FONT_SIZE};

// Import and re-export element types from elements module
pub use super::elements::{TextElement, TerminalElement, ImageElement, ButtonElement};
use super::elements::{unicode_display_width, hash_text};

// Import and re-export LayoutChild and Widget from child module (the central switchboard)
pub use super::child::{LayoutChild, Widget};

// Re-export FlowContainer from flow module (for backward compatibility)
pub use super::flow::FlowContainer;

/// Get the X offset in cell-width units for a given column index in a string.
/// Accounts for CJK (2-wide), combining marks (0-wide), etc.
fn unicode_col_x(text: &str, col: usize) -> f32 {
    text.chars()
        .take(col)
        .map(|c| UnicodeWidthChar::width(c).unwrap_or(0) as f32)
        .sum()
}

// =========================================================================
// TextInputElement Rendering
// =========================================================================

/// Render a TextInputElement at the given position and size.
fn render_text_input(
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
fn render_text_input_multiline(
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
// TextInputElement (stays in containers.rs for now)
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
    cache_key: u64,
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
// Table Element
// =========================================================================

/// A column definition for a table.
pub struct TableColumn {
    pub name: String,
    pub width: f32,
    pub sort_id: Option<SourceId>,
}

/// A cell in a table row.
pub struct TableCell {
    pub text: String,
    /// Pre-wrapped lines. If empty, `text` is rendered as a single line.
    pub lines: Vec<String>,
    pub color: Color,
    /// Optional widget ID for clickable cells (anchors).
    /// When set, the cell registers as a clickable widget with `CursorIcon::Pointer`.
    pub widget_id: Option<SourceId>,
}

/// A table element with column headers and data rows.
///
/// Headers register as clickable widgets (via `sort_id`) for sort interaction.
/// Data rows render as text primitives. All state is external.
pub struct TableElement {
    pub source_id: SourceId,
    pub columns: Vec<TableColumn>,
    pub rows: Vec<Vec<TableCell>>,
    pub header_bg: Color,
    pub header_text_color: Color,
    pub row_height: f32,
    pub line_height: f32,
    pub row_padding: f32,
    pub header_height: f32,
    pub stripe_color: Option<Color>,
    pub separator_color: Color,
}

impl TableElement {
    pub fn new(source_id: SourceId) -> Self {
        Self {
            source_id,
            columns: Vec::new(),
            rows: Vec::new(),
            header_bg: Color::rgba(0.15, 0.15, 0.2, 1.0),
            header_text_color: Color::rgba(0.6, 0.6, 0.65, 1.0),
            row_height: 22.0,
            line_height: 18.0,
            row_padding: 4.0,
            header_height: 26.0,
            stripe_color: Some(Color::rgba(1.0, 1.0, 1.0, 0.02)),
            separator_color: Color::rgba(1.0, 1.0, 1.0, 0.12),
        }
    }

    pub fn column(mut self, name: impl Into<String>, width: f32) -> Self {
        self.columns.push(TableColumn { name: name.into(), width, sort_id: None });
        self
    }

    pub fn column_sortable(mut self, name: impl Into<String>, width: f32, sort_id: SourceId) -> Self {
        self.columns.push(TableColumn { name: name.into(), width, sort_id: Some(sort_id) });
        self
    }

    pub fn row(mut self, cells: Vec<TableCell>) -> Self {
        self.rows.push(cells);
        self
    }

    pub fn header_bg(mut self, color: Color) -> Self { self.header_bg = color; self }
    pub fn header_text_color(mut self, color: Color) -> Self { self.header_text_color = color; self }
    pub fn row_height(mut self, height: f32) -> Self { self.row_height = height; self }
    pub fn header_height(mut self, height: f32) -> Self { self.header_height = height; self }
    pub fn stripe_color(mut self, color: Option<Color>) -> Self { self.stripe_color = color; self }
    pub fn separator_color(mut self, color: Color) -> Self { self.separator_color = color; self }

    pub(crate) fn estimate_size(&self) -> Size {
        let w: f32 = self.columns.iter().map(|c| c.width).sum();
        let rows_h: f32 = self.rows.iter().map(|row| self.row_height_for(row)).sum();
        let h = self.header_height + 1.0 + rows_h;
        Size::new(w, h)
    }

    /// Compute the height for a single row based on the tallest cell.
    fn row_height_for(&self, row: &[TableCell]) -> f32 {
        let max_lines = row.iter()
            .map(|cell| if cell.lines.is_empty() { 1 } else { cell.lines.len() })
            .max()
            .unwrap_or(1);
        if max_lines <= 1 {
            self.row_height // fast path: single-line rows use the fixed height
        } else {
            max_lines as f32 * self.line_height + self.row_padding
        }
    }
}

/// Render a table element into the snapshot.
fn render_table(
    snapshot: &mut LayoutSnapshot,
    table: TableElement,
    x: f32, y: f32, w: f32, _h: f32,
) {
    use crate::primitives::Point;

    let cell_pad = 8.0;

    // Header background
    snapshot.primitives_mut().add_solid_rect(
        Rect::new(x, y, w, table.header_height),
        table.header_bg,
    );

    // Header text + register sortable headers as widgets
    let mut col_x = x;
    let char_width = 8.4_f32;
    for col in &table.columns {
        let tx = col_x + cell_pad;
        let ty = y + 4.0;
        snapshot.primitives_mut().add_text_cached(
            col.name.clone(),
            Point::new(tx, ty),
            table.header_text_color,
            BASE_FONT_SIZE,
            hash_text(&col.name),
        );
        // Register header text for selection
        {
            use crate::layout_snapshot::{SourceLayout, TextLayout};
            let mut text_layout = TextLayout::simple(
                col.name.clone(), table.header_text_color.pack(),
                tx, ty, char_width, table.line_height,
            );
            text_layout.bounds.width = text_layout.bounds.width.max(col.width - cell_pad);
            snapshot.register_source(table.source_id, SourceLayout::text(text_layout));
        }
        if let Some(sort_id) = col.sort_id {
            snapshot.register_widget(sort_id, Rect::new(col_x, y, col.width, table.header_height));
            snapshot.set_cursor_hint(sort_id, CursorIcon::Pointer);
        }
        col_x += col.width;
    }

    // Separator line
    let sep_y = y + table.header_height;
    snapshot.primitives_mut().add_line(
        Point::new(x, sep_y),
        Point::new(x + w, sep_y),
        1.0,
        table.separator_color,
    );

    // Data rows — variable height based on wrapped line count
    let data_y = sep_y + 1.0;
    let mut ry = data_y;
    let char_width = 8.4_f32;
    // Get clip bounds for row-level culling (from parent ScrollColumn)
    let clip_bounds = snapshot.primitives().current_clip_bounds();
    for (row_idx, row) in table.rows.iter().enumerate() {
        let rh = table.row_height_for(row);

        // Cull rows entirely outside the clip region (viewport)
        if let Some(clip) = clip_bounds {
            if ry + rh < clip.y || ry > clip.y + clip.height {
                ry += rh;
                continue;
            }
        }

        // Stripe background for odd rows
        if row_idx % 2 == 1 {
            if let Some(stripe) = table.stripe_color {
                snapshot.primitives_mut().add_solid_rect(
                    Rect::new(x, ry, w, rh),
                    stripe,
                );
            }
        }

        let mut col_x = x;
        for (col_idx, cell) in row.iter().enumerate() {
            if col_idx < table.columns.len() {
                if cell.lines.len() <= 1 {
                    // Single line (fast path)
                    let text = if cell.lines.len() == 1 { &cell.lines[0] } else { &cell.text };
                    let tx = col_x + cell_pad;
                    let ty = ry + 2.0;
                    let text_width = unicode_display_width(text) * char_width;
                    snapshot.primitives_mut().add_text_cached(
                        text.clone(),
                        Point::new(tx, ty),
                        cell.color,
                        BASE_FONT_SIZE,
                        hash_text(text),
                    );
                    // Register clickable cell as widget (text-width only)
                    if let Some(wid) = cell.widget_id {
                        let text_rect = Rect::new(tx, ty, text_width, table.line_height);
                        snapshot.register_widget(wid, text_rect);
                        snapshot.set_cursor_hint(wid, CursorIcon::Pointer);
                    }
                    // Register for text selection (anchors are both clickable and selectable)
                    {
                        use crate::layout_snapshot::{SourceLayout, TextLayout};
                        let mut text_layout = TextLayout::simple(
                            text.clone(), cell.color.pack(),
                            tx, ty, char_width, table.line_height,
                        );
                        text_layout.bounds.width = text_layout.bounds.width.max(table.columns[col_idx].width - cell_pad);
                        snapshot.register_source(table.source_id, SourceLayout::text(text_layout));
                    }
                } else {
                    // Multi-line wrapped cell
                    let mut max_text_width: f32 = 0.0;
                    for (line_idx, line) in cell.lines.iter().enumerate() {
                        let tx = col_x + cell_pad;
                        let ly = ry + 2.0 + line_idx as f32 * table.line_height;
                        let line_width = unicode_display_width(line) * char_width;
                        max_text_width = max_text_width.max(line_width);
                        snapshot.primitives_mut().add_text_cached(
                            line.clone(),
                            Point::new(tx, ly),
                            cell.color,
                            BASE_FONT_SIZE,
                            hash_text(line),
                        );
                        // Register for text selection (anchors are both clickable and selectable)
                        {
                            use crate::layout_snapshot::{SourceLayout, TextLayout};
                            let mut text_layout = TextLayout::simple(
                                line.clone(), cell.color.pack(),
                                tx, ly, char_width, table.line_height,
                            );
                            text_layout.bounds.width = text_layout.bounds.width.max(table.columns[col_idx].width - cell_pad);
                            snapshot.register_source(table.source_id, SourceLayout::text(text_layout));
                        }
                    }
                    // Register clickable cell as widget covering all lines
                    if let Some(wid) = cell.widget_id {
                        let tx = col_x + cell_pad;
                        let ty = ry + 2.0;
                        let text_rect = Rect::new(tx, ty, max_text_width, cell.lines.len() as f32 * table.line_height);
                        snapshot.register_widget(wid, text_rect);
                        snapshot.set_cursor_hint(wid, CursorIcon::Pointer);
                    }
                }
                col_x += table.columns[col_idx].width;
            }
        }

        ry += rh;
    }

}

// =========================================================================
// Virtual Table Element (O(visible) rendering)
// =========================================================================

/// A lightweight cell for virtual tables — just the pre-formatted text, no wrapping yet.
pub struct VirtualCell {
    pub text: String,
    pub color: Color,
    pub widget_id: Option<SourceId>,
}

/// A virtual table that only materializes visible rows during rendering.
///
/// Unlike `TableElement` which requires all rows to be fully built (with wrapping)
/// upfront, `VirtualTableElement` stores lightweight cell data and defers
/// wrapping + layout to render time, processing only the ~30 visible rows.
///
/// This makes the cost O(visible_rows) instead of O(total_rows).
pub struct VirtualTableElement {
    pub source_id: SourceId,
    pub columns: Vec<TableColumn>,
    /// Lightweight rows: just text + color + optional widget_id per cell.
    pub rows: Vec<Vec<VirtualCell>>,
    pub header_bg: Color,
    pub header_text_color: Color,
    pub row_height: f32,
    pub line_height: f32,
    pub row_padding: f32,
    pub header_height: f32,
    pub stripe_color: Option<Color>,
    pub separator_color: Color,
}

impl VirtualTableElement {
    pub fn new(source_id: SourceId) -> Self {
        Self {
            source_id,
            columns: Vec::new(),
            rows: Vec::new(),
            header_bg: Color::rgba(0.15, 0.15, 0.2, 1.0),
            header_text_color: Color::rgba(0.6, 0.6, 0.65, 1.0),
            row_height: 22.0,
            line_height: 18.0,
            row_padding: 4.0,
            header_height: 26.0,
            stripe_color: Some(Color::rgba(1.0, 1.0, 1.0, 0.02)),
            separator_color: Color::rgba(1.0, 1.0, 1.0, 0.12),
        }
    }

    pub fn column(mut self, name: impl Into<String>, width: f32) -> Self {
        self.columns.push(TableColumn { name: name.into(), width, sort_id: None });
        self
    }

    pub fn column_sortable(mut self, name: impl Into<String>, width: f32, sort_id: SourceId) -> Self {
        self.columns.push(TableColumn { name: name.into(), width, sort_id: Some(sort_id) });
        self
    }

    pub fn row(mut self, cells: Vec<VirtualCell>) -> Self {
        self.rows.push(cells);
        self
    }

    pub fn header_bg(mut self, color: Color) -> Self { self.header_bg = color; self }
    pub fn header_text_color(mut self, color: Color) -> Self { self.header_text_color = color; self }
    pub fn row_height(mut self, height: f32) -> Self { self.row_height = height; self }
    pub fn header_height(mut self, height: f32) -> Self { self.header_height = height; self }
    pub fn stripe_color(mut self, color: Option<Color>) -> Self { self.stripe_color = color; self }
    pub fn separator_color(mut self, color: Color) -> Self { self.separator_color = color; self }

    /// O(1) size estimate — assumes all rows are single-line (default_row_height).
    pub(crate) fn estimate_size(&self) -> Size {
        let w: f32 = self.columns.iter().map(|c| c.width).sum();
        let h = self.header_height + 1.0 + self.rows.len() as f32 * self.row_height;
        Size::new(w, h)
    }
}

/// Render a virtual table — only wraps and emits text for visible rows.
fn render_virtual_table(
    snapshot: &mut LayoutSnapshot,
    table: VirtualTableElement,
    x: f32, y: f32, w: f32, _h: f32,
) {
    use crate::primitives::Point;

    let cell_pad = 8.0;
    let char_width = 8.4_f32;

    // Header background
    snapshot.primitives_mut().add_solid_rect(
        Rect::new(x, y, w, table.header_height),
        table.header_bg,
    );

    // Header text + register sortable headers
    let mut col_x = x;
    for col in &table.columns {
        let tx = col_x + cell_pad;
        let ty = y + 4.0;
        snapshot.primitives_mut().add_text_cached(
            col.name.clone(),
            Point::new(tx, ty),
            table.header_text_color,
            BASE_FONT_SIZE,
            hash_text(&col.name),
        );
        {
            use crate::layout_snapshot::{SourceLayout, TextLayout};
            let mut text_layout = TextLayout::simple(
                col.name.clone(), table.header_text_color.pack(),
                tx, ty, char_width, table.line_height,
            );
            text_layout.bounds.width = text_layout.bounds.width.max(col.width - cell_pad);
            snapshot.register_source(table.source_id, SourceLayout::text(text_layout));
        }
        if let Some(sort_id) = col.sort_id {
            snapshot.register_widget(sort_id, Rect::new(col_x, y, col.width, table.header_height));
            snapshot.set_cursor_hint(sort_id, CursorIcon::Pointer);
        }
        col_x += col.width;
    }

    // Separator
    let sep_y = y + table.header_height;
    snapshot.primitives_mut().add_line(
        Point::new(x, sep_y),
        Point::new(x + w, sep_y),
        1.0,
        table.separator_color,
    );

    // Data rows — virtual: use clip bounds to find visible range
    let data_y = sep_y + 1.0;
    let clip_bounds = snapshot.primitives().current_clip_bounds();

    // Compute visible row range using default row height (O(1) per row skip)
    let (first_visible, last_visible) = if let Some(clip) = clip_bounds {
        let clip_top = clip.y;
        let clip_bottom = clip.y + clip.height;
        // Fast index calculation for uniform row heights
        let first = ((clip_top - data_y) / table.row_height).floor().max(0.0) as usize;
        let last = ((clip_bottom - data_y) / table.row_height).ceil().max(0.0) as usize;
        (first, last.min(table.rows.len()))
    } else {
        (0, table.rows.len())
    };

    // Render only visible rows
    for row_idx in first_visible..last_visible {
        let row = &table.rows[row_idx];
        let ry = data_y + row_idx as f32 * table.row_height;

        // Stripe background for odd rows
        if row_idx % 2 == 1 {
            if let Some(stripe) = table.stripe_color {
                snapshot.primitives_mut().add_solid_rect(
                    Rect::new(x, ry, w, table.row_height),
                    stripe,
                );
            }
        }

        let mut col_x = x;
        for (col_idx, cell) in row.iter().enumerate() {
            if col_idx < table.columns.len() {
                let tx = col_x + cell_pad;
                let ty = ry + 2.0;
                let text_width = unicode_display_width(&cell.text) * char_width;
                snapshot.primitives_mut().add_text_cached(
                    cell.text.clone(),
                    Point::new(tx, ty),
                    cell.color,
                    BASE_FONT_SIZE,
                    hash_text(&cell.text),
                );
                if let Some(wid) = cell.widget_id {
                    let text_rect = Rect::new(tx, ty, text_width, table.line_height);
                    snapshot.register_widget(wid, text_rect);
                    snapshot.set_cursor_hint(wid, CursorIcon::Pointer);
                }
                {
                    use crate::layout_snapshot::{SourceLayout, TextLayout};
                    let mut text_layout = TextLayout::simple(
                        cell.text.clone(), cell.color.pack(),
                        tx, ty, char_width, table.line_height,
                    );
                    text_layout.bounds.width = text_layout.bounds.width.max(table.columns[col_idx].width - cell_pad);
                    snapshot.register_source(table.source_id, SourceLayout::text(text_layout));
                }
                col_x += table.columns[col_idx].width;
            }
        }
    }
}

// =========================================================================
// Column
// =========================================================================

/// A vertical layout container (children flow top to bottom).
pub struct Column {
    /// Widget ID for hit-testing and overlay anchoring.
    id: Option<SourceId>,
    /// Child elements.
    children: Vec<LayoutChild>,
    /// Spacing between children.
    spacing: f32,
    /// Padding around all children.
    padding: Padding,
    /// Main axis alignment.
    alignment: Alignment,
    /// Cross axis alignment.
    cross_alignment: CrossAxisAlignment,
    /// Background color (optional).
    background: Option<Color>,
    /// Corner radius for background.
    corner_radius: f32,
    /// Width sizing mode.
    pub(crate) width: Length,
    /// Height sizing mode.
    pub(crate) height: Length,
    /// Border color (optional).
    border_color: Option<Color>,
    /// Border width.
    border_width: f32,
    /// Shadow: (blur_radius, color).
    shadow: Option<(f32, Color)>,
    /// Cursor hint when hovering (requires `id` to take effect).
    cursor_hint: Option<CursorIcon>,
}

impl Default for Column {
    fn default() -> Self {
        Self::new()
    }
}

impl Column {
    /// Create a new column.
    pub fn new() -> Self {
        Self {
            id: None,
            children: Vec::new(),
            spacing: 0.0,
            padding: Padding::default(),
            alignment: Alignment::Start,
            cross_alignment: CrossAxisAlignment::Start,
            background: None,
            corner_radius: 0.0,
            width: Length::Shrink,
            height: Length::Shrink,
            border_color: None,
            border_width: 0.0,
            shadow: None,
            cursor_hint: None,
        }
    }

    /// Set widget ID for hit-testing and overlay anchoring.
    pub fn id(mut self, id: SourceId) -> Self {
        self.id = Some(id);
        self
    }

    /// Set spacing between children.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set padding (uniform on all sides).
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = Padding::all(padding);
        self
    }

    /// Set custom padding.
    pub fn padding_custom(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    /// Set main axis alignment.
    pub fn align(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Set cross axis alignment.
    pub fn cross_align(mut self, alignment: CrossAxisAlignment) -> Self {
        self.cross_alignment = alignment;
        self
    }

    /// Set background color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Set corner radius for background.
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Set width sizing mode.
    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Set height sizing mode.
    pub fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    /// Set border (color + width).
    pub fn border(mut self, color: Color, width: f32) -> Self {
        self.border_color = Some(color);
        self.border_width = width;
        self
    }

    /// Set drop shadow (blur_radius, color).
    pub fn shadow(mut self, blur: f32, color: Color) -> Self {
        self.shadow = Some((blur, color));
        self
    }

    /// Add a text element.
    pub fn text(mut self, element: TextElement) -> Self {
        self.children.push(LayoutChild::Text(element));
        self
    }

    /// Add a terminal element.
    pub fn terminal(mut self, element: TerminalElement) -> Self {
        self.children.push(LayoutChild::Terminal(element));
        self
    }

    /// Add a nested column.
    pub fn column(mut self, column: Column) -> Self {
        self.children.push(LayoutChild::Column(Box::new(column)));
        self
    }

    /// Add a nested row.
    pub fn row(mut self, row: Row) -> Self {
        self.children.push(LayoutChild::Row(Box::new(row)));
        self
    }

    /// Add a scroll column.
    pub fn scroll_column(mut self, scroll: ScrollColumn) -> Self {
        self.children.push(LayoutChild::ScrollColumn(Box::new(scroll)));
        self
    }

    /// Add a flexible spacer.
    pub fn spacer(mut self, flex: f32) -> Self {
        self.children.push(LayoutChild::Spacer { flex });
        self
    }

    /// Add a fixed-size spacer.
    pub fn fixed_spacer(mut self, size: f32) -> Self {
        self.children.push(LayoutChild::FixedSpacer { size });
        self
    }

    /// Add an image element.
    pub fn image(mut self, element: ImageElement) -> Self {
        self.children.push(LayoutChild::Image(element));
        self
    }

    /// Add a button element.
    pub fn button(mut self, element: ButtonElement) -> Self {
        self.children.push(LayoutChild::Button(element));
        self
    }

    /// Add a text input element.
    pub fn text_input(mut self, element: TextInputElement) -> Self {
        self.children.push(LayoutChild::TextInput(element));
        self
    }

    pub fn table(mut self, element: TableElement) -> Self {
        self.children.push(LayoutChild::Table(element));
        self
    }

    pub fn virtual_table(mut self, element: VirtualTableElement) -> Self {
        self.children.push(LayoutChild::VirtualTable(element));
        self
    }

    /// Add any child element using `From<T> for LayoutChild`.
    ///
    /// This is a generic alternative to the type-specific methods above.
    /// The compiler resolves the `Into` conversion at compile time, so this
    /// generates identical code to calling `.text()`, `.button()`, etc. directly.
    #[inline(always)]
    pub fn push(mut self, child: impl Into<LayoutChild>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Compute intrinsic size (content size + padding).
    ///
    /// Short-circuits on Fixed axes — does not recurse into children
    /// for dimensions that are already determined.
    pub fn measure(&self) -> Size {
        let intrinsic_width = match self.width {
            Length::Fixed(px) => px,
            _ => {
                let mut max_child_width: f32 = 0.0;
                for child in &self.children {
                    max_child_width = max_child_width.max(child.measure_cross(true));
                }
                max_child_width + self.padding.horizontal()
            }
        };

        let intrinsic_height = match self.height {
            Length::Fixed(px) => px,
            _ => {
                let mut total_height: f32 = 0.0;
                for child in &self.children {
                    // Skip flex children in measurement (they fill remaining space)
                    if child.flex_factor(true) > 0.0 {
                        continue;
                    }
                    total_height += child.measure_main(true);
                }
                // Spacing between all children (flex children still occupy a slot)
                if self.children.len() > 1 {
                    total_height += self.spacing * (self.children.len() - 1) as f32;
                }
                total_height + self.padding.vertical()
            }
        };

        Size::new(intrinsic_width, intrinsic_height)
    }

    /// Calculate the height of this Column for a given available width.
    /// This is needed because FlowContainer and Row children have width-dependent heights.
    pub fn height_for_width(&self, available_width: f32) -> f32 {
        if let Length::Fixed(px) = self.height {
            return px;
        }

        let content_width = available_width - self.padding.horizontal();
        let mut total_height = 0.0f32;

        for child in &self.children {
            // Skip flex children in measurement (they fill remaining space)
            if child.flex_factor(true) > 0.0 {
                continue;
            }

            let h = match child {
                LayoutChild::Flow(f) => f.height_for_width(content_width),
                LayoutChild::Row(r) => r.height_for_width(content_width),
                LayoutChild::Column(c) => c.height_for_width(content_width),
                _ => child.measure_main(true),
            };
            total_height += h;
        }

        // Spacing between all children (flex children still occupy a slot)
        if self.children.len() > 1 {
            total_height += self.spacing * (self.children.len() - 1) as f32;
        }

        total_height + self.padding.vertical()
    }

    /// Compute layout and flush to snapshot.
    ///
    /// This is where the actual layout math happens - ONCE per frame.
    pub fn layout(self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        // Available space after padding
        let content_x = bounds.x + self.padding.left;
        let content_y = bounds.y + self.padding.top;
        let content_width = bounds.width - self.padding.horizontal();

        // Draw shadow → background → border (correct z-order)
        // These are drawn OUTSIDE the clip rect (they ARE the container chrome).
        if let Some((blur, color)) = self.shadow {
            snapshot.primitives_mut().add_shadow(
                Rect::new(bounds.x + 4.0, bounds.y + 4.0, bounds.width, bounds.height),
                self.corner_radius,
                blur,
                color,
            );
        }
        if let Some(bg) = self.background {
            if self.corner_radius > 0.0 {
                snapshot.primitives_mut().add_rounded_rect(bounds, self.corner_radius, bg);
            } else {
                snapshot.primitives_mut().add_solid_rect(bounds, bg);
            }
        }
        if let Some(border_color) = self.border_color {
            snapshot.primitives_mut().add_border(
                bounds,
                self.corner_radius,
                self.border_width,
                border_color,
            );
        }

        let has_chrome = self.background.is_some() || self.border_color.is_some();

        // =====================================================================
        // Measurement pass: compute child heights and flex factors
        // Also tracks max cross-axis width for overflow detection.
        // =====================================================================
        let mut total_fixed_height = 0.0;
        let mut total_flex = 0.0;
        let mut max_child_cross: f32 = 0.0;
        let mut child_heights: Vec<f32> = Vec::with_capacity(self.children.len());

        for child in &self.children {
            max_child_cross = max_child_cross.max(child.measure_cross(true));
            match child {
                LayoutChild::Text(t) => {
                    let h = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT).height;
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::Terminal(t) => {
                    let h = t.size().height;
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::Column(c) => {
                    match c.height {
                        Length::Fixed(px) => {
                            child_heights.push(px);
                            total_fixed_height += px;
                        }
                        Length::Fill | Length::FillPortion(_) => {
                            child_heights.push(0.0);
                            total_flex += c.height.flex();
                        }
                        Length::Shrink => {
                            let h = c.measure().height;
                            child_heights.push(h);
                            total_fixed_height += h;
                        }
                    }
                }
                LayoutChild::Row(r) => {
                    match r.height {
                        Length::Fixed(px) => {
                            child_heights.push(px);
                            total_fixed_height += px;
                        }
                        Length::Fill | Length::FillPortion(_) => {
                            child_heights.push(0.0);
                            total_flex += r.height.flex();
                        }
                        Length::Shrink => {
                            // Use height_for_width to account for FlowContainer wrapping
                            let h = r.height_for_width(content_width);
                            child_heights.push(h);
                            total_fixed_height += h;
                        }
                    }
                }
                LayoutChild::ScrollColumn(s) => {
                    match s.height {
                        Length::Fixed(px) => {
                            child_heights.push(px);
                            total_fixed_height += px;
                        }
                        Length::Fill | Length::FillPortion(_) => {
                            child_heights.push(0.0);
                            total_flex += s.height.flex();
                        }
                        Length::Shrink => {
                            let h = s.measure().height;
                            child_heights.push(h);
                            total_fixed_height += h;
                        }
                    }
                }
                LayoutChild::Image(img) => {
                    let h = img.height;
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::Button(btn) => {
                    let h = btn.estimate_size().height;
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::TextInput(input) => {
                    let h = LINE_HEIGHT + input.padding.vertical();
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::Table(table) => {
                    let h = table.estimate_size().height;
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::VirtualTable(table) => {
                    let h = table.estimate_size().height;
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::Flow(flow) => {
                    // FlowContainer height depends on available width
                    let h = flow.height_for_width(content_width);
                    child_heights.push(h);
                    total_fixed_height += h;
                }
                LayoutChild::Spacer { flex } => {
                    child_heights.push(0.0);
                    total_flex += flex;
                }
                LayoutChild::FixedSpacer { size } => {
                    child_heights.push(*size);
                    total_fixed_height += size;
                }
            }
        }

        // Add spacing to fixed height
        if !self.children.is_empty() {
            total_fixed_height += self.spacing * (self.children.len() - 1) as f32;
        }

        // Overflow detection (replaces previous self.measure() call)
        let content_w = max_child_cross + self.padding.horizontal();
        let content_h = total_fixed_height + self.padding.vertical();
        let content_overflows = bounds.width < content_w || bounds.height < content_h;
        let clips = has_chrome || content_overflows;
        if clips {
            snapshot.primitives_mut().push_clip(bounds);
        }

        let available_flex = (bounds.height - self.padding.vertical() - total_fixed_height).max(0.0);

        // Compute total consumed height (flex children consume available_flex)
        let total_flex_consumed = if total_flex > 0.0 { available_flex } else { 0.0 };
        let used_height = total_fixed_height + total_flex_consumed;
        let free_space = (bounds.height - self.padding.vertical() - used_height).max(0.0);

        // =====================================================================
        // Main axis alignment: compute starting y and extra per-gap spacing
        // =====================================================================
        let n = self.children.len();
        let (mut y, alignment_gap) = match self.alignment {
            Alignment::Start => (content_y, 0.0),
            Alignment::End => (content_y + free_space, 0.0),
            Alignment::Center => (content_y + free_space / 2.0, 0.0),
            Alignment::SpaceBetween => {
                if n > 1 {
                    (content_y, free_space / (n - 1) as f32)
                } else {
                    (content_y, 0.0)
                }
            }
            Alignment::SpaceAround => {
                if n > 0 {
                    let space = free_space / n as f32;
                    (content_y + space / 2.0, space)
                } else {
                    (content_y, 0.0)
                }
            }
        };

        // =====================================================================
        // Position pass: place children and flush to snapshot
        // =====================================================================
        for (i, child) in self.children.into_iter().enumerate() {
            let mut height = child_heights[i];

            // Helper: resolve cross-axis x position for a child of given width
            let cross_x = |child_width: f32| -> f32 {
                match self.cross_alignment {
                    CrossAxisAlignment::Start | CrossAxisAlignment::Stretch => content_x,
                    CrossAxisAlignment::End => content_x + content_width - child_width,
                    CrossAxisAlignment::Center => content_x + (content_width - child_width) / 2.0,
                }
            };

            match child {
                LayoutChild::Text(t) => {
                    let fs = t.font_size();
                    let size = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT);
                    let x = cross_x(size.width);

                    use crate::layout_snapshot::{SourceLayout, TextLayout};
                    if let Some(source_id) = t.source_id {
                        let scale = fs / BASE_FONT_SIZE;
                        let mut text_layout = TextLayout::simple(
                            t.text.clone(),
                            t.color.pack(),
                            x, y,
                            CHAR_WIDTH * scale, LINE_HEIGHT * scale,
                        );
                        // Expand hit-box to full content width — in Column, text
                        // owns the entire line so this is safe (no sibling conflicts).
                        text_layout.bounds.width = text_layout.bounds.width.max(content_width);
                        snapshot.register_source(source_id, SourceLayout::text(text_layout));
                    }

                    // Register widget if this text is clickable
                    if let Some(widget_id) = t.widget_id {
                        let text_rect = Rect::new(x, y, size.width, size.height);
                        snapshot.register_widget(widget_id, text_rect);
                        if let Some(cursor) = t.cursor_hint {
                            snapshot.set_cursor_hint(widget_id, cursor);
                        }
                    }

                    snapshot.primitives_mut().add_text_cached_styled(
                        t.text,
                        crate::primitives::Point::new(x, y),
                        t.color,
                        fs,
                        t.cache_key,
                        t.bold,
                        t.italic,
                    );

                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::Terminal(t) => {
                    let size = t.size();
                    let x = cross_x(size.width);

                    use crate::layout_snapshot::{GridLayout, GridRow, SourceLayout};
                    let rows_content: Vec<GridRow> = t.row_content.into_iter()
                        .map(|runs| GridRow { runs })
                        .collect();
                    let mut grid_layout = GridLayout::with_rows(
                        Rect::new(x, y, size.width.max(content_width), size.height),
                        t.cell_width, t.cell_height,
                        t.cols, t.rows,
                        rows_content,
                    );
                    grid_layout.clip_rect = snapshot.current_clip();
                    snapshot.register_source(t.source_id, SourceLayout::grid(grid_layout));

                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::Image(img) => {
                    let x = cross_x(img.width);
                    let img_rect = Rect::new(x, y, img.width, img.height);
                    snapshot.primitives_mut().add_image(
                        img_rect,
                        img.handle,
                        img.corner_radius,
                        img.tint,
                    );
                    if let Some(id) = img.widget_id {
                        snapshot.register_widget(id, img_rect);
                        if let Some(cursor) = img.cursor_hint {
                            snapshot.set_cursor_hint(id, cursor);
                        }
                    }
                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::Button(btn) => {
                    let size = btn.estimate_size();
                    let bx = cross_x(size.width);
                    let btn_rect = Rect::new(bx, y, size.width, size.height);
                    snapshot.primitives_mut().add_rounded_rect(btn_rect, btn.corner_radius, btn.background);
                    snapshot.primitives_mut().add_text_cached(
                        btn.label,
                        crate::primitives::Point::new(bx + btn.padding.left, y + btn.padding.top),
                        btn.text_color,
                        BASE_FONT_SIZE,
                        btn.cache_key,
                    );
                    snapshot.register_widget(btn.id, btn_rect);
                    snapshot.set_cursor_hint(btn.id, CursorIcon::Pointer);
                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::TextInput(input) => {
                    let h = if input.multiline {
                        input.estimate_size().height
                    } else {
                        LINE_HEIGHT + input.padding.vertical()
                    };
                    let w = match input.width {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_width,
                        Length::Shrink => input.estimate_size().width.min(content_width),
                    };
                    let ix = cross_x(w);
                    if input.multiline {
                        render_text_input_multiline(snapshot, input, ix, y, w, h);
                    } else {
                        render_text_input(snapshot, input, ix, y, w, h);
                    }
                    y += h + self.spacing + alignment_gap;
                }
                LayoutChild::Table(table) => {
                    let size = table.estimate_size();
                    let w = size.width.min(content_width);
                    let tx = cross_x(w);
                    render_table(snapshot, table, tx, y, w, size.height);
                    y += size.height + self.spacing + alignment_gap;
                }
                LayoutChild::VirtualTable(table) => {
                    let size = table.estimate_size();
                    let w = size.width.min(content_width);
                    let tx = cross_x(w);
                    render_virtual_table(snapshot, table, tx, y, w, size.height);
                    y += size.height + self.spacing + alignment_gap;
                }
                LayoutChild::Flow(flow) => {
                    let w = match flow.width {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) | Length::Shrink => content_width,
                    };
                    let h = flow.height_for_width(w);
                    let fx = cross_x(w);
                    flow.layout(snapshot, fx, y, w);
                    y += h + self.spacing + alignment_gap;
                }
                LayoutChild::Column(nested) => {
                    // Resolve flex height for Fill children
                    if nested.height.is_flex() && total_flex > 0.0 {
                        height = (nested.height.flex() / total_flex) * available_flex;
                    }
                    // Resolve width
                    let w = match nested.width {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_width,
                        Length::Shrink => nested.measure().width.min(content_width),
                    };
                    let x = cross_x(w);
                    let nested_bounds = Rect::new(x, y, w, height);
                    nested.layout(snapshot, nested_bounds);
                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::Row(nested) => {
                    // Resolve flex height for Fill children
                    if nested.height.is_flex() && total_flex > 0.0 {
                        height = (nested.height.flex() / total_flex) * available_flex;
                    }
                    // Give Rows the full content width so their children's
                    // hit-boxes can expand to fill the line (same as Column text).
                    let w = match nested.width {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) | Length::Shrink => content_width,
                    };
                    let x = cross_x(w);
                    let nested_bounds = Rect::new(x, y, w, height);
                    nested.layout(snapshot, nested_bounds);
                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::ScrollColumn(nested) => {
                    // Resolve flex height for Fill children
                    if nested.height.is_flex() && total_flex > 0.0 {
                        height = (nested.height.flex() / total_flex) * available_flex;
                    }
                    // Resolve width
                    let w = match nested.width {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_width,
                        Length::Shrink => nested.measure().width.min(content_width),
                    };
                    let x = cross_x(w);
                    let nested_bounds = Rect::new(x, y, w, height);
                    nested.layout(snapshot, nested_bounds);
                    y += height + self.spacing + alignment_gap;
                }
                LayoutChild::Spacer { flex } => {
                    if total_flex > 0.0 {
                        let space = (flex / total_flex) * available_flex;
                        y += space;
                    }
                    y += alignment_gap;
                }
                LayoutChild::FixedSpacer { size } => {
                    y += size + alignment_gap;
                }
            }
        }

        // Register widget ID for hit-testing and overlay anchoring
        if let Some(id) = self.id {
            snapshot.register_widget(id, bounds);
            if let Some(cursor) = self.cursor_hint {
                snapshot.set_cursor_hint(id, cursor);
            }
        }

        if clips {
            snapshot.primitives_mut().pop_clip();
        }
    }
}

// =========================================================================
// Row
// =========================================================================

/// A horizontal layout container (children flow left to right).
pub struct Row {
    /// Widget ID for hit-testing and overlay anchoring.
    id: Option<SourceId>,
    /// Child elements.
    children: Vec<LayoutChild>,
    /// Spacing between children.
    spacing: f32,
    /// Padding around all children.
    padding: Padding,
    /// Main axis alignment.
    alignment: Alignment,
    /// Cross axis alignment.
    cross_alignment: CrossAxisAlignment,
    /// Background color (optional).
    background: Option<Color>,
    /// Corner radius for background.
    corner_radius: f32,
    /// Width sizing mode.
    pub(crate) width: Length,
    /// Height sizing mode.
    pub(crate) height: Length,
    /// Border color (optional).
    border_color: Option<Color>,
    /// Border width.
    border_width: f32,
    /// Shadow: (blur_radius, color).
    shadow: Option<(f32, Color)>,
    /// Cursor hint when hovering (requires `id` to take effect).
    cursor_hint: Option<CursorIcon>,
}

impl Default for Row {
    fn default() -> Self {
        Self::new()
    }
}

impl Row {
    /// Create a new row.
    pub fn new() -> Self {
        Self {
            id: None,
            children: Vec::new(),
            spacing: 0.0,
            padding: Padding::default(),
            alignment: Alignment::Start,
            cross_alignment: CrossAxisAlignment::Start,
            background: None,
            corner_radius: 0.0,
            width: Length::Shrink,
            height: Length::Shrink,
            border_color: None,
            border_width: 0.0,
            shadow: None,
            cursor_hint: None,
        }
    }

    /// Set widget ID for hit-testing and overlay anchoring.
    pub fn id(mut self, id: SourceId) -> Self {
        self.id = Some(id);
        self
    }

    /// Set cursor hint for hover feedback (requires `id` to take effect).
    pub fn cursor_hint(mut self, cursor: CursorIcon) -> Self {
        self.cursor_hint = Some(cursor);
        self
    }

    /// Set spacing between children.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set padding (uniform on all sides).
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = Padding::all(padding);
        self
    }

    /// Set custom padding.
    pub fn padding_custom(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    /// Set main axis alignment.
    pub fn align(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Set cross axis alignment.
    pub fn cross_align(mut self, alignment: CrossAxisAlignment) -> Self {
        self.cross_alignment = alignment;
        self
    }

    /// Set background color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Set corner radius for background.
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Set width sizing mode.
    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Set height sizing mode.
    pub fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    /// Set border (color + width).
    pub fn border(mut self, color: Color, width: f32) -> Self {
        self.border_color = Some(color);
        self.border_width = width;
        self
    }

    /// Set drop shadow (blur_radius, color).
    pub fn shadow(mut self, blur: f32, color: Color) -> Self {
        self.shadow = Some((blur, color));
        self
    }

    /// Add a text element.
    pub fn text(mut self, element: TextElement) -> Self {
        self.children.push(LayoutChild::Text(element));
        self
    }

    /// Add a terminal element.
    pub fn terminal(mut self, element: TerminalElement) -> Self {
        self.children.push(LayoutChild::Terminal(element));
        self
    }

    /// Add a nested column.
    pub fn column(mut self, column: Column) -> Self {
        self.children.push(LayoutChild::Column(Box::new(column)));
        self
    }

    /// Add a nested row.
    pub fn row(mut self, row: Row) -> Self {
        self.children.push(LayoutChild::Row(Box::new(row)));
        self
    }

    /// Add a scroll column.
    pub fn scroll_column(mut self, scroll: ScrollColumn) -> Self {
        self.children.push(LayoutChild::ScrollColumn(Box::new(scroll)));
        self
    }

    /// Add a flexible spacer.
    pub fn spacer(mut self, flex: f32) -> Self {
        self.children.push(LayoutChild::Spacer { flex });
        self
    }

    /// Add a fixed-size spacer.
    pub fn fixed_spacer(mut self, size: f32) -> Self {
        self.children.push(LayoutChild::FixedSpacer { size });
        self
    }

    /// Add an image element.
    pub fn image(mut self, element: ImageElement) -> Self {
        self.children.push(LayoutChild::Image(element));
        self
    }

    /// Add a button element.
    pub fn button(mut self, element: ButtonElement) -> Self {
        self.children.push(LayoutChild::Button(element));
        self
    }

    /// Add a text input element.
    pub fn text_input(mut self, element: TextInputElement) -> Self {
        self.children.push(LayoutChild::TextInput(element));
        self
    }

    /// Add a table element.
    pub fn table(mut self, element: TableElement) -> Self {
        self.children.push(LayoutChild::Table(element));
        self
    }

    pub fn virtual_table(mut self, element: VirtualTableElement) -> Self {
        self.children.push(LayoutChild::VirtualTable(element));
        self
    }

    /// Add any child element using `From<T> for LayoutChild`.
    #[inline(always)]
    pub fn push(mut self, child: impl Into<LayoutChild>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Compute intrinsic size (content size + padding).
    ///
    /// Short-circuits on Fixed axes.
    pub fn measure(&self) -> Size {
        let intrinsic_width = match self.width {
            Length::Fixed(px) => px,
            _ => {
                let mut total_width: f32 = 0.0;
                for child in &self.children {
                    if child.flex_factor(false) > 0.0 {
                        continue;
                    }
                    total_width += child.measure_main(false);
                }
                // Spacing between all children (flex children still occupy a slot)
                if self.children.len() > 1 {
                    total_width += self.spacing * (self.children.len() - 1) as f32;
                }
                total_width + self.padding.horizontal()
            }
        };

        let intrinsic_height = match self.height {
            Length::Fixed(px) => px,
            _ => {
                let mut max_child_height: f32 = 0.0;
                for child in &self.children {
                    max_child_height = max_child_height.max(child.measure_cross(false));
                }
                max_child_height + self.padding.vertical()
            }
        };

        Size::new(intrinsic_width, intrinsic_height)
    }

    /// Calculate the height of this Row for a given available width.
    /// This is needed because FlowContainer children have width-dependent heights.
    pub fn height_for_width(&self, available_width: f32) -> f32 {
        if let Length::Fixed(px) = self.height {
            return px;
        }

        // Calculate fixed widths and flex factor (mirrors measurement pass logic)
        let mut total_fixed_width = 0.0f32;
        let mut total_flex = 0.0f32;

        for child in &self.children {
            match child {
                LayoutChild::Flow(flow) => {
                    match flow.width {
                        Length::Fill | Length::FillPortion(_) => {
                            total_flex += flow.width.flex();
                        }
                        Length::Fixed(px) => total_fixed_width += px,
                        Length::Shrink => total_fixed_width += flow.measure().width,
                    }
                }
                LayoutChild::Column(c) => {
                    match c.width {
                        Length::Fill | Length::FillPortion(_) => total_flex += c.width.flex(),
                        Length::Fixed(px) => total_fixed_width += px,
                        Length::Shrink => total_fixed_width += c.measure().width,
                    }
                }
                LayoutChild::Row(r) => {
                    match r.width {
                        Length::Fill | Length::FillPortion(_) => total_flex += r.width.flex(),
                        Length::Fixed(px) => total_fixed_width += px,
                        Length::Shrink => total_fixed_width += r.measure().width,
                    }
                }
                LayoutChild::Spacer { flex } => total_flex += flex,
                LayoutChild::FixedSpacer { size } => total_fixed_width += size,
                _ => total_fixed_width += child.measure_main(false),
            }
        }

        // Add spacing
        if self.children.len() > 1 {
            total_fixed_width += self.spacing * (self.children.len() - 1) as f32;
        }

        let content_width = available_width - self.padding.horizontal();
        let available_flex = (content_width - total_fixed_width).max(0.0);

        // Calculate max child height, using height_for_width for width-dependent containers
        let mut max_height = 0.0f32;
        for child in &self.children {
            let h = match child {
                LayoutChild::Flow(flow) => {
                    let flow_width = if flow.width.is_flex() && total_flex > 0.0 {
                        (flow.width.flex() / total_flex) * available_flex
                    } else {
                        match flow.width {
                            Length::Fixed(px) => px,
                            Length::Shrink => flow.measure().width,
                            _ => available_flex,
                        }
                    };
                    flow.height_for_width(flow_width)
                }
                LayoutChild::Column(col) => {
                    let col_width = if col.width.is_flex() && total_flex > 0.0 {
                        (col.width.flex() / total_flex) * available_flex
                    } else {
                        match col.width {
                            Length::Fixed(px) => px,
                            Length::Shrink => col.measure().width,
                            _ => content_width, // Fill takes remaining width
                        }
                    };
                    col.height_for_width(col_width)
                }
                LayoutChild::Row(row) => {
                    let row_width = if row.width.is_flex() && total_flex > 0.0 {
                        (row.width.flex() / total_flex) * available_flex
                    } else {
                        match row.width {
                            Length::Fixed(px) => px,
                            Length::Shrink => row.measure().width,
                            _ => content_width,
                        }
                    };
                    row.height_for_width(row_width)
                }
                _ => child.measure_cross(false),
            };
            max_height = max_height.max(h);
        }

        max_height + self.padding.vertical()
    }

    /// Compute layout and flush to snapshot.
    pub fn layout(self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        // Available space after padding
        let content_x = bounds.x + self.padding.left;
        let content_y = bounds.y + self.padding.top;
        let content_height = bounds.height - self.padding.vertical();

        // Draw shadow → background → border (outside clip)
        if let Some((blur, color)) = self.shadow {
            snapshot.primitives_mut().add_shadow(
                Rect::new(bounds.x + 4.0, bounds.y + 4.0, bounds.width, bounds.height),
                self.corner_radius,
                blur,
                color,
            );
        }
        if let Some(bg) = self.background {
            if self.corner_radius > 0.0 {
                snapshot.primitives_mut().add_rounded_rect(bounds, self.corner_radius, bg);
            } else {
                snapshot.primitives_mut().add_solid_rect(bounds, bg);
            }
        }
        if let Some(border_color) = self.border_color {
            snapshot.primitives_mut().add_border(
                bounds,
                self.corner_radius,
                self.border_width,
                border_color,
            );
        }

        let has_chrome = self.background.is_some() || self.border_color.is_some();

        // =====================================================================
        // Measurement pass: compute child widths and flex factors.
        // Also tracks max cross-axis height for overflow detection.
        // =====================================================================
        let mut total_fixed_width = 0.0;
        let mut total_flex = 0.0;
        let mut max_child_cross: f32 = 0.0;
        let mut child_widths: Vec<f32> = Vec::with_capacity(self.children.len());

        for child in &self.children {
            max_child_cross = max_child_cross.max(child.measure_cross(false));
            match child {
                LayoutChild::Text(t) => {
                    let w = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT).width;
                    child_widths.push(w);
                    total_fixed_width += w;
                }
                LayoutChild::Terminal(t) => {
                    let w = t.size().width;
                    child_widths.push(w);
                    total_fixed_width += w;
                }
                LayoutChild::Column(c) => {
                    match c.width {
                        Length::Fixed(px) => {
                            child_widths.push(px);
                            total_fixed_width += px;
                        }
                        Length::Fill | Length::FillPortion(_) => {
                            child_widths.push(0.0);
                            total_flex += c.width.flex();
                        }
                        Length::Shrink => {
                            let w = c.measure().width;
                            child_widths.push(w);
                            total_fixed_width += w;
                        }
                    }
                }
                LayoutChild::Row(r) => {
                    match r.width {
                        Length::Fixed(px) => {
                            child_widths.push(px);
                            total_fixed_width += px;
                        }
                        Length::Fill | Length::FillPortion(_) => {
                            child_widths.push(0.0);
                            total_flex += r.width.flex();
                        }
                        Length::Shrink => {
                            let w = r.measure().width;
                            child_widths.push(w);
                            total_fixed_width += w;
                        }
                    }
                }
                LayoutChild::ScrollColumn(s) => {
                    match s.width {
                        Length::Fixed(px) => {
                            child_widths.push(px);
                            total_fixed_width += px;
                        }
                        Length::Fill | Length::FillPortion(_) => {
                            child_widths.push(0.0);
                            total_flex += s.width.flex();
                        }
                        Length::Shrink => {
                            let w = s.measure().width;
                            child_widths.push(w);
                            total_fixed_width += w;
                        }
                    }
                }
                LayoutChild::Image(img) => {
                    let w = img.width;
                    child_widths.push(w);
                    total_fixed_width += w;
                }
                LayoutChild::Button(btn) => {
                    let w = btn.estimate_size().width;
                    child_widths.push(w);
                    total_fixed_width += w;
                }
                LayoutChild::TextInput(input) => {
                    match input.width {
                        Length::Fill | Length::FillPortion(_) => {
                            child_widths.push(0.0);
                            total_flex += input.width.flex();
                        }
                        Length::Fixed(px) => {
                            child_widths.push(px);
                            total_fixed_width += px;
                        }
                        Length::Shrink => {
                            let w = input.estimate_size().width;
                            child_widths.push(w);
                            total_fixed_width += w;
                        }
                    }
                }
                LayoutChild::Table(table) => {
                    let w = table.estimate_size().width;
                    child_widths.push(w);
                    total_fixed_width += w;
                }
                LayoutChild::VirtualTable(table) => {
                    let w = table.estimate_size().width;
                    child_widths.push(w);
                    total_fixed_width += w;
                }
                LayoutChild::Flow(flow) => {
                    match flow.width {
                        Length::Fill | Length::FillPortion(_) => {
                            child_widths.push(0.0);
                            total_flex += flow.width.flex();
                        }
                        Length::Fixed(px) => {
                            child_widths.push(px);
                            total_fixed_width += px;
                        }
                        Length::Shrink => {
                            let w = flow.measure().width;
                            child_widths.push(w);
                            total_fixed_width += w;
                        }
                    }
                }
                LayoutChild::Spacer { flex } => {
                    child_widths.push(0.0);
                    total_flex += flex;
                }
                LayoutChild::FixedSpacer { size } => {
                    child_widths.push(*size);
                    total_fixed_width += size;
                }
            }
        }

        // Add spacing to fixed width
        if !self.children.is_empty() {
            total_fixed_width += self.spacing * (self.children.len() - 1) as f32;
        }

        let available_flex = (bounds.width - self.padding.horizontal() - total_fixed_width).max(0.0);

        // Recalculate max_child_cross for width-dependent children now that we know their widths.
        // FlowContainer/Column/Row heights can depend on available width for wrapping.
        for (i, child) in self.children.iter().enumerate() {
            let child_height = match child {
                LayoutChild::Flow(flow) => {
                    let flow_width = if flow.width.is_flex() && total_flex > 0.0 {
                        (flow.width.flex() / total_flex) * available_flex
                    } else {
                        child_widths[i]
                    };
                    Some(flow.height_for_width(flow_width))
                }
                LayoutChild::Column(col) => {
                    let col_width = if col.width.is_flex() && total_flex > 0.0 {
                        (col.width.flex() / total_flex) * available_flex
                    } else {
                        child_widths[i]
                    };
                    Some(col.height_for_width(col_width))
                }
                LayoutChild::Row(row) => {
                    let row_width = if row.width.is_flex() && total_flex > 0.0 {
                        (row.width.flex() / total_flex) * available_flex
                    } else {
                        child_widths[i]
                    };
                    Some(row.height_for_width(row_width))
                }
                _ => None,
            };
            if let Some(h) = child_height {
                max_child_cross = max_child_cross.max(h);
            }
        }

        // Overflow detection (replaces previous self.measure() call)
        let content_w = total_fixed_width + self.padding.horizontal();
        let content_h = max_child_cross + self.padding.vertical();
        let content_overflows = bounds.width < content_w || bounds.height < content_h;
        let clips = has_chrome || content_overflows;
        if clips {
            snapshot.primitives_mut().push_clip(bounds);
        }

        // Compute total consumed width (flex children consume available_flex)
        let total_flex_consumed = if total_flex > 0.0 { available_flex } else { 0.0 };
        let used_width = total_fixed_width + total_flex_consumed;
        let free_space = (bounds.width - self.padding.horizontal() - used_width).max(0.0);

        // =====================================================================
        // Main axis alignment: compute starting x and extra per-gap spacing
        // =====================================================================
        let n = self.children.len();
        let (mut x, alignment_gap) = match self.alignment {
            Alignment::Start => (content_x, 0.0),
            Alignment::End => (content_x + free_space, 0.0),
            Alignment::Center => (content_x + free_space / 2.0, 0.0),
            Alignment::SpaceBetween => {
                if n > 1 {
                    (content_x, free_space / (n - 1) as f32)
                } else {
                    (content_x, 0.0)
                }
            }
            Alignment::SpaceAround => {
                if n > 0 {
                    let space = free_space / n as f32;
                    (content_x + space / 2.0, space)
                } else {
                    (content_x, 0.0)
                }
            }
        };

        // =====================================================================
        // Position pass: place children and flush to snapshot
        // =====================================================================
        for (i, child) in self.children.into_iter().enumerate() {
            let mut width = child_widths[i];

            // Helper: resolve cross-axis y position for a child of given height
            let cross_y = |child_height: f32| -> f32 {
                match self.cross_alignment {
                    CrossAxisAlignment::Start | CrossAxisAlignment::Stretch => content_y,
                    CrossAxisAlignment::End => content_y + content_height - child_height,
                    CrossAxisAlignment::Center => {
                        content_y + (content_height - child_height) / 2.0
                    }
                }
            };

            // Right edge of Row content area, for expanding hit-boxes.
            let content_right = content_x + bounds.width - self.padding.horizontal();

            match child {
                LayoutChild::Text(t) => {
                    let fs = t.font_size();
                    let size = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT);
                    let y = cross_y(size.height);

                    use crate::layout_snapshot::{SourceLayout, TextLayout};
                    if let Some(source_id) = t.source_id {
                        let scale = fs / BASE_FONT_SIZE;
                        let mut text_layout = TextLayout::simple(
                            t.text.clone(),
                            t.color.pack(),
                            x, y,
                            CHAR_WIDTH * scale, LINE_HEIGHT * scale,
                        );
                        // Expand hit-box to fill remaining Row width so empty
                        // space to the right of text is clickable.
                        text_layout.bounds.width = text_layout.bounds.width.max(content_right - x);
                        snapshot.register_source(source_id, SourceLayout::text(text_layout));
                    }

                    // Register widget if this text is clickable
                    if let Some(widget_id) = t.widget_id {
                        let text_rect = Rect::new(x, y, size.width, size.height);
                        snapshot.register_widget(widget_id, text_rect);
                        if let Some(cursor) = t.cursor_hint {
                            snapshot.set_cursor_hint(widget_id, cursor);
                        }
                    }

                    snapshot.primitives_mut().add_text_cached_styled(
                        t.text,
                        crate::primitives::Point::new(x, y),
                        t.color,
                        fs,
                        t.cache_key,
                        t.bold,
                        t.italic,
                    );

                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Terminal(t) => {
                    let size = t.size();
                    let y = cross_y(size.height);

                    use crate::layout_snapshot::{GridLayout, GridRow, SourceLayout};
                    let rows_content: Vec<GridRow> = t.row_content.into_iter()
                        .map(|runs| GridRow { runs })
                        .collect();
                    let mut grid_layout = GridLayout::with_rows(
                        Rect::new(x, y, size.width.max(content_right - x), size.height),
                        t.cell_width, t.cell_height,
                        t.cols, t.rows,
                        rows_content,
                    );
                    grid_layout.clip_rect = snapshot.current_clip();
                    snapshot.register_source(t.source_id, SourceLayout::grid(grid_layout));

                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Image(img) => {
                    let y = cross_y(img.height);
                    let img_rect = Rect::new(x, y, img.width, img.height);
                    snapshot.primitives_mut().add_image(
                        img_rect,
                        img.handle,
                        img.corner_radius,
                        img.tint,
                    );
                    if let Some(id) = img.widget_id {
                        snapshot.register_widget(id, img_rect);
                        if let Some(cursor) = img.cursor_hint {
                            snapshot.set_cursor_hint(id, cursor);
                        }
                    }
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Button(btn) => {
                    let size = btn.estimate_size();
                    let by = cross_y(size.height);
                    let btn_rect = Rect::new(x, by, size.width, size.height);
                    snapshot.primitives_mut().add_rounded_rect(btn_rect, btn.corner_radius, btn.background);
                    snapshot.primitives_mut().add_text_cached(
                        btn.label,
                        crate::primitives::Point::new(x + btn.padding.left, by + btn.padding.top),
                        btn.text_color,
                        BASE_FONT_SIZE,
                        btn.cache_key,
                    );
                    snapshot.register_widget(btn.id, btn_rect);
                    snapshot.set_cursor_hint(btn.id, CursorIcon::Pointer);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::TextInput(input) => {
                    let w = if input.width.is_flex() && total_flex > 0.0 {
                        (input.width.flex() / total_flex) * available_flex
                    } else {
                        width
                    };
                    let h = if input.multiline {
                        input.estimate_size().height
                    } else {
                        LINE_HEIGHT + input.padding.vertical()
                    };
                    let iy = cross_y(h);
                    if input.multiline {
                        render_text_input_multiline(snapshot, input, x, iy, w, h);
                    } else {
                        render_text_input(snapshot, input, x, iy, w, h);
                    }
                    x += w + self.spacing + alignment_gap;
                }
                LayoutChild::Table(table) => {
                    let size = table.estimate_size();
                    let ty = cross_y(size.height);
                    render_table(snapshot, table, x, ty, size.width, size.height);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::VirtualTable(table) => {
                    let size = table.estimate_size();
                    let ty = cross_y(size.height);
                    render_virtual_table(snapshot, table, x, ty, size.width, size.height);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Flow(flow) => {
                    // Resolve flex width for Fill children
                    if flow.width.is_flex() && total_flex > 0.0 {
                        width = (flow.width.flex() / total_flex) * available_flex;
                    }
                    let h = flow.height_for_width(width);
                    let fy = cross_y(h);
                    flow.layout(snapshot, x, fy, width);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Column(nested) => {
                    // Resolve flex width for Fill children
                    if nested.width.is_flex() && total_flex > 0.0 {
                        width = (nested.width.flex() / total_flex) * available_flex;
                    }
                    // Resolve height
                    let h = match nested.height {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_height,
                        Length::Shrink => nested.measure().height.min(content_height),
                    };
                    let y = cross_y(h);
                    let nested_bounds = Rect::new(x, y, width, h);
                    nested.layout(snapshot, nested_bounds);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Row(nested) => {
                    // Resolve flex width for Fill children
                    if nested.width.is_flex() && total_flex > 0.0 {
                        width = (nested.width.flex() / total_flex) * available_flex;
                    }
                    // Resolve height
                    let h = match nested.height {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_height,
                        Length::Shrink => nested.measure().height.min(content_height),
                    };
                    let y = cross_y(h);
                    let nested_bounds = Rect::new(x, y, width, h);
                    nested.layout(snapshot, nested_bounds);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::ScrollColumn(nested) => {
                    // Resolve flex width for Fill children
                    if nested.width.is_flex() && total_flex > 0.0 {
                        width = (nested.width.flex() / total_flex) * available_flex;
                    }
                    // Resolve height
                    let h = match nested.height {
                        Length::Fixed(px) => px,
                        Length::Fill | Length::FillPortion(_) => content_height,
                        Length::Shrink => nested.measure().height.min(content_height),
                    };
                    let y = cross_y(h);
                    let nested_bounds = Rect::new(x, y, width, h);
                    nested.layout(snapshot, nested_bounds);
                    x += width + self.spacing + alignment_gap;
                }
                LayoutChild::Spacer { flex } => {
                    if total_flex > 0.0 {
                        let space = (flex / total_flex) * available_flex;
                        x += space;
                    }
                    x += alignment_gap;
                }
                LayoutChild::FixedSpacer { size } => {
                    x += size + alignment_gap;
                }
            }
        }

        // Register widget ID for hit-testing and overlay anchoring
        if let Some(id) = self.id {
            snapshot.register_widget(id, bounds);
            if let Some(cursor) = self.cursor_hint {
                snapshot.set_cursor_hint(id, cursor);
            }
        }

        if clips {
            snapshot.primitives_mut().pop_clip();
        }
    }
}

// =========================================================================
// ScrollColumn
// =========================================================================

/// A virtualized vertical scroll container.
///
/// Scroll state lives in app state. The container receives the current scroll
/// offset as a parameter. Wheel events flow through `on_mouse` → message →
/// `update()` modifies offset.
///
/// The ID is required (for event routing and hit-testing the scroll area).
pub struct ScrollColumn {
    /// Widget ID (required for hit-testing and scroll event routing).
    id: SourceId,
    /// Scrollbar thumb widget ID (for drag interaction).
    thumb_id: SourceId,
    /// Child elements.
    children: Vec<LayoutChild>,
    /// Current scroll offset (from app state).
    scroll_offset: f32,
    /// Spacing between children.
    spacing: f32,
    /// Padding around all children.
    padding: Padding,
    /// Background color (optional).
    background: Option<Color>,
    /// Corner radius for background.
    corner_radius: f32,
    /// Width sizing mode.
    pub(crate) width: Length,
    /// Height sizing mode.
    pub(crate) height: Length,
    /// Border color (optional).
    border_color: Option<Color>,
    /// Border width.
    border_width: f32,
}

impl ScrollColumn {
    /// Create a new scroll column with a required ID.
    pub fn new(id: SourceId, thumb_id: SourceId) -> Self {
        Self {
            id,
            thumb_id,
            children: Vec::new(),
            scroll_offset: 0.0,
            spacing: 0.0,
            padding: Padding::default(),
            background: None,
            corner_radius: 0.0,
            width: Length::Shrink,
            height: Length::Shrink,
            border_color: None,
            border_width: 0.0,
        }
    }

    /// Create from a `ScrollState`, copying id, thumb_id, and offset.
    ///
    /// This pulls all state-driven fields so you only chain layout props.
    pub fn from_state(state: &ScrollState) -> Self {
        let mut sc = Self::new(state.id(), state.thumb_id());
        sc.scroll_offset = state.offset;
        sc
    }

    /// Set the scroll offset (from app state).
    pub fn scroll_offset(mut self, offset: f32) -> Self {
        self.scroll_offset = offset;
        self
    }

    /// Set spacing between children.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self
    }

    /// Set padding (uniform on all sides).
    pub fn padding(mut self, padding: f32) -> Self {
        self.padding = Padding::all(padding);
        self
    }

    /// Set custom padding.
    pub fn padding_custom(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    /// Set background color.
    pub fn background(mut self, color: Color) -> Self {
        self.background = Some(color);
        self
    }

    /// Set corner radius for background.
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.corner_radius = radius;
        self
    }

    /// Set width sizing mode.
    pub fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Set height sizing mode.
    pub fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    /// Set border (color + width).
    pub fn border(mut self, color: Color, width: f32) -> Self {
        self.border_color = Some(color);
        self.border_width = width;
        self
    }

    /// Add a text element.
    pub fn text(mut self, element: TextElement) -> Self {
        self.children.push(LayoutChild::Text(element));
        self
    }

    /// Add a terminal element.
    pub fn terminal(mut self, element: TerminalElement) -> Self {
        self.children.push(LayoutChild::Terminal(element));
        self
    }

    /// Add a nested column.
    pub fn column(mut self, column: Column) -> Self {
        self.children.push(LayoutChild::Column(Box::new(column)));
        self
    }

    /// Add a nested row.
    pub fn row(mut self, row: Row) -> Self {
        self.children.push(LayoutChild::Row(Box::new(row)));
        self
    }

    /// Add a flexible spacer.
    pub fn spacer(mut self, flex: f32) -> Self {
        self.children.push(LayoutChild::Spacer { flex });
        self
    }

    /// Add a fixed-size spacer.
    pub fn fixed_spacer(mut self, size: f32) -> Self {
        self.children.push(LayoutChild::FixedSpacer { size });
        self
    }

    /// Add an image element.
    pub fn image(mut self, element: ImageElement) -> Self {
        self.children.push(LayoutChild::Image(element));
        self
    }

    /// Add a button element.
    pub fn button(mut self, element: ButtonElement) -> Self {
        self.children.push(LayoutChild::Button(element));
        self
    }

    /// Add a text input element.
    pub fn text_input(mut self, element: TextInputElement) -> Self {
        self.children.push(LayoutChild::TextInput(element));
        self
    }

    /// Add a table element.
    pub fn table(mut self, element: TableElement) -> Self {
        self.children.push(LayoutChild::Table(element));
        self
    }

    pub fn virtual_table(mut self, element: VirtualTableElement) -> Self {
        self.children.push(LayoutChild::VirtualTable(element));
        self
    }

    /// Add any child element using `From<T> for LayoutChild`.
    #[inline(always)]
    pub fn push(mut self, child: impl Into<LayoutChild>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Compute intrinsic size (content size + padding).
    pub fn measure(&self) -> Size {
        let intrinsic_width = match self.width {
            Length::Fixed(px) => px,
            _ => {
                let mut max_child_width: f32 = 0.0;
                for child in &self.children {
                    max_child_width = max_child_width.max(child.measure_cross(true));
                }
                max_child_width + self.padding.horizontal()
            }
        };

        let intrinsic_height = match self.height {
            Length::Fixed(px) => px,
            _ => {
                let mut total_height: f32 = 0.0;
                for child in &self.children {
                    if child.flex_factor(true) > 0.0 {
                        continue;
                    }
                    total_height += child.measure_main(true);
                }
                if self.children.len() > 1 {
                    total_height += self.spacing * (self.children.len() - 1) as f32;
                }
                total_height + self.padding.vertical()
            }
        };

        Size::new(intrinsic_width, intrinsic_height)
    }

    /// Compute layout and flush to snapshot.
    ///
    /// Implements virtualization: only children intersecting the viewport
    /// are laid out. A scrollbar thumb is drawn when content overflows.
    pub fn layout(self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        let content_x = bounds.x + self.padding.left;
        let full_content_width = bounds.width - self.padding.horizontal();
        let viewport_h = bounds.height;

        // Draw chrome outside clip
        if let Some(bg) = self.background {
            if self.corner_radius > 0.0 {
                snapshot.primitives_mut().add_rounded_rect(bounds, self.corner_radius, bg);
            } else {
                snapshot.primitives_mut().add_solid_rect(bounds, bg);
            }
        }
        if let Some(border_color) = self.border_color {
            snapshot.primitives_mut().add_border(
                bounds,
                self.corner_radius,
                self.border_width,
                border_color,
            );
        }

        // Push clip to viewport bounds
        snapshot.primitives_mut().push_clip(bounds);

        // Reserve space for scrollbar (we'll check if we need it after measuring).
        const SCROLLBAR_GUTTER: f32 = 24.0;

        // First pass: measure heights assuming no scrollbar
        let mut child_heights: Vec<f32> = Vec::with_capacity(self.children.len());
        let mut total_content_height = self.padding.vertical();
        for child in &self.children {
            let h = match child {
                LayoutChild::Flow(f) => f.height_for_width(full_content_width),
                LayoutChild::Row(r) => r.height_for_width(full_content_width),
                LayoutChild::Column(c) => c.height_for_width(full_content_width),
                _ => child.measure_main(true),
            };
            child_heights.push(h);
            total_content_height += h;
        }
        if self.children.len() > 1 {
            total_content_height += self.spacing * (self.children.len() - 1) as f32;
        }

        let overflows = total_content_height > viewport_h;
        let content_width = if overflows { full_content_width - SCROLLBAR_GUTTER } else { full_content_width };

        // If we overflow, re-measure width-dependent children with the reduced width
        if overflows {
            child_heights.clear();
            total_content_height = self.padding.vertical();
            for child in &self.children {
                let h = match child {
                    LayoutChild::Flow(f) => f.height_for_width(content_width),
                    LayoutChild::Row(r) => r.height_for_width(content_width),
                    LayoutChild::Column(c) => c.height_for_width(content_width),
                    _ => child.measure_main(true),
                };
                child_heights.push(h);
                total_content_height += h;
            }
            if self.children.len() > 1 {
                total_content_height += self.spacing * (self.children.len() - 1) as f32;
            }
        }

        // Register container widget for hit-testing (wheel events route here).
        // When overflowing, exclude the gutter so this doesn't compete with the
        // scrollbar thumb track widget in the HashMap-based hit test.
        let container_hit_width = if overflows { bounds.width - SCROLLBAR_GUTTER } else { bounds.width };
        snapshot.register_widget(self.id, Rect::new(bounds.x, bounds.y, container_hit_width, bounds.height));

        // Clamp scroll offset and record max for app-side clamping
        let max_scroll = (total_content_height - viewport_h).max(0.0);
        snapshot.set_scroll_limit(self.id, max_scroll);
        let offset = self.scroll_offset.clamp(0.0, max_scroll);

        // Position pass with virtualization
        let mut virtual_y = self.padding.top; // position in content space
        let viewport_top = offset;
        let viewport_bottom = offset + viewport_h;

        for (i, child) in self.children.into_iter().enumerate() {
            let h = child_heights[i];
            let child_top = virtual_y;
            let child_bottom = virtual_y + h;

            // Check if child intersects the viewport
            if child_bottom > viewport_top && child_top < viewport_bottom {
                // Compute screen-space Y
                let screen_y = bounds.y + child_top - offset;

                match child {
                    LayoutChild::Text(t) => {
                        let fs = t.font_size();
                        let size = t.estimate_size(CHAR_WIDTH, LINE_HEIGHT);
                        use crate::layout_snapshot::{SourceLayout, TextLayout};
                        if let Some(source_id) = t.source_id {
                            let scale = fs / BASE_FONT_SIZE;
                            let mut text_layout = TextLayout::simple(
                                t.text.clone(),
                                t.color.pack(),
                                content_x, screen_y,
                                CHAR_WIDTH * scale, LINE_HEIGHT * scale,
                            );
                            // Expand hit-box to full content width — in ScrollColumn,
                            // text owns the entire line so this is safe.
                            text_layout.bounds.width = text_layout.bounds.width.max(content_width);
                            snapshot.register_source(source_id, SourceLayout::text(text_layout));
                        }

                        // Register widget if this text is clickable
                        if let Some(widget_id) = t.widget_id {
                            let text_rect = Rect::new(content_x, screen_y, size.width, size.height);
                            snapshot.register_widget(widget_id, text_rect);
                            if let Some(cursor) = t.cursor_hint {
                                snapshot.set_cursor_hint(widget_id, cursor);
                            }
                        }

                        snapshot.primitives_mut().add_text_cached_styled(
                            t.text,
                            crate::primitives::Point::new(content_x, screen_y),
                            t.color,
                            fs,
                            t.cache_key,
                            t.bold,
                            t.italic,
                        );
                    }
                    LayoutChild::Terminal(t) => {
                        let size = t.size();

                        use crate::layout_snapshot::{GridLayout, GridRow, SourceLayout};
                        let rows_content: Vec<GridRow> = t.row_content.into_iter()
                            .map(|runs| GridRow { runs })
                            .collect();
                        let mut grid_layout = GridLayout::with_rows(
                            Rect::new(content_x, screen_y, size.width.max(content_width), size.height),
                            t.cell_width, t.cell_height,
                            t.cols, t.rows,
                            rows_content,
                        );
                        grid_layout.clip_rect = snapshot.current_clip();
                        snapshot.register_source(t.source_id, SourceLayout::grid(grid_layout));
                    }
                    LayoutChild::Image(img) => {
                        let img_rect = Rect::new(content_x, screen_y, img.width, img.height);
                        snapshot.primitives_mut().add_image(
                            img_rect,
                            img.handle,
                            img.corner_radius,
                            img.tint,
                        );
                        if let Some(id) = img.widget_id {
                            snapshot.register_widget(id, img_rect);
                        }
                    }
                    LayoutChild::Button(btn) => {
                        let size = btn.estimate_size();
                        let btn_rect = Rect::new(content_x, screen_y, size.width, size.height);
                        snapshot.primitives_mut().add_rounded_rect(btn_rect, btn.corner_radius, btn.background);
                        snapshot.primitives_mut().add_text_cached(
                            btn.label,
                            crate::primitives::Point::new(content_x + btn.padding.left, screen_y + btn.padding.top),
                            btn.text_color,
                            BASE_FONT_SIZE,
                            btn.cache_key,
                        );
                        snapshot.register_widget(btn.id, btn_rect);
                        snapshot.set_cursor_hint(btn.id, CursorIcon::Pointer);
                    }
                    LayoutChild::TextInput(input) => {
                        let w = match input.width {
                            Length::Fixed(px) => px,
                            Length::Fill | Length::FillPortion(_) => content_width,
                            Length::Shrink => input.estimate_size().width.min(content_width),
                        };
                        let h = if input.multiline {
                            input.estimate_size().height
                        } else {
                            LINE_HEIGHT + input.padding.vertical()
                        };
                        if input.multiline {
                            render_text_input_multiline(snapshot, input, content_x, screen_y, w, h);
                        } else {
                            render_text_input(snapshot, input, content_x, screen_y, w, h);
                        }
                    }
                    LayoutChild::Table(table) => {
                        let size = table.estimate_size();
                        let w = size.width.min(content_width);
                        render_table(snapshot, table, content_x, screen_y, w, size.height);
                    }
                    LayoutChild::VirtualTable(table) => {
                        let size = table.estimate_size();
                        let w = size.width.min(content_width);
                        render_virtual_table(snapshot, table, content_x, screen_y, w, size.height);
                    }
                    LayoutChild::Flow(flow) => {
                        let w = match flow.width {
                            Length::Fixed(px) => px,
                            Length::Fill | Length::FillPortion(_) | Length::Shrink => content_width,
                        };
                        flow.layout(snapshot, content_x, screen_y, w);
                    }
                    LayoutChild::Column(nested) => {
                        let w = match nested.width {
                            Length::Fixed(px) => px,
                            Length::Fill | Length::FillPortion(_) => content_width,
                            Length::Shrink => nested.measure().width.min(content_width),
                        };
                        nested.layout(snapshot, Rect::new(content_x, screen_y, w, h));
                    }
                    LayoutChild::Row(nested) => {
                        // Give Rows the full content width so their children's
                        // hit-boxes can expand to fill the line.
                        let w = match nested.width {
                            Length::Fixed(px) => px,
                            Length::Fill | Length::FillPortion(_) | Length::Shrink => content_width,
                        };
                        nested.layout(snapshot, Rect::new(content_x, screen_y, w, h));
                    }
                    LayoutChild::ScrollColumn(nested) => {
                        let w = match nested.width {
                            Length::Fixed(px) => px,
                            Length::Fill | Length::FillPortion(_) => content_width,
                            Length::Shrink => nested.measure().width.min(content_width),
                        };
                        nested.layout(snapshot, Rect::new(content_x, screen_y, w, h));
                    }
                    LayoutChild::Spacer { .. } | LayoutChild::FixedSpacer { .. } => {
                        // Spacers have no visual representation
                    }
                }
            }

            virtual_y += h + self.spacing;
        }

        // Draw scrollbar thumb if content overflows
        if total_content_height > viewport_h {
            let thumb_h = ((viewport_h / total_content_height) * viewport_h).max(20.0);
            let scroll_pct = if max_scroll > 0.0 { offset / max_scroll } else { 0.0 };
            let scroll_available = viewport_h - thumb_h;
            let thumb_y = bounds.y + scroll_pct * scroll_available;
            let thumb_visual = Rect::new(bounds.x + bounds.width - 8.0, thumb_y, 6.0, thumb_h);

            snapshot.primitives_mut().add_rounded_rect(
                thumb_visual,
                3.0,
                Color::rgba(1.0, 1.0, 1.0, 0.25),
            );

            // Register the full-height track as the hit region so clicking
            // anywhere in the scrollbar gutter initiates a drag.
            let track_hit = Rect::new(bounds.x + bounds.width - SCROLLBAR_GUTTER, bounds.y, SCROLLBAR_GUTTER, viewport_h);
            snapshot.register_widget(self.thumb_id, track_hit);
            snapshot.set_cursor_hint(self.thumb_id, CursorIcon::Grab);

            // Store track info so the app can convert mouse Y → scroll offset
            use crate::layout_snapshot::ScrollTrackInfo;
            snapshot.set_scroll_track(self.id, ScrollTrackInfo {
                track_y: bounds.y,
                track_height: viewport_h,
                thumb_height: thumb_h,
                max_scroll,
            });
        }

        // Pop clip
        snapshot.primitives_mut().pop_clip();
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // unicode_display_width tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_unicode_display_width_ascii() {
        assert_eq!(unicode_display_width("hello"), 5.0);
    }

    #[test]
    fn test_unicode_display_width_empty() {
        assert_eq!(unicode_display_width(""), 0.0);
    }

    #[test]
    fn test_unicode_display_width_cjk() {
        // CJK characters are double-width
        assert_eq!(unicode_display_width("中文"), 4.0);
    }

    #[test]
    fn test_unicode_display_width_mixed() {
        // "a中b" = 1 + 2 + 1 = 4
        assert_eq!(unicode_display_width("a中b"), 4.0);
    }

    #[test]
    fn test_unicode_display_width_emoji() {
        // Many emojis are double-width
        let width = unicode_display_width("😀");
        assert!(width >= 1.0); // At least 1, could be 2 depending on implementation
    }

    // -------------------------------------------------------------------------
    // unicode_col_x tests
    // -------------------------------------------------------------------------

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

    // -------------------------------------------------------------------------
    // hash_text tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_hash_text_same_input_same_hash() {
        let h1 = hash_text("hello world");
        let h2 = hash_text("hello world");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_text_different_input_different_hash() {
        let h1 = hash_text("hello");
        let h2 = hash_text("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hash_text_empty() {
        let h = hash_text("");
        // Just verify it doesn't panic and returns something
        assert!(h > 0 || h == 0); // Always true, but tests the call
    }

    // -------------------------------------------------------------------------
    // Length tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_length_flex_shrink() {
        assert_eq!(Length::Shrink.flex(), 0.0);
    }

    #[test]
    fn test_length_flex_fill() {
        assert_eq!(Length::Fill.flex(), 1.0);
    }

    #[test]
    fn test_length_flex_fill_portion() {
        assert_eq!(Length::FillPortion(3).flex(), 3.0);
    }

    #[test]
    fn test_length_flex_fixed() {
        assert_eq!(Length::Fixed(100.0).flex(), 0.0);
    }

    #[test]
    fn test_length_is_flex_shrink() {
        assert!(!Length::Shrink.is_flex());
    }

    #[test]
    fn test_length_is_flex_fill() {
        assert!(Length::Fill.is_flex());
    }

    #[test]
    fn test_length_is_flex_fill_portion() {
        assert!(Length::FillPortion(2).is_flex());
    }

    #[test]
    fn test_length_is_flex_fixed() {
        assert!(!Length::Fixed(50.0).is_flex());
    }

    // -------------------------------------------------------------------------
    // Padding tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_padding_new() {
        let p = Padding::new(1.0, 2.0, 3.0, 4.0);
        assert_eq!(p.top, 1.0);
        assert_eq!(p.right, 2.0);
        assert_eq!(p.bottom, 3.0);
        assert_eq!(p.left, 4.0);
    }

    #[test]
    fn test_padding_all() {
        let p = Padding::all(10.0);
        assert_eq!(p.top, 10.0);
        assert_eq!(p.right, 10.0);
        assert_eq!(p.bottom, 10.0);
        assert_eq!(p.left, 10.0);
    }

    #[test]
    fn test_padding_symmetric() {
        let p = Padding::symmetric(5.0, 10.0);
        assert_eq!(p.left, 5.0);
        assert_eq!(p.right, 5.0);
        assert_eq!(p.top, 10.0);
        assert_eq!(p.bottom, 10.0);
    }

    #[test]
    fn test_padding_horizontal() {
        let p = Padding::new(1.0, 20.0, 3.0, 10.0);
        assert_eq!(p.horizontal(), 30.0);
    }

    #[test]
    fn test_padding_vertical() {
        let p = Padding::new(15.0, 2.0, 25.0, 4.0);
        assert_eq!(p.vertical(), 40.0);
    }

    #[test]
    fn test_padding_default() {
        let p = Padding::default();
        assert_eq!(p.top, 0.0);
        assert_eq!(p.right, 0.0);
        assert_eq!(p.bottom, 0.0);
        assert_eq!(p.left, 0.0);
    }

    // -------------------------------------------------------------------------
    // Alignment/CrossAxisAlignment default tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_alignment_default() {
        let a = Alignment::default();
        assert_eq!(a, Alignment::Start);
    }

    #[test]
    fn test_cross_axis_alignment_default() {
        let a = CrossAxisAlignment::default();
        assert_eq!(a, CrossAxisAlignment::Start);
    }

    #[test]
    fn test_length_default() {
        let l = Length::default();
        assert_eq!(l, Length::Shrink);
    }

    // -------------------------------------------------------------------------
    // FlowContainer tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_flow_container_measure_single_line() {
        // Single text element should measure as single line
        let flow = FlowContainer::new()
            .push(TextElement::new("hello"));

        let size = flow.measure();
        // "hello" = 5 chars * CHAR_WIDTH
        assert_eq!(size.width, 5.0 * CHAR_WIDTH);
        assert_eq!(size.height, LINE_HEIGHT);
    }

    #[test]
    fn test_flow_container_measure_multiple_words() {
        // Multiple words measured as single line (measure doesn't wrap)
        let flow = FlowContainer::new()
            .push(TextElement::new("hello "))
            .push(TextElement::new("world"));

        let size = flow.measure();
        // "hello " (6) + "world" (5) = 11 chars
        assert_eq!(size.width, 11.0 * CHAR_WIDTH);
        assert_eq!(size.height, LINE_HEIGHT);
    }

    #[test]
    fn test_flow_container_height_for_width_no_wrap() {
        // Wide enough container - no wrapping needed
        let flow = FlowContainer::new()
            .push(TextElement::new("hello "))
            .push(TextElement::new("world"));

        // 11 chars * CHAR_WIDTH = 92.4, give it 100px
        let height = flow.height_for_width(100.0);
        assert_eq!(height, LINE_HEIGHT);
    }

    #[test]
    fn test_flow_container_height_for_width_with_wrap() {
        // Narrow container - should wrap to 2 lines
        let flow = FlowContainer::new()
            .push(TextElement::new("hello "))  // 6 chars = 50.4px
            .push(TextElement::new("world"));   // 5 chars = 42px

        // Give it 60px width - "hello " fits, "world" wraps
        let height = flow.height_for_width(60.0);
        println!("height_for_width(60.0) = {}, LINE_HEIGHT = {}", height, LINE_HEIGHT);
        // Should be more than single line (wrapping occurred)
        assert!(height > LINE_HEIGHT, "Expected wrapping to occur");
        // Should be approximately 2 lines
        assert!(height >= LINE_HEIGHT * 2.0 - 1.0 && height <= LINE_HEIGHT * 2.0 + 5.0,
            "Expected ~2 lines height, got {}", height);
    }

    #[test]
    fn test_flow_container_height_for_width_many_words() {
        // Multiple words that require multiple lines
        let flow = FlowContainer::new()
            .push(TextElement::new("one "))    // 4 chars
            .push(TextElement::new("two "))    // 4 chars
            .push(TextElement::new("three "))  // 6 chars
            .push(TextElement::new("four "))   // 5 chars
            .push(TextElement::new("five"));   // 4 chars

        // Total: 23 chars = 193.2px
        // With 80px width (~9.5 chars per line):
        // Line 1: "one two " (8 chars = 67.2px) - fits
        // Line 2: "three " (6 chars = 50.4px) - fits
        // Line 3: "four " (5 chars = 42px) - fits
        // Line 4: "five" (4 chars = 33.6px) - fits
        let height = flow.height_for_width(80.0);
        println!("CHAR_WIDTH={}, height={}, LINE_HEIGHT={}", CHAR_WIDTH, height, LINE_HEIGHT);
        // Should be multiple lines
        assert!(height > LINE_HEIGHT, "Expected multiple lines, got single line height");
    }

    #[test]
    fn test_layout_child_size() {
        let text = TextElement::new("test");
        let child = LayoutChild::Text(text);
        let size = child.size();
        assert_eq!(size.width, 4.0 * CHAR_WIDTH);
        assert_eq!(size.height, LINE_HEIGHT);
    }

    #[test]
    fn test_column_height_for_width_with_flow() {
        // Column containing a FlowContainer
        let flow = FlowContainer::new()
            .width(Length::Fill)
            .push(TextElement::new("hello "))
            .push(TextElement::new("world"));

        let col = Column::new()
            .push(flow);

        // Wide - no wrap
        let height_wide = col.height_for_width(200.0);
        assert_eq!(height_wide, LINE_HEIGHT);

        // Narrow - should wrap (2 lines + line_spacing between them)
        let height_narrow = col.height_for_width(60.0);
        assert_eq!(height_narrow, LINE_HEIGHT * 2.0 + 2.0); // 2.0 is default line_spacing
    }

    #[test]
    fn test_row_height_for_width_with_nested_column_flow() {
        // This mimics the markdown rendering structure:
        // Row [ bullet, Column [ FlowContainer ] ]
        let flow = FlowContainer::new()
            .width(Length::Fill)
            .push(TextElement::new("hello "))
            .push(TextElement::new("world"));

        let inner_col = Column::new()
            .width(Length::Fill)
            .push(flow);

        let row = Row::new()
            .push(TextElement::new("* "))  // 2 chars bullet
            .push(inner_col);

        // Wide - no wrap needed
        let height_wide = row.height_for_width(200.0);
        println!("height_wide = {}", height_wide);
        assert_eq!(height_wide, LINE_HEIGHT);

        // Narrow - should wrap the flow content
        // 60px total, minus bullet (2 chars = 16.8px) = ~43px for flow
        // "hello " (50.4px) won't fit, wraps
        let height_narrow = row.height_for_width(60.0);
        println!("height_narrow = {}", height_narrow);
        assert!(height_narrow > LINE_HEIGHT, "Expected wrapped height > single line");
    }

    #[test]
    fn test_agent_widget_markdown_structure() {
        // This test mimics the exact structure from nexus_widgets.rs:
        // Row {
        //   TextElement("●"),      // bullet
        //   Column {               // from markdown::render
        //     FlowContainer {      // paragraph
        //       TextElement("word1 "),
        //       TextElement("word2 "),
        //       TextElement("word3"),
        //     }
        //   }
        // }

        // Create a FlowContainer with multiple words (simulating a paragraph)
        let flow = FlowContainer::new()
            .width(Length::Fill)
            .push(TextElement::new("This "))
            .push(TextElement::new("is "))
            .push(TextElement::new("a "))
            .push(TextElement::new("test "))
            .push(TextElement::new("paragraph "))
            .push(TextElement::new("that "))
            .push(TextElement::new("should "))
            .push(TextElement::new("wrap "))
            .push(TextElement::new("to "))
            .push(TextElement::new("multiple "))
            .push(TextElement::new("lines."));

        // markdown::render returns a Column
        let markdown_column = Column::new()
            .width(Length::Fill)
            .spacing(2.0)
            .push(flow);

        // Agent widget wraps in Row with bullet (exactly like nexus_widgets.rs)
        let agent_row = Row::new()
            .spacing(6.0)
            .cross_align(CrossAxisAlignment::Start)
            .push(TextElement::new("\u{25CF}").color(Color::WHITE))  // ● bullet (1 char)
            .push(markdown_column);

        // Test with different widths
        let wide_height = agent_row.height_for_width(800.0);
        let medium_height = agent_row.height_for_width(300.0);
        let narrow_height = agent_row.height_for_width(150.0);

        println!("Agent widget heights:");
        println!("  wide (800px): {}", wide_height);
        println!("  medium (300px): {}", medium_height);
        println!("  narrow (150px): {}", narrow_height);
        println!("  LINE_HEIGHT: {}", LINE_HEIGHT);

        // Wide should fit on one line
        assert_eq!(wide_height, LINE_HEIGHT, "Wide layout should be single line");

        // Medium should require some wrapping
        assert!(medium_height > LINE_HEIGHT, "Medium layout should wrap");

        // Narrow should require more wrapping
        assert!(narrow_height > medium_height, "Narrow layout should wrap more than medium");
    }

    #[test]
    fn test_flow_wrapping_logic_directly() {
        // Test the actual wrapping math
        let word1_width = 6.0 * CHAR_WIDTH;  // "hello "
        let word2_width = 5.0 * CHAR_WIDTH;  // "world"
        let max_width: f32 = 60.0;

        // Simulating FlowContainer logic:
        let mut line_x: f32 = 0.0;
        let mut line_y: f32 = 0.0;
        let mut line_height: f32 = 0.0;
        let line_spacing: f32 = 0.0;

        // First word
        let size1 = Size::new(word1_width, LINE_HEIGHT);
        if line_x > 0.0 && line_x + size1.width > max_width {
            line_y += line_height + line_spacing;
            line_x = 0.0;
            line_height = 0.0;
        }
        line_x += size1.width;
        line_height = line_height.max(size1.height);
        println!("After word1: line_x={}, line_y={}, line_height={}", line_x, line_y, line_height);

        // Second word
        let size2 = Size::new(word2_width, LINE_HEIGHT);
        if line_x > 0.0 && line_x + size2.width > max_width {
            line_y += line_height + line_spacing;
            line_x = 0.0;
            line_height = 0.0;
            println!("WRAPPED! line_y now = {}", line_y);
        }
        line_x += size2.width;
        line_height = line_height.max(size2.height);
        println!("After word2: line_x={}, line_y={}, line_height={}", line_x, line_y, line_height);

        let total_height = line_y + line_height;
        println!("Total height = {}", total_height);

        // Check: word1 (50.4) + word2 (42) = 92.4 > 60, so should wrap
        assert!(word1_width + word2_width > max_width, "Words should exceed max_width");
        assert!(line_y > 0.0, "Should have wrapped to a new line");
        assert_eq!(total_height, LINE_HEIGHT * 2.0, "Should be 2 lines");
    }
}
