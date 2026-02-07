//! Table rendering with virtualized rows and sortable columns.

use std::cell::RefCell;
use std::collections::HashMap;

use nexus_api::{Value, format_value_for_display};

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

/// Render a table with virtualized rows and sortable columns.
pub(super) fn render_table<'a>(
    parent: Column<'a>,
    columns: &[nexus_api::TableColumn],
    rows: &[Vec<Value>],
    block: &Block,
    click_registry: &RefCell<HashMap<SourceId, ClickAction>>,
) -> Column<'a> {
    let block_id = block.id;
    let _t0 = std::time::Instant::now();
    let source_id = ids::table(block_id);
    let num_cols = columns.len();

    // Column width estimation: sample first 100 rows (O(1) vs O(n))
    let sample_count = rows.len().min(100);
    let mut max_col_lens = vec![0usize; num_cols];
    for row in rows[..sample_count].iter() {
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

    let mut table = VirtualTableElement::new(source_id);

    for (i, col) in columns.iter().enumerate() {
        let sort_id = ids::table_sort(block_id, i);
        let header_name = if block.table_sort.column == Some(i) {
            if block.table_sort.ascending {
                format!("{} \u{25B2}", col.name)
            } else {
                format!("{} \u{25BC}", col.name)
            }
        } else {
            col.name.clone()
        };
        table = table.column_sortable(&header_name, col_widths[i], sort_id);
    }

    let mut anchor_idx = 0usize;
    for (_row_idx, row) in rows.iter().enumerate() {
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
                frame, _t1, _t2 - _t1, rows.len(), num_cols);
        }
    }
    result
}
