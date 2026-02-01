//! Selection widget — owns selection state and text extraction logic.

use std::collections::HashMap;

use nexus_api::BlockId;

use crate::agent_block::AgentBlock;
use crate::blocks::{Block, UnifiedBlockRef};
use super::context_menu::ContextTarget;
use super::message::SelectionMsg;
use super::source_ids;
use strata::Command;
use strata::component::Ctx;
use strata::content_address::{ContentAddress, SourceId, SourceOrdering};
use strata::layout_snapshot::HitResult;
use strata::Selection;

/// Selection state and text extraction logic.
pub(crate) struct SelectionWidget {
    pub selection: Option<Selection>,
    pub is_selecting: bool,
    pub select_mode: super::drag_state::SelectMode,
}

impl SelectionWidget {
    pub fn new() -> Self {
        Self {
            selection: None,
            is_selecting: false,
            select_mode: super::drag_state::SelectMode::Char,
        }
    }

    pub fn update(&mut self, msg: SelectionMsg, _ctx: &mut Ctx) -> (Command<SelectionMsg>, ()) {
        match msg {
            SelectionMsg::Start(addr, mode) => {
                self.select_mode = mode;
                self.selection = Some(Selection::new(addr.clone(), addr));
                self.is_selecting = true;
            }
            SelectionMsg::Extend(addr) => {
                if let Some(sel) = &mut self.selection {
                    sel.focus = addr;
                }
            }
            SelectionMsg::End => {
                self.is_selecting = false;
            }
            SelectionMsg::Clear => {
                self.selection = None;
                self.is_selecting = false;
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
}

/// Extract all text from a specific block (for context menu Copy).
pub(crate) fn extract_block_text(
    blocks: &[Block],
    block_index: &HashMap<BlockId, usize>,
    agent_blocks: &[AgentBlock],
    agent_block_index: &HashMap<BlockId, usize>,
    input_text: &str,
    target: &ContextTarget,
) -> Option<String> {
    match target {
        ContextTarget::Block(block_id) => {
            let idx = block_index.get(block_id)?;
            let block = blocks.get(*idx)?;

            // If the block has native output, convert it to text
            if let Some(ref value) = block.native_output {
                return Some(value.to_text());
            }

            // Otherwise extract from terminal grid
            let grid = if block.parser.is_alternate_screen() || block.is_running() {
                block.parser.grid()
            } else {
                block.parser.grid_with_scrollback()
            };

            let mut lines = Vec::new();
            for row in grid.rows_iter() {
                let text: String = row.iter().map(|cell| cell.c).collect();
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
                if let Some(ref value) = block.native_output {
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
                if !block.response.is_empty() {
                    ordering.register(source_ids::agent_response(block.id));
                }
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
    // Shell blocks
    for block in blocks {
        let header_id = source_ids::shell_header(block.id);
        if header_id == source_id {
            let text = format!("$ {}", block.command);
            let lines: Vec<&str> = text.lines().collect();
            return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
        }

        let term_id = source_ids::shell_term(block.id);
        if term_id == source_id && block.native_output.is_none() {
            let grid = if block.parser.is_alternate_screen() || block.is_running() {
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

        let native_id = source_ids::native(block.id);
        if native_id == source_id {
            if let Some(ref value) = block.native_output {
                let full_text = value.to_text();
                let lines: Vec<&str> = full_text.lines().collect();
                return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
            }
        }

        let table_id = source_ids::table(block.id);
        if table_id == source_id {
            if let Some(nexus_api::Value::Table { columns, rows }) = &block.native_output {
                let mut lines: Vec<String> = Vec::new();
                for col in columns {
                    lines.push(col.name.clone());
                }
                for row in rows {
                    for cell in row {
                        let text = cell.to_text();
                        for l in text.lines() {
                            lines.push(l.to_string());
                        }
                    }
                }
                let line_refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
                return Some(extract_multi_item_range(&line_refs, is_start, is_end, start, end));
            }
        }
    }

    // Agent blocks
    for block in agent_blocks {
        let query_id = source_ids::agent_query(block.id);
        if query_id == source_id {
            let lines: Vec<&str> = vec!["?", &block.query];
            return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
        }

        let thinking_id = source_ids::agent_thinking(block.id);
        if thinking_id == source_id {
            let preview = if block.thinking.len() > 500 {
                format!("{}...", &block.thinking[..500])
            } else {
                block.thinking.clone()
            };
            let lines: Vec<&str> = preview.lines().collect();
            return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
        }

        let response_id = source_ids::agent_response(block.id);
        if response_id == source_id {
            if block.response.is_empty() {
                return None;
            }
            let lines: Vec<&str> = block.response.lines().collect();
            return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
        }
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

        let line: String = row.iter()
            .skip(col_start)
            .take(col_end.saturating_sub(col_start))
            .map(|cell| cell.c)
            .collect();

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
