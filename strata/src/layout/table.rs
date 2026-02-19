//! TableElement - Table with headers and data rows.
//!
//! Supports sortable column headers, clickable cells, text selection,
//! and row striping. VirtualTableElement provides O(visible) rendering
//! for large datasets.

use crate::content_address::SourceId;
use crate::layout_snapshot::{CursorIcon, LayoutSnapshot};
use crate::primitives::{Color, Rect, Size};

use super::elements::{unicode_display_width, hash_text};
use super::length::BASE_FONT_SIZE;

// =========================================================================
// TableColumn and TableCell
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

// =========================================================================
// TableElement
// =========================================================================

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
pub(crate) fn render_table(
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

    // Header text + sortable widget registration (no source items — headers
    // are interactive/display-only, not text-selectable)
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
    let char_width = 8.4_f32;
    // Get clip bounds for row-level culling (from parent ScrollColumn)
    let clip_bounds = snapshot.primitives().current_clip_bounds();

    // Pre-compute row y-positions (needed for both source registration and rendering)
    let mut row_y_positions: Vec<f32> = Vec::with_capacity(table.rows.len());
    {
        let mut ry = data_y;
        for row in &table.rows {
            row_y_positions.push(ry);
            ry += table.row_height_for(row);
        }
    }

    // Register source items for ALL rows so ContentAddress.item_index
    // remains stable across scroll frames. Only primitive rendering below
    // is culled to the visible region.
    {
        use crate::layout_snapshot::{SourceLayout, TextLayout};
        for (row_idx, row) in table.rows.iter().enumerate() {
            let ry = row_y_positions[row_idx];
            let mut col_x = x;
            for (col_idx, cell) in row.iter().enumerate() {
                if col_idx < table.columns.len() {
                    if cell.lines.len() <= 1 {
                        let text = if cell.lines.len() == 1 { &cell.lines[0] } else { &cell.text };
                        let tx = col_x + cell_pad;
                        let ty = ry + 2.0;
                        let mut text_layout = TextLayout::simple(
                            text.clone(), cell.color.pack(),
                            tx, ty, char_width, table.line_height,
                        );
                        text_layout.bounds.width = text_layout.bounds.width.max(table.columns[col_idx].width - cell_pad);
                        snapshot.register_source(table.source_id, SourceLayout::text(text_layout));
                    } else {
                        for (line_idx, line) in cell.lines.iter().enumerate() {
                            let tx = col_x + cell_pad;
                            let ly = ry + 2.0 + line_idx as f32 * table.line_height;
                            let mut text_layout = TextLayout::simple(
                                line.clone(), cell.color.pack(),
                                tx, ly, char_width, table.line_height,
                            );
                            text_layout.bounds.width = text_layout.bounds.width.max(table.columns[col_idx].width - cell_pad);
                            snapshot.register_source(table.source_id, SourceLayout::text(text_layout));
                        }
                    }
                    col_x += table.columns[col_idx].width;
                }
            }
        }
    }

    // Render only visible rows (primitives + widgets)
    for (row_idx, row) in table.rows.iter().enumerate() {
        let ry = row_y_positions[row_idx];
        let rh = table.row_height_for(row);

        // Cull rows entirely outside the clip region (viewport)
        if let Some(clip) = clip_bounds {
            if ry + rh < clip.y || ry > clip.y + clip.height {
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
                    if let Some(wid) = cell.widget_id {
                        let text_rect = Rect::new(tx, ty, text_width, table.line_height);
                        snapshot.register_widget(wid, text_rect);
                        snapshot.set_cursor_hint(wid, CursorIcon::Pointer);
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
                    }
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
    }
}

// =========================================================================
// VirtualTableElement (O(visible) rendering)
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
pub(crate) fn render_virtual_table(
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

    // Header text + sortable widget registration (no source items — headers
    // are interactive/display-only, not text-selectable)
    let mut col_x = x;
    let num_cols = table.columns.len();
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

    // Register source items for VISIBLE rows only, with an offset so that
    // ContentAddress.item_index = row * num_cols + col stays stable across
    // scroll frames. The item_index_offset tells the snapshot "items[0]
    // corresponds to logical index first_visible * num_cols".
    {
        use crate::layout_snapshot::{SourceLayout, TextLayout};
        let item_offset = first_visible * num_cols;
        for row_idx in first_visible..last_visible {
            let row = &table.rows[row_idx];
            let ry = data_y + row_idx as f32 * table.row_height;
            let mut col_x = x;
            for (col_idx, cell) in row.iter().enumerate() {
                if col_idx < num_cols {
                    let tx = col_x + cell_pad;
                    let ty = ry + 2.0;
                    let mut text_layout = TextLayout::simple(
                        cell.text.clone(), cell.color.pack(),
                        tx, ty, char_width, table.line_height,
                    );
                    text_layout.bounds.width = text_layout.bounds.width.max(table.columns[col_idx].width - cell_pad);
                    snapshot.register_source_with_offset(table.source_id, SourceLayout::text(text_layout), item_offset);
                    col_x += table.columns[col_idx].width;
                }
            }
        }
    }

    // Render only visible rows (primitives + widgets)
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
                col_x += table.columns[col_idx].width;
            }
        }
    }
}
