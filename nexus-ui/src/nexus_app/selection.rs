//! Selection widget — owns selection state and text extraction logic.

use std::collections::HashMap;

use nexus_api::BlockId;

use crate::agent_block::{AgentBlock, AgentBlockState, ToolInvocation};
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
                // Format tables as markdown
                if let nexus_api::Value::Table { columns, rows } = value {
                    return Some(format_table_as_markdown(columns, rows));
                }
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
                // Tables have one item per cell in the UI, but markdown has one line per row.
                // Rather than complex mapping, copy the full table as markdown when selected.
                return Some(format_table_as_markdown(columns, rows));
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

        for (i, tool) in block.tools.iter().enumerate() {
            let tool_id = source_ids::agent_tool(block.id, i);
            if tool_id == source_id {
                let text = extract_tool_text(tool);
                let lines: Vec<&str> = text.lines().collect();
                return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
            }
        }

        if let Some(ref perm) = block.pending_permission {
            let perm_id = source_ids::agent_perm_text(block.id);
            if perm_id == source_id {
                let mut text = String::from("\u{26A0} Permission Required\n");
                text.push_str(&perm.description);
                text.push('\n');
                text.push_str(&perm.action);
                if let Some(ref dir) = perm.working_dir {
                    text.push('\n');
                    text.push_str(&format!("in {}", dir));
                }
                let lines: Vec<&str> = text.lines().collect();
                return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
            }
        }

        if let Some(ref q) = block.pending_question {
            let q_id = source_ids::agent_question_text(block.id);
            if q_id == source_id {
                let mut text = String::from("\u{2753} Claude is asking:\n");
                for question in &q.questions {
                    text.push_str(&question.question);
                    text.push('\n');
                }
                let lines: Vec<&str> = text.lines().collect();
                return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
            }
        }

        let response_id = source_ids::agent_response(block.id);
        if response_id == source_id {
            if block.response.is_empty() {
                return None;
            }
            let lines: Vec<&str> = block.response.lines().collect();
            return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
        }

        let footer_id = source_ids::agent_footer(block.id);
        if footer_id == source_id {
            let text = extract_agent_footer_text(block);
            let lines: Vec<&str> = text.lines().collect();
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

/// Gather all visible text from a tool invocation for copy/selection extraction.
fn extract_tool_text(tool: &ToolInvocation) -> String {
    let mut parts: Vec<String> = Vec::new();

    // Header label (mirrors tool_header_label in nexus_widgets)
    parts.push(tool.name.clone());

    if let Some(ref msg) = tool.message {
        parts.push(msg.clone());
    }

    if tool.collapsed {
        // Collapsed summary — just the output first line
        if let Some(ref output) = tool.output {
            if let Some(first) = output.lines().next() {
                let truncated = if first.len() > 200 { &first[..200] } else { first };
                parts.push(truncated.to_string());
            }
        }
    } else {
        // Expanded body — parameters + output
        for (name, value) in &tool.parameters {
            let display = if value.len() > 100 {
                format!("{}: {}...", name, &value[..100])
            } else {
                format!("{}: {}", name, value)
            };
            parts.push(display);
        }
        if let Some(ref output) = tool.output {
            parts.push(output.clone());
        }
    }

    parts.join("\n")
}

/// Gather footer text from an agent block for copy/selection extraction.
fn extract_agent_footer_text(block: &AgentBlock) -> String {
    let mut parts: Vec<String> = Vec::new();

    let status = match &block.state {
        AgentBlockState::Pending => "Waiting...",
        AgentBlockState::Streaming => "Streaming...",
        AgentBlockState::Thinking => "Thinking...",
        AgentBlockState::Executing => "Executing...",
        AgentBlockState::Completed => "Completed",
        AgentBlockState::Failed(err) => err.as_str(),
        AgentBlockState::AwaitingPermission => "Awaiting permission...",
        AgentBlockState::Interrupted => "Interrupted",
    };
    parts.push(status.to_string());

    if let Some(ms) = block.duration_ms {
        if ms < 1000 {
            parts.push(format!("{}ms", ms));
        } else {
            parts.push(format!("{:.1}s", ms as f64 / 1000.0));
        }
    }

    if let Some(cost) = block.cost_usd {
        parts.push(format!("${:.4}", cost));
    }

    let total_tokens = block.input_tokens.unwrap_or(0) + block.output_tokens.unwrap_or(0);
    if total_tokens > 0 {
        if total_tokens >= 1_000_000 {
            parts.push(format!("{:.1}M tokens", total_tokens as f64 / 1_000_000.0));
        } else if total_tokens >= 1_000 {
            parts.push(format!("{:.1}k tokens", total_tokens as f64 / 1_000.0));
        } else {
            parts.push(format!("{} tokens", total_tokens));
        }
    }

    parts.join(" ")
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
        assert!(matches!(widget.select_mode, super::super::drag_state::SelectMode::Char));
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

    // ========== extract_tool_text tests ==========

    use crate::agent_block::ToolStatus;
    use std::collections::HashMap;

    fn make_test_tool(name: &str) -> ToolInvocation {
        ToolInvocation {
            id: "tool-1".to_string(),
            name: name.to_string(),
            parameters: HashMap::new(),
            output: None,
            status: ToolStatus::Success,
            message: None,
            collapsed: false,
        }
    }

    #[test]
    fn test_extract_tool_text_basic() {
        let mut tool = make_test_tool("read_file");
        tool.parameters.insert("path".to_string(), "/test/file.txt".to_string());
        tool.output = Some("File contents here".to_string());

        let result = extract_tool_text(&tool);
        assert!(result.contains("read_file"));
        assert!(result.contains("path: /test/file.txt"));
        assert!(result.contains("File contents here"));
    }

    #[test]
    fn test_extract_tool_text_collapsed() {
        let mut tool = make_test_tool("bash");
        tool.parameters.insert("command".to_string(), "ls -la".to_string());
        tool.output = Some("First line\nSecond line\nThird line".to_string());
        tool.collapsed = true;

        let result = extract_tool_text(&tool);
        assert!(result.contains("bash"));
        assert!(result.contains("First line"));
        // Collapsed should only show first line
        assert!(!result.contains("Second line"));
    }

    #[test]
    fn test_extract_tool_text_with_message() {
        let mut tool = make_test_tool("write_file");
        tool.message = Some("Writing to /test.txt".to_string());

        let result = extract_tool_text(&tool);
        assert!(result.contains("write_file"));
        assert!(result.contains("Writing to /test.txt"));
    }

    #[test]
    fn test_extract_tool_text_long_parameter_truncated() {
        let long_value = "x".repeat(200);
        let mut tool = make_test_tool("test");
        tool.parameters.insert("content".to_string(), long_value);

        let result = extract_tool_text(&tool);
        assert!(result.contains("content: "));
        assert!(result.contains("...")); // Should be truncated
    }

    // ========== extract_agent_footer_text tests ==========

    fn make_test_agent_block(state: AgentBlockState) -> AgentBlock {
        AgentBlock {
            id: BlockId(1),
            query: "test".to_string(),
            thinking: String::new(),
            thinking_collapsed: true,
            response: String::new(),
            tools: vec![],
            active_tool_id: None,
            images: vec![],
            state,
            started_at: std::time::Instant::now(),
            pending_permission: None,
            pending_question: None,
            duration_ms: None,
            cost_usd: None,
            input_tokens: None,
            output_tokens: None,
            version: 0,
        }
    }

    #[test]
    fn test_extract_agent_footer_text_pending() {
        let block = make_test_agent_block(AgentBlockState::Pending);
        let result = extract_agent_footer_text(&block);
        assert_eq!(result, "Waiting...");
    }

    #[test]
    fn test_extract_agent_footer_text_completed_with_stats() {
        let mut block = make_test_agent_block(AgentBlockState::Completed);
        block.response = "Done".to_string();
        block.duration_ms = Some(1500);
        block.cost_usd = Some(0.0023);
        block.input_tokens = Some(100);
        block.output_tokens = Some(50);

        let result = extract_agent_footer_text(&block);
        assert!(result.contains("Completed"));
        assert!(result.contains("1.5s"));
        assert!(result.contains("$0.0023"));
        assert!(result.contains("150 tokens"));
    }

    #[test]
    fn test_extract_agent_footer_text_duration_ms() {
        let mut block = make_test_agent_block(AgentBlockState::Completed);
        block.duration_ms = Some(500);

        let result = extract_agent_footer_text(&block);
        assert!(result.contains("500ms"));
    }

    #[test]
    fn test_extract_agent_footer_text_large_tokens() {
        let mut block = make_test_agent_block(AgentBlockState::Completed);
        block.input_tokens = Some(500_000);
        block.output_tokens = Some(600_000);

        let result = extract_agent_footer_text(&block);
        assert!(result.contains("1.1M tokens"));
    }

    #[test]
    fn test_extract_agent_footer_text_k_tokens() {
        let mut block = make_test_agent_block(AgentBlockState::Completed);
        block.input_tokens = Some(1500);
        block.output_tokens = Some(500);

        let result = extract_agent_footer_text(&block);
        assert!(result.contains("2.0k tokens"));
    }

    #[test]
    fn test_extract_agent_footer_text_failed() {
        let block = make_test_agent_block(AgentBlockState::Failed("Connection error".to_string()));
        let result = extract_agent_footer_text(&block);
        assert!(result.contains("Connection error"));
    }
}
