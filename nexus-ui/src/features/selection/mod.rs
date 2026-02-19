//! Selection widget — owns selection state and text extraction logic.

pub(crate) mod drag;
pub(crate) mod drop;
pub(crate) mod snap;

use nexus_api::BlockId;

use crate::data::agent_block::AgentBlock;
use crate::data::{Block, UnifiedBlockRef};
use crate::features::shell::block_manager::BlockManager;
use crate::ui::context_menu::ContextTarget;
use self::drag::PendingIntent;
use crate::app::message::{DragMsg, NexusMessage, SelectionMsg};
use crate::utils::ids as source_ids;
use strata::Command;
use strata::MouseResponse;
use strata::component::Ctx;
use strata::content_address::{ContentAddress, SourceId, SourceOrdering};
use strata::layout_snapshot::HitResult;
use strata::primitives::Point;
use strata::Selection;

/// Selection state and text extraction logic.
pub(crate) struct SelectionWidget {
    pub selection: Option<Selection>,
    pub is_selecting: bool,
    pub select_mode: self::drag::SelectMode,
    /// Snapped anchor range for word/line drag-extend.
    snap_origin: Option<(ContentAddress, ContentAddress)>,
    /// Visual origin point for rectangular selection (the mouse-down position).
    rect_origin: Option<Point>,
}

impl SelectionWidget {
    pub fn new() -> Self {
        Self {
            selection: None,
            is_selecting: false,
            select_mode: self::drag::SelectMode::Char,
            snap_origin: None,
            rect_origin: None,
        }
    }

    pub fn update(
        &mut self,
        msg: SelectionMsg,
        _ctx: &mut Ctx,
        snap_content: Option<&snap::SnapContent>,
    ) -> (Command<SelectionMsg>, ()) {
        match msg {
            SelectionMsg::Start(addr, mode, position) => {
                self.select_mode = mode;
                match (mode, snap_content) {
                    (drag::SelectMode::Word, Some(content)) => {
                        let (start, end) = snap::snap_word(&addr, content);
                        self.selection = Some(Selection::new(start.clone(), end.clone()));
                        self.snap_origin = Some((start, end));
                        self.rect_origin = None;
                    }
                    (drag::SelectMode::Line, Some(content)) => {
                        let (start, end) = snap::snap_line(&addr, content);
                        self.selection = Some(Selection::new(start.clone(), end.clone()));
                        self.snap_origin = Some((start, end));
                        self.rect_origin = None;
                    }
                    (drag::SelectMode::Rect, _) => {
                        let shape = strata::SelectionShape::Rectangular {
                            x_min: position.x,
                            x_max: position.x,
                        };
                        self.selection = Some(Selection::with_shape(addr.clone(), addr, shape));
                        self.snap_origin = None;
                        self.rect_origin = Some(position);
                    }
                    _ => {
                        // Char mode or no snap content: fall back to char behavior
                        self.selection = Some(Selection::new(addr.clone(), addr));
                        self.snap_origin = None;
                        self.rect_origin = None;
                    }
                }
                self.is_selecting = true;
            }
            SelectionMsg::Extend(addr, position) => {
                if let Some(sel) = &mut self.selection {
                    match (self.select_mode, snap_content, &self.snap_origin) {
                        (drag::SelectMode::Rect, _, _) => {
                            sel.focus = addr;
                            if let Some(origin) = self.rect_origin {
                                sel.shape = strata::SelectionShape::Rectangular {
                                    x_min: origin.x.min(position.x),
                                    x_max: origin.x.max(position.x),
                                };
                            }
                        }
                        (drag::SelectMode::Word, Some(content), Some((origin_start, origin_end))) => {
                            let (snapped_start, snapped_end) = snap::snap_word(&addr, content);
                            // Compare focus to origin to determine direction
                            let focus_before_origin = (addr.item_index, addr.content_offset)
                                < (origin_start.item_index, origin_start.content_offset);
                            if focus_before_origin {
                                sel.anchor = origin_end.clone();
                                sel.focus = snapped_start;
                            } else {
                                sel.anchor = origin_start.clone();
                                sel.focus = snapped_end;
                            }
                        }
                        (drag::SelectMode::Line, Some(content), Some((origin_start, origin_end))) => {
                            let (snapped_start, snapped_end) = snap::snap_line(&addr, content);
                            let focus_before_origin = (addr.item_index, addr.content_offset)
                                < (origin_start.item_index, origin_start.content_offset);
                            if focus_before_origin {
                                sel.anchor = origin_end.clone();
                                sel.focus = snapped_start;
                            } else {
                                sel.anchor = origin_start.clone();
                                sel.focus = snapped_end;
                            }
                        }
                        _ => {
                            sel.focus = addr;
                        }
                    }
                }
            }
            SelectionMsg::End => {
                self.is_selecting = false;
            }
            SelectionMsg::Clear => {
                self.selection = None;
                self.is_selecting = false;
                self.snap_origin = None;
                self.rect_origin = None;
            }
        }
        (Command::none(), ())
    }

    /// Select all content across all blocks.
    pub fn select_all(&mut self, blocks: &[Block], agent_blocks: &[AgentBlock]) {
        let ordering = build_source_ordering(blocks, agent_blocks);
        let sources = ordering.sources_in_order();
        if let (Some(&first), Some(&last)) = (sources.first(), sources.last()) {
            self.selection = Some(Selection::new(
                ContentAddress::start_of(first),
                ContentAddress::new(last, usize::MAX, usize::MAX),
            ));
            self.is_selecting = false;
        }
    }

    /// Check if a click hit falls inside the current non-collapsed selection.
    ///
    /// Returns `(source_id, content_address)` for starting a selection drag,
    /// or `None` if the click is outside the selection or there is no selection.
    pub fn hit_in_selection(
        &self,
        hit: &Option<HitResult>,
        shell_blocks: &[Block],
        agent_blocks: &[AgentBlock],
    ) -> Option<(SourceId, ContentAddress)> {
        let sel = self.selection.as_ref()?;
        if sel.is_collapsed() {
            return None;
        }

        match hit {
            Some(HitResult::Content(addr)) => {
                let ordering = build_source_ordering(shell_blocks, agent_blocks);
                if sel.contains(addr, &ordering) {
                    Some((addr.source_id, addr.clone()))
                } else {
                    None
                }
            }
            Some(HitResult::Widget(id)) => {
                let ordering = build_source_ordering(shell_blocks, agent_blocks);
                if sel.sources(&ordering).contains(id) {
                    Some((*id, ContentAddress::start_of(*id)))
                } else {
                    None
                }
            }
            None => None,
        }
    }

    /// If a left-click lands inside the current selection, start a selection drag.
    /// Returns `None` if no selection or click is outside it.
    pub fn route_selection_drag(
        &self,
        hit: &Option<HitResult>,
        shell_blocks: &[Block],
        agent_blocks: &[AgentBlock],
        position: Point,
    ) -> Option<MouseResponse<NexusMessage>> {
        let (source, origin_addr) = self.hit_in_selection(hit, shell_blocks, agent_blocks)?;
        let text = self.extract_selected_text(shell_blocks, agent_blocks).unwrap_or_default();
        let intent = PendingIntent::SelectionDrag { source, text, origin_addr };
        Some(MouseResponse::message_and_capture(
            NexusMessage::Drag(DragMsg::Start(intent, position)),
            source,
        ))
    }

    /// Extract selected text from content blocks (not input text selection).
    pub fn extract_selected_text(
        &self,
        blocks: &[Block],
        agent_blocks: &[AgentBlock],
    ) -> Option<String> {
        let sel = self.selection.as_ref()?;
        if sel.is_collapsed() {
            return None;
        }

        let ordering = build_source_ordering(blocks, agent_blocks);
        let sources = sel.sources(&ordering);

        if sources.is_empty() {
            return None;
        }

        let (start, end) = sel.normalized(&ordering);

        match sel.shape {
            strata::SelectionShape::Linear => {
                let mut parts: Vec<String> = Vec::new();
                for source_id in &sources {
                    let is_start = *source_id == start.source_id;
                    let is_end = *source_id == end.source_id;
                    if let Some(text) = extract_source_text(blocks, agent_blocks, *source_id, is_start, is_end, &start, &end) {
                        if !text.is_empty() {
                            parts.push(text);
                        }
                    }
                }
                let result = parts.join("\n");
                if result.is_empty() { None } else { Some(result) }
            }
            strata::SelectionShape::Rectangular { .. } => {
                let mut parts: Vec<String> = Vec::new();
                for source_id in &sources {
                    let is_start = *source_id == start.source_id;
                    let is_end = *source_id == end.source_id;
                    if let Some(text) = extract_source_text_rect(blocks, agent_blocks, *source_id, is_start, is_end, &start, &end) {
                        if !text.is_empty() {
                            parts.push(text);
                        }
                    }
                }
                let result = parts.join("\n");
                if result.is_empty() { None } else { Some(result) }
            }
        }
    }
}

/// Extract all text from a specific block (for context menu Copy).
pub(crate) fn extract_block_text(
    bm: &BlockManager,
    agent_blocks: &[AgentBlock],
    agent_block_index: &std::collections::HashMap<BlockId, usize>,
    input_text: &str,
    target: &ContextTarget,
) -> Option<String> {
    match target {
        ContextTarget::Block(block_id) => {
            let block = bm.get(*block_id)?;

            // If the block has native output, convert it to text
            if let Some(ref value) = block.structured_output {
                // Format tables as markdown
                if let nexus_api::Value::Table { columns, rows } = value {
                    return Some(format_table_as_markdown(columns, rows));
                }
                return Some(value.to_text());
            }

            // Otherwise extract from terminal grid (match view's grid selection)
            let grid = if block.parser.is_alternate_screen() {
                block.parser.grid()
            } else {
                block.parser.grid_with_scrollback()
            };

            let mut lines = Vec::new();
            for row in grid.rows_iter() {
                let mut text = String::with_capacity(row.len());
                for cell in row {
                    if cell.flags.wide_char_spacer { continue; }
                    cell.push_grapheme(&mut text);
                }
                let trimmed = text.trim_end();
                if !trimmed.is_empty() || !lines.is_empty() {
                    lines.push(trimmed.to_string());
                }
            }
            // Trim trailing empty lines
            while lines.last().map_or(false, |l| l.is_empty()) {
                lines.pop();
            }
            if lines.is_empty() {
                None
            } else {
                Some(lines.join("\n"))
            }
        }
        ContextTarget::AgentBlock(block_id) => {
            let idx = agent_block_index.get(block_id)?;
            let block = agent_blocks.get(*idx)?;

            let mut text = String::new();
            if !block.response.is_empty() {
                text.push_str(&block.response);
            }
            if text.is_empty() && !block.thinking.is_empty() {
                text.push_str(&block.thinking);
            }
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        ContextTarget::Input => {
            if input_text.is_empty() { None } else { Some(input_text.to_string()) }
        }
    }
}

// =========================================================================
// Internal helpers
// =========================================================================

/// Build a source ordering reflecting current document order.
pub(crate) fn build_source_ordering(blocks: &[Block], agent_blocks: &[AgentBlock]) -> SourceOrdering {
    let mut ordering = SourceOrdering::new();
    let unified = build_unified_refs(blocks, agent_blocks);
    for block_ref in &unified {
        match block_ref {
            UnifiedBlockRef::Shell(block) => {
                ordering.register(source_ids::shell_header(block.id));
                if let Some(ref value) = block.structured_output {
                    if matches!(value, nexus_api::Value::Table { .. }) {
                        ordering.register(source_ids::table(block.id));
                    } else {
                        ordering.register(source_ids::native(block.id));
                    }
                } else {
                    ordering.register(source_ids::shell_term(block.id));
                }
            }
            UnifiedBlockRef::Agent(block) => {
                ordering.register(source_ids::agent_query(block.id));
                if !block.thinking.is_empty() && !block.thinking_collapsed {
                    ordering.register(source_ids::agent_thinking(block.id));
                }
                for (i, _tool) in block.tools.iter().enumerate() {
                    ordering.register(source_ids::agent_tool(block.id, i));
                }
                if block.pending_permission.is_some() {
                    ordering.register(source_ids::agent_perm_text(block.id));
                }
                if block.pending_question.is_some() {
                    ordering.register(source_ids::agent_question_text(block.id));
                }
                if !block.response.is_empty() {
                    ordering.register(source_ids::agent_response(block.id));
                }
                ordering.register(source_ids::agent_footer(block.id));
            }
        }
    }
    ordering
}

fn build_unified_refs<'a>(blocks: &'a [Block], agent_blocks: &'a [AgentBlock]) -> Vec<UnifiedBlockRef<'a>> {
    let mut unified: Vec<UnifiedBlockRef> = Vec::with_capacity(blocks.len() + agent_blocks.len());
    for b in blocks {
        unified.push(UnifiedBlockRef::Shell(b));
    }
    for b in agent_blocks {
        unified.push(UnifiedBlockRef::Agent(b));
    }
    unified.sort_by_key(|b| match b {
        UnifiedBlockRef::Shell(b) => b.id.0,
        UnifiedBlockRef::Agent(b) => b.id.0,
    });
    unified
}

/// Extract text from a single source within a selection range.
fn extract_source_text(
    blocks: &[Block],
    agent_blocks: &[AgentBlock],
    source_id: SourceId,
    is_start: bool,
    is_end: bool,
    start: &ContentAddress,
    end: &ContentAddress,
) -> Option<String> {
    for block in blocks {
        if let Some(text) = extract_shell_source(block, source_id, is_start, is_end, start, end) {
            return Some(text);
        }
    }
    for block in agent_blocks {
        if let Some(text) = extract_agent_source(block, source_id, is_start, is_end, start, end) {
            return Some(text);
        }
    }
    None
}

/// Extract text from a shell block's source region.
fn extract_shell_source(
    block: &Block,
    source_id: SourceId,
    is_start: bool,
    is_end: bool,
    start: &ContentAddress,
    end: &ContentAddress,
) -> Option<String> {
    if source_id == source_ids::shell_header(block.id) {
        let text = format!("$ {}", block.command);
        let lines: Vec<&str> = text.lines().collect();
        return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
    }

    if source_id == source_ids::shell_term(block.id) && block.structured_output.is_none() {
        // Must match the view's grid selection (shell_block.rs build()) so that
        // content_offset values from the layout snapshot are compatible.
        let grid = if block.parser.is_alternate_screen() {
            block.parser.grid()
        } else {
            block.parser.grid_with_scrollback()
        };
        let cols = grid.cols() as usize;
        if cols == 0 {
            return Some(String::new());
        }

        let start_offset = if is_start { start.content_offset } else { 0 };
        let total_cells = grid.content_rows() as usize * cols;
        let end_offset = if is_end { end.content_offset } else { total_cells };

        if start_offset >= end_offset {
            return Some(String::new());
        }

        let rows: Vec<Vec<nexus_term::Cell>> = grid.rows_iter().map(|r| r.to_vec()).collect();
        return Some(extract_grid_range(&rows, cols, start_offset, end_offset));
    }

    if source_id == source_ids::native(block.id) {
        if let Some(ref value) = block.structured_output {
            let full_text = value.to_text();
            let lines: Vec<&str> = full_text.lines().collect();
            return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
        }
    }

    if source_id == source_ids::table(block.id) {
        if let Some(nexus_api::Value::Table { columns, rows }) = &block.structured_output {
            let text = format_table_as_markdown(columns, rows);
            let lines: Vec<&str> = text.lines().collect();
            return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
        }
    }

    None
}

/// Extract text from an agent block's source region.
fn extract_agent_source(
    block: &AgentBlock,
    source_id: SourceId,
    is_start: bool,
    is_end: bool,
    start: &ContentAddress,
    end: &ContentAddress,
) -> Option<String> {
    let extract = |text: String| {
        let lines: Vec<&str> = text.lines().collect();
        extract_multi_item_range(&lines, is_start, is_end, start, end)
    };

    if source_id == source_ids::agent_query(block.id) {
        let lines = vec!["?", block.query.as_str()];
        return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
    }

    if source_id == source_ids::agent_thinking(block.id) {
        let preview = if block.thinking.len() > 500 {
            format!("{}...", &block.thinking[..500])
        } else {
            block.thinking.clone()
        };
        return Some(extract(preview));
    }

    for (i, tool) in block.tools.iter().enumerate() {
        if source_id == source_ids::agent_tool(block.id, i) {
            return Some(extract(tool.extract_text()));
        }
    }

    if let Some(ref perm) = block.pending_permission {
        if source_id == source_ids::agent_perm_text(block.id) {
            let mut text = String::from("\u{26A0} Permission Required\n");
            text.push_str(&perm.description);
            text.push('\n');
            text.push_str(&perm.action);
            if let Some(ref dir) = perm.working_dir {
                text.push_str(&format!("\nin {}", dir));
            }
            return Some(extract(text));
        }
    }

    if let Some(ref q) = block.pending_question {
        if source_id == source_ids::agent_question_text(block.id) {
            let mut text = String::from("\u{2753} Claude is asking:\n");
            for question in &q.questions {
                text.push_str(&question.question);
                text.push('\n');
            }
            return Some(extract(text));
        }
    }

    if source_id == source_ids::agent_response(block.id) && !block.response.is_empty() {
        return Some(extract(block.response.clone()));
    }

    if source_id == source_ids::agent_footer(block.id) {
        return Some(extract(block.footer_text()));
    }

    None
}

/// Extract a range of characters from a terminal grid.
fn extract_grid_range(rows: &[Vec<nexus_term::Cell>], cols: usize, start: usize, end: usize) -> String {
    let start_row = start / cols;
    let start_col = start % cols;
    let end_row = end / cols;
    let end_col = end % cols;

    let mut result = String::new();
    for row_idx in start_row..=end_row {
        if row_idx >= rows.len() {
            break;
        }
        let row = &rows[row_idx];
        let col_start = if row_idx == start_row { start_col } else { 0 };
        let col_end = if row_idx == end_row { end_col } else { row.len() };

        let mut line = String::new();
        for cell in row.iter().skip(col_start).take(col_end.saturating_sub(col_start)) {
            if cell.flags.wide_char_spacer { continue; }
            cell.push_grapheme(&mut line);
        }

        result.push_str(line.trim_end());
        if row_idx < end_row {
            result.push('\n');
        }
    }
    result
}

/// Extract text from a multi-item source (each line is a separate item).
fn extract_multi_item_range(
    lines: &[&str],
    is_start: bool,
    is_end: bool,
    start: &ContentAddress,
    end: &ContentAddress,
) -> String {
    if lines.is_empty() {
        return String::new();
    }

    let start_item = if is_start { start.item_index } else { 0 };
    let end_item = if is_end { end.item_index } else { lines.len().saturating_sub(1) };

    if start_item > end_item || start_item >= lines.len() {
        return String::new();
    }

    let mut parts: Vec<String> = Vec::new();
    for i in start_item..=end_item.min(lines.len() - 1) {
        let line = lines[i];
        let chars: Vec<char> = line.chars().collect();
        let from = if i == start_item && is_start { start.content_offset.min(chars.len()) } else { 0 };
        let to = if i == end_item && is_end { end.content_offset.min(chars.len()) } else { chars.len() };
        if from <= to {
            parts.push(chars[from..to].iter().collect());
        }
    }
    parts.join("\n")
}

// =========================================================================
// Rectangular selection extraction
// =========================================================================

/// Extract text from a single source for rectangular selection.
/// For grids: every row gets the same column range.
/// For text: every line gets the same character-offset range.
fn extract_source_text_rect(
    blocks: &[Block],
    agent_blocks: &[AgentBlock],
    source_id: SourceId,
    is_start: bool,
    is_end: bool,
    start: &ContentAddress,
    end: &ContentAddress,
) -> Option<String> {
    for block in blocks {
        if let Some(text) = extract_shell_source_rect(block, source_id, is_start, is_end, start, end) {
            return Some(text);
        }
    }
    for block in agent_blocks {
        if let Some(text) = extract_agent_source_rect(block, source_id, is_start, is_end, start, end) {
            return Some(text);
        }
    }
    None
}

/// Extract rectangular selection from a shell block's source.
fn extract_shell_source_rect(
    block: &Block,
    source_id: SourceId,
    is_start: bool,
    is_end: bool,
    start: &ContentAddress,
    end: &ContentAddress,
) -> Option<String> {
    if source_id == source_ids::shell_header(block.id) {
        let text = format!("$ {}", block.command);
        let lines: Vec<&str> = text.lines().collect();
        return Some(extract_multi_item_range_rect(&lines, is_start, is_end, start, end));
    }

    if source_id == source_ids::shell_term(block.id) && block.structured_output.is_none() {
        let grid = if block.parser.is_alternate_screen() {
            block.parser.grid()
        } else {
            block.parser.grid_with_scrollback()
        };
        let cols = grid.cols() as usize;
        if cols == 0 {
            return Some(String::new());
        }

        let start_offset = if is_start { start.content_offset } else { 0 };
        let total_cells = grid.content_rows() as usize * cols;
        let end_offset = if is_end { end.content_offset } else { total_cells };

        if start_offset >= end_offset {
            return Some(String::new());
        }

        let rows: Vec<Vec<nexus_term::Cell>> = grid.rows_iter().map(|r| r.to_vec()).collect();
        return Some(extract_grid_range_rect(&rows, cols, start_offset, end_offset));
    }

    if source_id == source_ids::native(block.id) {
        if let Some(ref value) = block.structured_output {
            let full_text = value.to_text();
            let lines: Vec<&str> = full_text.lines().collect();
            return Some(extract_multi_item_range_rect(&lines, is_start, is_end, start, end));
        }
    }

    if source_id == source_ids::table(block.id) {
        if let Some(nexus_api::Value::Table { columns, rows }) = &block.structured_output {
            let text = format_table_as_markdown(columns, rows);
            let lines: Vec<&str> = text.lines().collect();
            return Some(extract_multi_item_range_rect(&lines, is_start, is_end, start, end));
        }
    }

    None
}

/// Extract rectangular selection from an agent block's source.
fn extract_agent_source_rect(
    block: &AgentBlock,
    source_id: SourceId,
    is_start: bool,
    is_end: bool,
    start: &ContentAddress,
    end: &ContentAddress,
) -> Option<String> {
    let extract = |text: String| {
        let lines: Vec<&str> = text.lines().collect();
        extract_multi_item_range_rect(&lines, is_start, is_end, start, end)
    };

    if source_id == source_ids::agent_query(block.id) {
        let lines = vec!["?", block.query.as_str()];
        return Some(extract_multi_item_range_rect(&lines, is_start, is_end, start, end));
    }

    if source_id == source_ids::agent_thinking(block.id) {
        let preview = if block.thinking.len() > 500 {
            format!("{}...", &block.thinking[..500])
        } else {
            block.thinking.clone()
        };
        return Some(extract(preview));
    }

    for (i, tool) in block.tools.iter().enumerate() {
        if source_id == source_ids::agent_tool(block.id, i) {
            return Some(extract(tool.extract_text()));
        }
    }

    if let Some(ref perm) = block.pending_permission {
        if source_id == source_ids::agent_perm_text(block.id) {
            let mut text = String::from("\u{26A0} Permission Required\n");
            text.push_str(&perm.description);
            text.push('\n');
            text.push_str(&perm.action);
            if let Some(ref dir) = perm.working_dir {
                text.push_str(&format!("\nin {}", dir));
            }
            return Some(extract(text));
        }
    }

    if let Some(ref q) = block.pending_question {
        if source_id == source_ids::agent_question_text(block.id) {
            let mut text = String::from("\u{2753} Claude is asking:\n");
            for question in &q.questions {
                text.push_str(&question.question);
                text.push('\n');
            }
            return Some(extract(text));
        }
    }

    if source_id == source_ids::agent_response(block.id) && !block.response.is_empty() {
        return Some(extract(block.response.clone()));
    }

    if source_id == source_ids::agent_footer(block.id) {
        return Some(extract(block.footer_text()));
    }

    None
}

/// Extract a rectangular range from a terminal grid.
/// Every row gets the same column range derived from the anchor/focus columns.
fn extract_grid_range_rect(rows: &[Vec<nexus_term::Cell>], cols: usize, start: usize, end: usize) -> String {
    let start_row = start / cols;
    let start_col = start % cols;
    let end_row = end / cols;
    let end_col = end % cols;

    // Fixed column range for all rows
    let col_min = start_col.min(end_col);
    let col_max = start_col.max(end_col);

    let mut result = String::new();
    let last_row = end_row.min(rows.len().saturating_sub(1));
    for row_idx in start_row..=last_row {
        let row = &rows[row_idx];
        let mut line = String::new();
        for cell in row.iter().skip(col_min).take(col_max.saturating_sub(col_min)) {
            if cell.flags.wide_char_spacer { continue; }
            cell.push_grapheme(&mut line);
        }
        result.push_str(line.trim_end());
        if row_idx < last_row {
            result.push('\n');
        }
    }
    result
}

/// Extract rectangular text from a multi-item source.
/// Every line gets the same character-offset range derived from the anchor/focus offsets.
fn extract_multi_item_range_rect(
    lines: &[&str],
    is_start: bool,
    is_end: bool,
    start: &ContentAddress,
    end: &ContentAddress,
) -> String {
    if lines.is_empty() {
        return String::new();
    }

    let start_item = if is_start { start.item_index } else { 0 };
    let end_item = if is_end { end.item_index } else { lines.len().saturating_sub(1) };

    if start_item > end_item || start_item >= lines.len() {
        return String::new();
    }

    // Fixed character-offset range for all lines
    let col_min = start.content_offset.min(end.content_offset);
    let col_max = start.content_offset.max(end.content_offset);

    let mut parts: Vec<String> = Vec::new();
    for i in start_item..=end_item.min(lines.len() - 1) {
        let line = lines[i];
        let chars: Vec<char> = line.chars().collect();
        let from = col_min.min(chars.len());
        let to = col_max.min(chars.len());
        if from <= to {
            let extracted: String = chars[from..to].iter().collect();
            // Pad with spaces to maintain rectangular shape
            let desired_width = col_max - col_min;
            let actual_width = to - from;
            if actual_width < desired_width {
                let mut padded = extracted;
                for _ in 0..(desired_width - actual_width) {
                    padded.push(' ');
                }
                parts.push(padded);
            } else {
                parts.push(extracted);
            }
        } else {
            // Line is entirely before the selection — pad with spaces
            parts.push(" ".repeat(col_max - col_min));
        }
    }
    parts.join("\n")
}

/// Format a table as a markdown table.
fn format_table_as_markdown(columns: &[nexus_api::TableColumn], rows: &[Vec<nexus_api::Value>]) -> String {
    let mut lines: Vec<String> = Vec::new();

    // Header row
    let header: String = columns
        .iter()
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    lines.push(format!("| {} |", header));

    // Separator row
    let separator: String = columns
        .iter()
        .map(|_| "---")
        .collect::<Vec<_>>()
        .join(" | ");
    lines.push(format!("| {} |", separator));

    // Data rows
    for row in rows {
        let cells: String = row
            .iter()
            .map(|cell| {
                // Escape pipes in cell content and replace newlines
                cell.to_text()
                    .replace('|', "\\|")
                    .replace('\n', " ")
            })
            .collect::<Vec<_>>()
            .join(" | ");
        lines.push(format!("| {} |", cells));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== SelectionWidget tests ==========

    #[test]
    fn test_selection_widget_new() {
        let widget = SelectionWidget::new();
        assert!(widget.selection.is_none());
        assert!(!widget.is_selecting);
    }

    #[test]
    fn test_selection_widget_default_select_mode() {
        let widget = SelectionWidget::new();
        assert!(matches!(widget.select_mode, crate::features::selection::drag::SelectMode::Char));
    }

    // ========== extract_multi_item_range tests ==========

    #[test]
    fn test_extract_multi_item_range_empty_lines() {
        let lines: Vec<&str> = vec![];
        let start = ContentAddress::new(SourceId::from_raw(1), 0, 0);
        let end = ContentAddress::new(SourceId::from_raw(1), 0, 5);

        let result = extract_multi_item_range(&lines, true, true, &start, &end);
        assert_eq!(result, "");
    }

    #[test]
    fn test_extract_multi_item_range_single_line_full() {
        let lines: Vec<&str> = vec!["Hello, World!"];
        let start = ContentAddress::new(SourceId::from_raw(1), 0, 0);
        let end = ContentAddress::new(SourceId::from_raw(1), 0, 13);

        let result = extract_multi_item_range(&lines, true, true, &start, &end);
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn test_extract_multi_item_range_single_line_partial() {
        let lines: Vec<&str> = vec!["Hello, World!"];
        let start = ContentAddress::new(SourceId::from_raw(1), 0, 0);
        let end = ContentAddress::new(SourceId::from_raw(1), 0, 5);

        let result = extract_multi_item_range(&lines, true, true, &start, &end);
        assert_eq!(result, "Hello");
    }

    #[test]
    fn test_extract_multi_item_range_single_line_middle() {
        let lines: Vec<&str> = vec!["Hello, World!"];
        let start = ContentAddress::new(SourceId::from_raw(1), 0, 7);
        let end = ContentAddress::new(SourceId::from_raw(1), 0, 12);

        let result = extract_multi_item_range(&lines, true, true, &start, &end);
        assert_eq!(result, "World");
    }

    #[test]
    fn test_extract_multi_item_range_multiple_lines() {
        let lines: Vec<&str> = vec!["Line 1", "Line 2", "Line 3"];
        let start = ContentAddress::new(SourceId::from_raw(1), 0, 0);
        let end = ContentAddress::new(SourceId::from_raw(1), 2, 6);

        let result = extract_multi_item_range(&lines, true, true, &start, &end);
        assert_eq!(result, "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn test_extract_multi_item_range_multiple_lines_partial() {
        let lines: Vec<&str> = vec!["Line 1", "Line 2", "Line 3"];
        let start = ContentAddress::new(SourceId::from_raw(1), 0, 5);
        let end = ContentAddress::new(SourceId::from_raw(1), 2, 4);

        let result = extract_multi_item_range(&lines, true, true, &start, &end);
        assert_eq!(result, "1\nLine 2\nLine");
    }

    #[test]
    fn test_extract_multi_item_range_not_start_source() {
        let lines: Vec<&str> = vec!["Line 1", "Line 2"];
        let start = ContentAddress::new(SourceId::from_raw(1), 0, 5);
        let end = ContentAddress::new(SourceId::from_raw(1), 1, 6);

        // is_start=false means we start from beginning of first line
        let result = extract_multi_item_range(&lines, false, true, &start, &end);
        assert_eq!(result, "Line 1\nLine 2");
    }

    #[test]
    fn test_extract_multi_item_range_not_end_source() {
        let lines: Vec<&str> = vec!["Line 1", "Line 2"];
        let start = ContentAddress::new(SourceId::from_raw(1), 0, 0);
        let end = ContentAddress::new(SourceId::from_raw(1), 0, 3);

        // is_end=false means we go to end of last line
        let result = extract_multi_item_range(&lines, true, false, &start, &end);
        assert_eq!(result, "Line 1\nLine 2");
    }

    #[test]
    fn test_extract_multi_item_range_start_beyond_lines() {
        let lines: Vec<&str> = vec!["Line 1"];
        let start = ContentAddress::new(SourceId::from_raw(1), 5, 0);
        let end = ContentAddress::new(SourceId::from_raw(1), 5, 10);

        let result = extract_multi_item_range(&lines, true, true, &start, &end);
        assert_eq!(result, "");
    }

}
