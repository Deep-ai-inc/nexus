//! Table rendering with virtualized rows and sortable columns.

use std::cell::RefCell;
use std::collections::HashMap;

use nexus_api::{BlockId, Value, format_value_for_display};

use crate::data::Block;
use crate::features::selection::drag::DragPayload;
use crate::features::shell::{
    AnchorEntry, ClickAction, register_anchor, semantic_text_for_value, value_to_anchor_action,
};
use crate::utils::ids;
use strata::content_address::SourceId;
use strata::layout::{Column, VirtualCell, VirtualTableElement};

use super::color::value_text_color;
use super::is_anchor_value;

const TABLE_CHAR_W: f32 = 8.4;
const TABLE_CELL_PADDING: f32 = 16.0;
const TABLE_MAX_COL_W: f32 = 400.0;

// =========================================================================
// Table Layout Cache — transient UI geometry for cell hit-testing
// =========================================================================

/// Cached table geometry for position-based cell hit-testing (context menus).
/// Populated during `render_table()`, cleared at the start of each view() pass.
/// Lives in the render layer — never serialized or diffed.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TableLayout {
    pub col_widths: Vec<f32>,
    pub row_height: f32,
    /// Y offset from the table origin to the first data row
    /// (accounts for header height + separator).
    pub header_height: f32,
    pub row_count: usize,
}

/// Cache of table geometry per block, populated during rendering.
/// Shared via RefCell, same lifecycle as click_registry (cleared per frame).
pub type TableLayoutCache = RefCell<HashMap<BlockId, TableLayout>>;

/// Render a table with virtualized rows and sortable columns.
/// When `block.filtered_row_indices` is `Some`, only those rows are rendered
/// and the header shows a filter count indicator.
pub(super) fn render_table<'a>(
    parent: Column<'a>,
    columns: &[nexus_api::TableColumn],
    rows: &[Vec<Value>],
    block: &Block,
    click_registry: &RefCell<HashMap<SourceId, ClickAction>>,
    table_layout_cache: &TableLayoutCache,
) -> Column<'a> {
    let block_id = block.id;
    let _t0 = std::time::Instant::now();
    let source_id = ids::table(block_id);
    let num_cols = columns.len();

    // Determine which rows to display
    let visible_indices: Option<&[usize]> = block.filtered_row_indices.as_deref();
    let visible_count = visible_indices.map_or(rows.len(), |idx| idx.len());

    // Column width estimation: sample first 100 visible rows (O(1) vs O(n))
    let sample_count = visible_count.min(100);
    let mut max_col_lens = vec![0usize; num_cols];
    for i in 0..sample_count {
        let row_idx = visible_indices.map_or(i, |idx| idx[i]);
        let row = match rows.get(row_idx) {
            Some(r) => r,
            None => continue,
        };
        for (col_idx, cell) in row.iter().enumerate() {
            if col_idx >= num_cols { break; }
            let text = if let Some(fmt) = columns.get(col_idx).and_then(|c| c.format) {
                format_value_for_display(cell, fmt)
            } else {
                cell.to_text()
            };
            let line_len = text.lines()
                .map(|l| unicode_width::UnicodeWidthStr::width(l))
                .max().unwrap_or(0);
            if line_len > max_col_lens[col_idx] {
                max_col_lens[col_idx] = line_len;
            }
        }
    }

    let col_widths: Vec<f32> = columns.iter().enumerate().map(|(i, col)| {
        let header_width = unicode_width::UnicodeWidthStr::width(col.name.as_str());
        let max_len = header_width.max(max_col_lens[i]).max(4);
        (max_len as f32 * TABLE_CHAR_W + TABLE_CELL_PADDING).min(TABLE_MAX_COL_W)
    }).collect();

    // Cache table geometry for cell hit-testing (context menus)
    table_layout_cache.borrow_mut().insert(block_id, TableLayout {
        col_widths: col_widths.clone(),
        row_height: 22.0, // matches VirtualTableElement default
        header_height: 26.0, // matches VirtualTableElement default
        row_count: visible_count,
    });

    let mut table = VirtualTableElement::new(source_id);
    let is_filtered = visible_indices.is_some();

    for (i, col) in columns.iter().enumerate() {
        let sort_id = ids::table_sort(block_id, i);
        let has_col_filter = block.table_filter.filters.contains_key(&i);

        // Build header: sort indicator + filter indicator
        let mut header_name = col.name.clone();
        if has_col_filter {
            header_name = format!("* {}", header_name);
        }
        if block.table_sort.column == Some(i) {
            if block.table_sort.ascending {
                header_name = format!("{} \u{25B2}", header_name);
            } else {
                header_name = format!("{} \u{25BC}", header_name);
            }
        }
        table = table.column_sortable(&header_name, col_widths[i], sort_id);
    }

    // Show filter count in a status line if filtered
    let parent = if is_filtered {
        let status = format!("Showing {} of {} rows", visible_count, rows.len());
        parent.push(strata::layout::TextElement::new(status).color(crate::ui::theme::TEXT_MUTED))
    } else {
        parent
    };

    let mut anchor_idx = 0usize;
    for i in 0..visible_count {
        let row_idx = visible_indices.map_or(i, |idx| idx[i]);
        let row = match rows.get(row_idx) {
            Some(r) => r,
            None => continue,
        };
        let cells: Vec<VirtualCell> = row.iter().enumerate().map(|(col_idx, cell)| {
            let text = if let Some(fmt) = columns.get(col_idx).and_then(|c| c.format) {
                format_value_for_display(cell, fmt)
            } else {
                cell.to_text()
            };
            let widget_id = if is_anchor_value(cell) {
                let id = ids::anchor(block_id, anchor_idx);
                register_anchor(click_registry, id, AnchorEntry {
                    block_id,
                    action: value_to_anchor_action(cell),
                    drag_payload: DragPayload::TableRow {
                        block_id,
                        row_index: anchor_idx,
                        display: semantic_text_for_value(cell, columns.get(col_idx)),
                    },
                    table_cell: Some((row_idx, col_idx)),
                });
                anchor_idx += 1;
                Some(id)
            } else {
                None
            };
            VirtualCell {
                text,
                color: value_text_color(cell),
                widget_id,
            }
        }).collect();
        table = table.row(cells);
    }

    let _t1 = _t0.elapsed();
    let result = parent.push(table);
    let _t2 = _t0.elapsed();
    if strata::frame_timing::is_enabled() {
        let frame = strata::frame_timing::current_frame();
        if frame % 60 == 0 {
            eprintln!("[frame {}] vtable build: {:.2?} layout={:.2?} ({}rows x {}cols)",
                frame, _t1, _t2 - _t1, visible_count, num_cols);
        }
    }
    result
}
