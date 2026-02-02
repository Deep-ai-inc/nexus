//! Nexus Widget Structs for Strata
//!
//! Production UI components that render real Nexus data (Block, AgentBlock, etc.)
//! using Strata's layout primitives. Each widget takes references to backend
//! data models and builds a layout tree.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;

use nexus_api::{BlockId, BlockState, FileEntry, FileType, Value, format_value_for_display};
use nexus_kernel::{Completion, CompletionKind};

use similar::{ChangeTag, TextDiff};

use crate::agent_block::{AgentBlock, AgentBlockState, ToolInvocation, ToolStatus};
use crate::blocks::Block;
use strata::content_address::SourceId;
use crate::nexus_app::drag_state::DragPayload;
use crate::nexus_app::shell::{AnchorEntry, value_to_anchor_action, semantic_text_for_value};
use crate::nexus_app::source_ids;
use strata::gpu::ImageHandle;
use strata::layout::containers::{
    ButtonElement, Column, CrossAxisAlignment, ImageElement, LayoutChild, Length, Padding, Row,
    ScrollColumn, TableCell, TableElement, TerminalElement, TextElement, Widget,
};
use strata::layout_snapshot::CursorIcon;
use strata::primitives::Color;
use strata::scroll_state::ScrollState;
use crate::blocks::{VisualJob, VisualJobState};

use crate::nexus_app::colors;

// =========================================================================
// Shell Block Widget — renders a real Block with TerminalParser data
// =========================================================================

pub struct ShellBlockWidget<'a> {
    pub block: &'a Block,
    pub kill_id: SourceId,
    pub image_info: Option<(ImageHandle, u32, u32)>,
    pub is_focused: bool,
    /// Shared anchor registry — populated during rendering so click/drag
    /// handling can do O(1) lookups without re-iterating the Value tree.
    pub(crate) anchor_registry: &'a RefCell<HashMap<SourceId, AnchorEntry>>,
    /// Tree expand registry — maps SourceId to (BlockId, PathBuf) for chevron clicks.
    pub(crate) tree_expand_registry: &'a RefCell<HashMap<SourceId, (BlockId, PathBuf)>>,
}

impl Widget for ShellBlockWidget<'_> {
    fn build(self) -> LayoutChild {
        let block = self.block;

        // Status icon and color
        let (status_icon, status_color) = match block.state {
            BlockState::Running => ("\u{25CF}", colors::RUNNING),    // ●
            BlockState::Success => ("\u{2713}", colors::SUCCESS),    // ✓
            BlockState::Failed(_) => ("\u{2717}", colors::ERROR),    // ✗
            BlockState::Killed(_) => ("\u{2717}", colors::ERROR),   // ✗
        };

        // Header row: status + command + [Kill/duration]
        let header_source = source_ids::shell_header(block.id);
        let mut header = Row::new()
            .spacing(8.0)
            .cross_align(CrossAxisAlignment::Center)
            .push(
                TextElement::new(format!("{} $ {}", status_icon, block.command))
                    .color(status_color)
                    .source(header_source),
            )
            .spacer(1.0);

        if block.is_running() {
            // Kill button
            header = header.push(
                ButtonElement::new(self.kill_id, "Kill")
                    .background(colors::BTN_KILL)
                    .corner_radius(4.0),
            );
        } else if let Some(ms) = block.duration_ms {
            let duration = if ms < 1000 {
                format!("{}ms", ms)
            } else {
                format!("{:.1}s", ms as f64 / 1000.0)
            };
            header = header.push(TextElement::new(duration).color(colors::TEXT_MUTED));
        }

        // Extract terminal content from parser
        let grid = if block.parser.is_alternate_screen() || block.is_running() {
            block.parser.grid()
        } else {
            block.parser.grid_with_scrollback()
        };
        let content_rows = grid.content_rows();
        let cols = grid.cols();

        let mut content = Column::new()
            .padding(6.0)
            .spacing(4.0)
            .background(colors::BG_BLOCK)
            .corner_radius(4.0)
            .width(Length::Fill);

        if self.is_focused {
            content = content.border(Color::rgb(0.3, 0.7, 1.0), 2.0);
        }

        content = content.push(header);

        // Render output: native structured data takes priority over terminal
        if let Some(value) = &block.native_output {
            content = render_native_value(content, value, block, self.image_info, self.anchor_registry, self.tree_expand_registry);
        } else if content_rows > 0 {
            let source_id = source_ids::shell_term(block.id);
            let mut term = TerminalElement::new(source_id, cols, content_rows)
                .cell_size(8.4, 18.0);

            // Extract text rows from the grid
            for row in grid.rows_iter() {
                let text: String = row.iter().map(|cell| cell.c).collect();
                // Use cell foreground color from first non-default cell, or default
                let fg = row.iter()
                    .find(|c| !matches!(c.fg, nexus_term::Color::Default))
                    .map(|c| term_color_to_strata(c.fg))
                    .unwrap_or(Color::rgb(0.8, 0.8, 0.8));
                term = term.row(&text, fg);
            }

            content = content.terminal(term);
        }

        // Exit code indicator for failed commands
        match block.state {
            BlockState::Failed(code) | BlockState::Killed(code) => {
                content = content.push(
                    TextElement::new(format!("exit {}", code)).color(colors::ERROR)
                        .source(header_source),
                );
            }
            _ => {}
        }

        content.into()
    }
}

// =========================================================================
// Agent Block Widget — renders a real AgentBlock
// =========================================================================

pub struct AgentBlockWidget<'a> {
    pub block: &'a AgentBlock,
    pub thinking_toggle_id: SourceId,
    pub stop_id: SourceId,
    /// Text input state for free-form question answers (only set when question is pending).
    pub question_input: Option<&'a strata::TextInputState>,
}

impl<'a> AgentBlockWidget<'a> {
    /// Generate a stable SourceId for a tool toggle button.
    fn tool_toggle_id(block_id: BlockId, tool_index: usize) -> SourceId {
        source_ids::agent_tool_toggle(block_id, tool_index)
    }

    /// Generate a stable SourceId for permission buttons.
    fn perm_deny_id(block_id: BlockId) -> SourceId {
        source_ids::agent_perm_deny(block_id)
    }
    fn perm_allow_id(block_id: BlockId) -> SourceId {
        source_ids::agent_perm_allow(block_id)
    }
    fn perm_always_id(block_id: BlockId) -> SourceId {
        source_ids::agent_perm_always(block_id)
    }
}

impl Widget for AgentBlockWidget<'_> {
    fn build(self) -> LayoutChild {
        let block = self.block;

        let mut content = Column::new()
            .padding(6.0)
            .spacing(3.0)
            .background(colors::BG_BLOCK)
            .corner_radius(4.0)
            .width(Length::Fill);

        // Query line
        let query_source = source_ids::agent_query(block.id);
        content = content.push(
            Row::new()
                .spacing(4.0)
                .push(TextElement::new("? ").color(colors::TEXT_PURPLE).source(query_source))
                .push(TextElement::new(&block.query).color(colors::TEXT_QUERY).source(query_source)),
        );

        // Thinking section
        if !block.thinking.is_empty() {
            let collapse_icon = if block.thinking_collapsed { "\u{25B6}" } else { "\u{25BC}" };
            content = content.push(
                ButtonElement::new(self.thinking_toggle_id, &format!("{} Thinking...", collapse_icon))
                    .background(Color::TRANSPARENT)
                    .text_color(colors::TEXT_MUTED)
                    .corner_radius(2.0),
            );

            if !block.thinking_collapsed {
                // Show thinking text indented
                let thinking_source = source_ids::agent_thinking(block.id);
                let thinking_preview = if block.thinking.len() > 500 {
                    format!("{}...", &block.thinking[..500])
                } else {
                    block.thinking.clone()
                };
                for line in thinking_preview.lines() {
                    content = content.push(
                        Row::new()
                            .fixed_spacer(16.0)
                            .push(TextElement::new(line).color(colors::THINKING).source(thinking_source)),
                    );
                }
            }
        }

        // Tool invocations
        for (i, tool) in block.tools.iter().enumerate() {
            let toggle_id = Self::tool_toggle_id(block.id, i);
            let tool_source = source_ids::agent_tool(block.id, i);
            content = content.push(build_tool_widget(tool, toggle_id, tool_source));
        }

        // Permission dialog
        if let Some(ref perm) = block.pending_permission {
            let perm_source = source_ids::agent_perm_text(block.id);
            content = content.push(build_permission_dialog(
                perm,
                Self::perm_deny_id(block.id),
                Self::perm_allow_id(block.id),
                Self::perm_always_id(block.id),
                perm_source,
            ));
        }

        // User question dialog (AskUserQuestion via MCP permission)
        if let Some(ref question) = block.pending_question {
            let q_source = source_ids::agent_question_text(block.id);
            content = content.push(build_question_dialog(question, block.id, self.question_input, q_source));
        }

        // Response text
        if !block.response.is_empty() {
            let response_source = source_ids::agent_response(block.id);
            content = content.push(build_response_text(&block.response, response_source));
        }

        // Status footer
        let (status_text, status_color) = match &block.state {
            AgentBlockState::Pending => ("Waiting...", colors::TEXT_MUTED),
            AgentBlockState::Streaming => ("Streaming...", colors::RUNNING),
            AgentBlockState::Thinking => ("Thinking...", colors::THINKING),
            AgentBlockState::Executing => ("Executing...", colors::RUNNING),
            AgentBlockState::Completed => ("Completed", colors::SUCCESS),
            AgentBlockState::Failed(err) => (err.as_str(), colors::ERROR),
            AgentBlockState::AwaitingPermission => ("Awaiting permission...", colors::WARNING),
            AgentBlockState::Interrupted => ("Interrupted", colors::TEXT_MUTED),
        };

        let is_running = block.is_running();
        let footer_source = source_ids::agent_footer(block.id);
        let mut footer = Row::new()
            .spacing(8.0)
            .cross_align(CrossAxisAlignment::Center);

        if is_running {
            footer = footer.push(
                ButtonElement::new(self.stop_id, "Stop")
                    .background(Color::rgba(0.5, 0.5, 0.5, 0.3))
                    .text_color(Color::rgb(0.9, 0.5, 0.5))
                    .corner_radius(4.0),
            );
        }

        footer = footer.push(TextElement::new(status_text).color(status_color).source(footer_source));

        if let Some(ms) = block.duration_ms {
            let duration = if ms < 1000 {
                format!("{}ms", ms)
            } else {
                format!("{:.1}s", ms as f64 / 1000.0)
            };
            footer = footer.push(TextElement::new(&duration).color(colors::TEXT_MUTED).source(footer_source));
        }

        if let Some(cost) = block.cost_usd {
            footer = footer.push(
                TextElement::new(&format!("${:.4}", cost)).color(colors::TEXT_MUTED).source(footer_source),
            );
        }

        let total_tokens = block.input_tokens.unwrap_or(0) + block.output_tokens.unwrap_or(0);
        if total_tokens > 0 {
            footer = footer.push(
                TextElement::new(&format!("\u{2193} {}", format_tokens(total_tokens)))
                    .color(colors::TEXT_MUTED).source(footer_source),
            );
        }

        content = content.fixed_spacer(4.0).push(footer);

        content.into()
    }
}

/// Format a token count with K/M suffixes.
fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M tokens", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k tokens", n as f64 / 1_000.0)
    } else {
        format!("{} tokens", n)
    }
}

/// Shorten a file path for display (keep last 2 components).
fn shorten_path(path: &str) -> String {
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 2 {
        path.to_string()
    } else {
        format!("\u{2026}/{}", parts[parts.len() - 2..].join("/"))
    }
}

/// Truncate a string with ellipsis.
fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}\u{2026}", &s[..max])
    }
}

/// Format a tool-specific header label based on tool name and parameters.
fn tool_header_label(tool: &ToolInvocation) -> String {
    match tool.name.as_str() {
        "Read" => {
            let path = tool.parameters.get("file_path")
                .map(|p| shorten_path(p))
                .unwrap_or_default();
            format!("Read({})", path)
        }
        "Edit" => {
            let path = tool.parameters.get("file_path")
                .map(|p| shorten_path(p))
                .unwrap_or_default();
            format!("Update({})", path)
        }
        "Write" => {
            let path = tool.parameters.get("file_path")
                .map(|p| shorten_path(p))
                .unwrap_or_default();
            format!("Write({})", path)
        }
        "Bash" => {
            let cmd = tool.parameters.get("command")
                .map(|c| truncate_str(c.lines().next().unwrap_or(c), 80))
                .unwrap_or_default();
            format!("Bash({})", cmd)
        }
        "Grep" => {
            let pattern = tool.parameters.get("pattern").cloned().unwrap_or_default();
            let path = tool.parameters.get("path")
                .map(|p| shorten_path(p))
                .unwrap_or_else(|| ".".to_string());
            format!("Search(\"{}\", {})", truncate_str(&pattern, 30), path)
        }
        "Glob" => {
            let pattern = tool.parameters.get("pattern").cloned().unwrap_or_default();
            format!("Glob({})", truncate_str(&pattern, 50))
        }
        "Task" => {
            let desc = tool.parameters.get("description")
                .map(|d| truncate_str(d, 60))
                .unwrap_or_default();
            format!("Task({})", desc)
        }
        "TodoWrite" => "TodoWrite".to_string(),
        other => other.to_string(),
    }
}

/// Generate a smart summary for collapsed tool output.
fn tool_collapsed_summary(tool: &ToolInvocation) -> Option<String> {
    let output = tool.output.as_deref()?;

    match tool.name.as_str() {
        "Read" => {
            let lines = output.lines().count();
            Some(format!("Read {} lines", lines))
        }
        "Edit" => Some("Applied edits".to_string()),
        "Write" => {
            let lines = tool.parameters.get("content")
                .map(|c| c.lines().count())
                .unwrap_or(0);
            Some(format!("Wrote {} lines", lines))
        }
        "Bash" => {
            let lines = output.lines().count();
            if lines == 0 {
                Some("(no output)".to_string())
            } else if lines == 1 {
                Some(truncate_str(output.trim(), 80))
            } else {
                Some(format!("{} lines", lines))
            }
        }
        "Grep" => {
            let lines = output.lines().count();
            Some(format!("Found {} results", lines))
        }
        "Glob" => {
            let files = output.lines().count();
            Some(format!("Found {} files", files))
        }
        "Task" => {
            let desc = tool.parameters.get("description")
                .map(|d| truncate_str(d, 40))
                .unwrap_or_else(|| "Done".to_string());
            let chars = output.len();
            if chars >= 1000 {
                Some(format!("{} ({:.1}k chars)", desc, chars as f64 / 1000.0))
            } else {
                Some(format!("{} ({} chars)", desc, chars))
            }
        }
        _ => {
            let lines = output.lines().count();
            if lines > 0 {
                Some(format!("{} lines", lines))
            } else {
                None
            }
        }
    }
}

// =========================================================================
// Tool-specific expanded body rendering
// =========================================================================

/// Dispatch to tool-specific body rendering.
fn build_tool_body(tool: &ToolInvocation, source_id: SourceId) -> Column {
    match tool.name.as_str() {
        "Edit" => build_edit_tool_body(tool, source_id),
        "Read" => build_read_tool_body(tool, source_id),
        "Bash" => build_bash_tool_body(tool, source_id),
        "Grep" | "Glob" => build_search_tool_body(tool, source_id),
        "Write" => build_write_tool_body(tool, source_id),
        "Task" => build_task_tool_body(tool, source_id),
        _ => build_generic_tool_body(tool, source_id),
    }
}

/// Edit tool: show a unified diff with colored +/- lines.
fn build_edit_tool_body(tool: &ToolInvocation, source_id: SourceId) -> Column {
    let old = tool.parameters.get("old_string").map(|s| s.as_str()).unwrap_or("");
    let new = tool.parameters.get("new_string").map(|s| s.as_str()).unwrap_or("");

    let mut col = Column::new().spacing(1.0);

    if !old.is_empty() || !new.is_empty() {
        let diff = TextDiff::from_lines(old, new);
        let mut diff_col = Column::new()
            .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
            .background(colors::TOOL_ARTIFACT_BG)
            .corner_radius(4.0)
            .width(Length::Fill);

        let mut line_count = 0;
        for change in diff.iter_all_changes() {
            if line_count >= 60 { break; }
            let text = change.value().trim_end_matches('\n');
            let (prefix, text_color, bg) = match change.tag() {
                ChangeTag::Insert => ("+", colors::DIFF_ADD, Some(colors::DIFF_BG_ADD)),
                ChangeTag::Delete => ("-", colors::DIFF_REMOVE, Some(colors::DIFF_BG_REMOVE)),
                ChangeTag::Equal => (" ", colors::TEXT_MUTED, None),
            };
            let line_text = format!("{} {}", prefix, text);
            let mut row = Row::new().width(Length::Fill);
            if let Some(bg_color) = bg {
                row = row.background(bg_color);
            }
            row = row.push(TextElement::new(&line_text).color(text_color).source(source_id));
            diff_col = diff_col.push(row);
            line_count += 1;
        }

        col = col.push(diff_col);
    }

    // Show tool output (e.g., confirmation message) if present
    if let Some(ref output) = tool.output {
        col = col.push(
            Row::new()
                .fixed_spacer(20.0)
                .spacing(4.0)
                .push(TextElement::new("\u{2514}").color(colors::TOOL_RESULT))
                .push(TextElement::new(&truncate_str(output, 200)).color(colors::TOOL_OUTPUT).source(source_id)),
        );
    }

    col
}

/// Read tool: code block with line numbers.
fn build_read_tool_body(tool: &ToolInvocation, source_id: SourceId) -> Column {
    let mut col = Column::new().spacing(1.0);
    if let Some(ref output) = tool.output {
        let mut code_col = Column::new()
            .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
            .background(colors::TOOL_ARTIFACT_BG)
            .corner_radius(4.0)
            .width(Length::Fill);
        for (i, line) in output.lines().take(50).enumerate() {
            let numbered = format!("{:4} {}", i + 1, line);
            code_col = code_col.push(TextElement::new(&numbered).color(colors::CODE_TEXT).source(source_id));
        }
        let total = output.lines().count();
        if total > 50 {
            code_col = code_col.push(
                TextElement::new(&format!("  \u{2026} ({} more lines)", total - 50))
                    .color(colors::TEXT_MUTED).source(source_id),
            );
        }
        col = col.push(code_col);
    }
    col
}

/// Bash tool: output in a code block with optional timeout display.
fn build_bash_tool_body(tool: &ToolInvocation, source_id: SourceId) -> Column {
    let mut col = Column::new().spacing(1.0);

    if let Some(timeout) = tool.parameters.get("timeout") {
        col = col.push(
            Row::new()
                .fixed_spacer(16.0)
                .push(TextElement::new(&format!("timeout: {}ms", timeout)).color(colors::TEXT_MUTED).source(source_id)),
        );
    }

    if let Some(ref output) = tool.output {
        let mut code_col = Column::new()
            .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
            .background(colors::TOOL_ARTIFACT_BG)
            .corner_radius(4.0)
            .width(Length::Fill);
        for line in output.lines().take(30) {
            code_col = code_col.push(TextElement::new(line).color(colors::TOOL_OUTPUT).source(source_id));
        }
        let total = output.lines().count();
        if total > 30 {
            code_col = code_col.push(
                TextElement::new(&format!("  \u{2026} ({} more lines)", total - 30))
                    .color(colors::TEXT_MUTED).source(source_id),
            );
        }
        col = col.push(code_col);
    }
    col
}

/// Grep/Glob tool: results list.
fn build_search_tool_body(tool: &ToolInvocation, source_id: SourceId) -> Column {
    let mut col = Column::new().spacing(1.0);
    if let Some(ref output) = tool.output {
        for line in output.lines().take(30) {
            col = col.push(
                Row::new()
                    .fixed_spacer(16.0)
                    .push(TextElement::new(line).color(colors::TOOL_PATH).source(source_id)),
            );
        }
        let total = output.lines().count();
        if total > 30 {
            col = col.push(
                Row::new()
                    .fixed_spacer(16.0)
                    .push(
                        TextElement::new(&format!("  \u{2026} ({} more results)", total - 30))
                            .color(colors::TEXT_MUTED).source(source_id),
                    ),
            );
        }
    }
    col
}

/// Write tool: show content being written in green.
fn build_write_tool_body(tool: &ToolInvocation, source_id: SourceId) -> Column {
    let mut col = Column::new().spacing(1.0);
    if let Some(content) = tool.parameters.get("content") {
        let mut code_col = Column::new()
            .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
            .background(colors::TOOL_ARTIFACT_BG)
            .corner_radius(4.0)
            .width(Length::Fill);
        for (i, line) in content.lines().take(30).enumerate() {
            let numbered = format!("{:4} {}", i + 1, line);
            code_col = code_col.push(TextElement::new(&numbered).color(colors::DIFF_ADD).source(source_id));
        }
        let total = content.lines().count();
        if total > 30 {
            code_col = code_col.push(
                TextElement::new(&format!("  \u{2026} ({} more lines)", total - 30))
                    .color(colors::TEXT_MUTED).source(source_id),
            );
        }
        col = col.push(code_col);
    }
    col
}

/// Task tool: sub-agent display with left-border threading.
fn build_task_tool_body(tool: &ToolInvocation, source_id: SourceId) -> Column {
    let mut col = Column::new().spacing(1.0);
    if let Some(ref output) = tool.output {
        // Use a Row: thin left border column + indented content
        let mut content_col = Column::new()
            .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
            .background(colors::TOOL_ARTIFACT_BG)
            .corner_radius(4.0)
            .width(Length::Fill);
        for line in output.lines().take(40) {
            content_col = content_col.push(TextElement::new(line).color(colors::TOOL_OUTPUT).source(source_id));
        }
        let total = output.lines().count();
        if total > 40 {
            content_col = content_col.push(
                TextElement::new(&format!("  \u{2026} ({} more lines)", total - 40))
                    .color(colors::TEXT_MUTED).source(source_id),
            );
        }

        // Left border line + content
        let border_line = Column::new()
            .width(Length::Fixed(2.0))
            .height(Length::Fill)
            .background(colors::TOOL_BORDER);
        col = col.push(
            Row::new()
                .fixed_spacer(16.0)
                .push(border_line)
                .fixed_spacer(8.0)
                .push(content_col),
        );
    }
    col
}

/// Generic tool: parameter dump + output (for MCP tools, TodoWrite, etc.)
fn build_generic_tool_body(tool: &ToolInvocation, source_id: SourceId) -> Column {
    let mut col = Column::new().spacing(2.0);

    // Parameters
    if !tool.parameters.is_empty() {
        for (name, value) in &tool.parameters {
            let display_value = if value.len() > 100 {
                format!("{}\u{2026}", &value[..100])
            } else {
                value.clone()
            };
            col = col.push(
                Row::new()
                    .fixed_spacer(16.0)
                    .push(TextElement::new(&format!("{}: {}", name, display_value)).color(colors::TEXT_MUTED).source(source_id)),
            );
        }
    }

    // Output
    if let Some(ref output) = tool.output {
        let display_output = if output.len() > 500 {
            format!("{}\u{2026}\n[{} more chars]", &output[..500], output.len() - 500)
        } else {
            output.clone()
        };
        for line in display_output.lines().take(20) {
            col = col.push(
                Row::new()
                    .fixed_spacer(16.0)
                    .push(TextElement::new(line).color(colors::TOOL_OUTPUT).source(source_id)),
            );
        }
    }

    col
}

/// Build a tool invocation widget.
fn build_tool_widget(tool: &ToolInvocation, toggle_id: SourceId, source_id: SourceId) -> Column {
    let (status_icon, status_color) = match tool.status {
        ToolStatus::Pending => ("\u{25CF}", colors::TOOL_PENDING),   // ●
        ToolStatus::Running => ("\u{25CF}", colors::RUNNING),        // ●
        ToolStatus::Success => ("\u{25CF}", colors::TOOL_ACTION),    // ●
        ToolStatus::Error   => ("\u{25CF}", colors::ERROR),          // ●
    };

    let collapse_icon = if tool.collapsed { "\u{25B6} " } else { "\u{25BC} " };
    let header_label = tool_header_label(tool);

    let mut header = Row::new()
        .id(toggle_id)
        .spacing(4.0)
        .cross_align(CrossAxisAlignment::Center)
        .push(TextElement::new(collapse_icon).color(colors::TEXT_MUTED))
        .push(TextElement::new(status_icon).color(status_color))
        .push(TextElement::new(&header_label).color(colors::TOOL_ACTION).source(source_id));

    if let Some(ref msg) = tool.message {
        header = header.push(TextElement::new(msg).color(colors::TEXT_MUTED).source(source_id));
    }

    let mut col = Column::new().spacing(2.0);
    col = col.push(header);

    // Collapsed summary line
    if tool.collapsed {
        if let Some(summary) = tool_collapsed_summary(tool) {
            col = col.push(
                Row::new()
                    .fixed_spacer(20.0)
                    .spacing(4.0)
                    .push(TextElement::new("\u{2514}").color(colors::TOOL_RESULT))
                    .push(TextElement::new(&summary).color(colors::TOOL_SUMMARY).source(source_id)),
            );
        }
    }

    if !tool.collapsed {
        col = col.push(build_tool_body(tool, source_id));
    }

    col
}

/// Build a permission dialog widget.
fn build_permission_dialog(
    perm: &crate::agent_block::PermissionRequest,
    deny_id: SourceId,
    allow_id: SourceId,
    always_id: SourceId,
    source_id: SourceId,
) -> Column {
    let code_block = Column::new()
        .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
        .background(Color::rgba(0.0, 0.0, 0.0, 0.3))
        .corner_radius(4.0)
        .push(TextElement::new(&perm.action).color(colors::WARNING).source(source_id));

    // Permission dialog colors from agent_widgets.rs
    let mut dialog = Column::new()
        .padding(8.0)
        .spacing(4.0)
        .background(Color::rgb(0.15, 0.1, 0.05))
        .corner_radius(8.0)
        .border(Color::rgb(0.8, 0.5, 0.2), 1.0)
        .width(Length::Fill)
        .push(TextElement::new("\u{26A0} Permission Required").color(colors::WARNING).source(source_id))
        .push(TextElement::new(&perm.description).color(colors::TEXT_SECONDARY).source(source_id))
        .push(code_block);

    if let Some(ref dir) = perm.working_dir {
        dialog = dialog.push(TextElement::new(format!("in {}", dir)).color(colors::TEXT_MUTED).source(source_id));
    }

    dialog = dialog.push(
        Row::new()
            .spacing(8.0)
            .push(
                ButtonElement::new(deny_id, "Deny")
                    .background(colors::BTN_DENY)
                    .corner_radius(4.0),
            )
            .push(
                ButtonElement::new(allow_id, "Allow Once")
                    .background(colors::BTN_ALLOW)
                    .corner_radius(4.0),
            )
            .push(
                ButtonElement::new(always_id, "Allow Always")
                    .background(colors::BTN_ALWAYS)
                    .corner_radius(4.0),
            ),
    );

    dialog
}

/// Build a question dialog for AskUserQuestion (via MCP permission).
fn build_question_dialog(
    question: &crate::agent_block::PendingUserQuestion,
    block_id: BlockId,
    question_input: Option<&strata::TextInputState>,
    source_id: SourceId,
) -> Column {
    let mut dialog = Column::new()
        .padding(8.0)
        .spacing(6.0)
        .background(Color::rgb(0.05, 0.08, 0.15))
        .corner_radius(8.0)
        .border(Color::rgb(0.2, 0.5, 0.8), 1.0)
        .width(Length::Fill)
        .push(TextElement::new("\u{2753} Claude is asking:").color(colors::TOOL_ACTION).source(source_id));

    for (q_idx, q) in question.questions.iter().enumerate() {
        dialog = dialog.push(
            TextElement::new(&q.question).color(colors::TEXT_PRIMARY).source(source_id)
        );

        let mut row = Row::new().spacing(8.0);
        for (o_idx, opt) in q.options.iter().enumerate() {
            let id = source_ids::agent_question_option(block_id, q_idx, o_idx);
            row = row.push(
                ButtonElement::new(id, &opt.label)
                    .background(Color::rgb(0.12, 0.25, 0.45))
                    .corner_radius(4.0),
            );
        }
        dialog = dialog.push(row);
    }

    // Free-form text input (the "Other" option)
    if let Some(input) = question_input {
        let has_options = question.questions.iter().any(|q| !q.options.is_empty());
        let label = if has_options {
            "Or type a custom answer:"
        } else {
            "Type your answer:"
        };
        dialog = dialog.push(
            TextElement::new(label).color(colors::TEXT_SECONDARY).source(source_id)
        );
        let submit_id = source_ids::agent_question_submit(block_id);
        dialog = dialog.push(
            Row::new().spacing(8.0).width(Length::Fill)
                .push(
                    strata::layout::containers::TextInputElement::from_state(input)
                        .placeholder("Type your answer and press Enter...")
                        .background(Color::rgb(0.08, 0.08, 0.12))
                        .border_color(Color::rgb(0.3, 0.3, 0.4))
                        .width(Length::Fill)
                )
                .push(
                    ButtonElement::new(submit_id, "Submit")
                        .background(Color::rgb(0.12, 0.25, 0.45))
                        .corner_radius(4.0)
                )
        );
    }

    dialog
}

/// Build response text with basic markdown rendering.
fn build_response_text(response: &str, source_id: SourceId) -> Column {
    let mut col = Column::new().spacing(2.0);

    let mut in_code_block = false;
    let mut code_lines: Vec<String> = Vec::new();

    for line in response.lines() {
        if line.starts_with("```") {
            if in_code_block {
                // End code block
                let code_col = Column::new()
                    .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
                    .background(colors::CODE_BG)
                    .corner_radius(4.0)
                    .width(Length::Fill);
                let mut code_inner = code_col;
                for code_line in code_lines.drain(..) {
                    code_inner = code_inner.push(TextElement::new(code_line).color(colors::CODE_TEXT).source(source_id));
                }
                col = col.push(code_inner);
                in_code_block = false;
            } else {
                in_code_block = true;
            }
        } else if in_code_block {
            code_lines.push(line.to_string());
        } else if line.starts_with("# ") {
            col = col.push(TextElement::new(&line[2..]).color(colors::TEXT_PRIMARY).size(16.0).source(source_id));
        } else if line.starts_with("## ") {
            col = col.push(TextElement::new(&line[3..]).color(colors::TEXT_PRIMARY).size(15.0).source(source_id));
        } else if line.starts_with("**") && line.ends_with("**") && line.len() > 4 {
            col = col.push(TextElement::new(&line[2..line.len()-2]).color(colors::TEXT_PRIMARY).source(source_id));
        } else if line.starts_with("- ") || line.starts_with("* ") {
            col = col.push(
                Row::new()
                    .push(TextElement::new("  \u{00B7} ").color(colors::TEXT_MUTED).source(source_id))
                    .push(TextElement::new(&line[2..]).color(colors::TEXT_PRIMARY).source(source_id)),
            );
        } else if line.is_empty() {
            col = col.fixed_spacer(4.0);
        } else {
            col = col.push(TextElement::new(line).color(colors::TEXT_PRIMARY).source(source_id));
        }
    }

    // Handle unclosed code block
    if in_code_block && !code_lines.is_empty() {
        let code_col = Column::new()
            .padding_custom(Padding::new(6.0, 12.0, 6.0, 12.0))
            .background(colors::CODE_BG)
            .corner_radius(4.0)
            .width(Length::Fill);
        let mut code_inner = code_col;
        for code_line in code_lines {
            code_inner = code_inner.push(TextElement::new(code_line).color(colors::CODE_TEXT).source(source_id));
        }
        col = col.push(code_inner);
    }

    col
}

// =========================================================================
// Welcome Screen — shown when no blocks exist
// =========================================================================

pub struct WelcomeScreen<'a> {
    pub cwd: &'a str,
}

impl Widget for WelcomeScreen<'_> {
    fn build(self) -> LayoutChild {
        // Shorten home directory
        let home = std::env::var("HOME").unwrap_or_default();
        let display_cwd = if self.cwd.starts_with(&home) {
            self.cwd.replacen(&home, "~", 1)
        } else {
            self.cwd.to_string()
        };

        let logo = r#" ███╗   ██╗███████╗██╗  ██╗██╗   ██╗███████╗
 ████╗  ██║██╔════╝╚██╗██╔╝██║   ██║██╔════╝
 ██╔██╗ ██║█████╗   ╚███╔╝ ██║   ██║███████╗
 ██║╚██╗██║██╔══╝   ██╔██╗ ██║   ██║╚════██║
 ██║ ╚████║███████╗██╔╝ ██╗╚██████╔╝███████║
 ╚═╝  ╚═══╝╚══════╝╚═╝  ╚═╝ ╚═════╝ ╚══════╝"#;

        // Left column: logo + welcome
        let mut logo_col = Column::new().spacing(0.0);
        for line in logo.lines() {
            logo_col = logo_col.push(TextElement::new(line).color(colors::WELCOME_TITLE));
        }

        let left = Column::new()
            .spacing(4.0)
            .width(Length::Fill)
            .push(logo_col)
            .fixed_spacer(8.0)
            .push(
                Row::new()
                    .spacing(8.0)
                    .push(TextElement::new("Welcome to Nexus Shell").color(colors::WELCOME_TITLE).size(16.0))
                    .push(TextElement::new("v0.1.0").color(colors::TEXT_MUTED)),
            )
            .fixed_spacer(4.0)
            .push(TextElement::new(format!("  {}", display_cwd)).color(colors::TEXT_PATH));

        // Tips card
        let tips = Column::new()
            .padding(8.0)
            .spacing(2.0)
            .background(colors::CARD_BG)
            .corner_radius(4.0)
            .border(colors::CARD_BORDER, 1.0)
            .width(Length::Fill)
            .push(TextElement::new("Getting Started").color(colors::WELCOME_HEADING))
            .fixed_spacer(8.0)
            .push(TextElement::new("\u{2022} Type any command and press Enter").color(colors::TEXT_SECONDARY))
            .push(TextElement::new("\u{2022} Use Tab for completions").color(colors::TEXT_SECONDARY))
            .fixed_spacer(8.0)
            .push(TextElement::new("\u{2022} Click [SH] to switch to AI mode").color(colors::TEXT_PURPLE))
            .push(TextElement::new("\u{2022} Prefix with \"? \" for one-shot AI queries").color(colors::TEXT_PURPLE))
            .fixed_spacer(8.0)
            .push(TextElement::new("Try: ? what files are in this directory?").color(colors::TEXT_PURPLE));

        // Shortcuts card
        let shortcuts = Column::new()
            .padding(8.0)
            .spacing(2.0)
            .background(colors::CARD_BG)
            .corner_radius(4.0)
            .border(colors::CARD_BORDER, 1.0)
            .width(Length::Fill)
            .push(TextElement::new("Shortcuts").color(colors::WELCOME_HEADING))
            .fixed_spacer(8.0)
            .push(TextElement::new("Cmd+K     Clear screen").color(colors::TEXT_SECONDARY))
            .push(TextElement::new("Cmd++/-   Zoom in/out").color(colors::TEXT_SECONDARY))
            .push(TextElement::new("Ctrl+R    Search history").color(colors::TEXT_SECONDARY))
            .push(TextElement::new("Up/Down   Navigate history").color(colors::TEXT_SECONDARY));

        // Right column: tips + shortcuts
        let right = Column::new()
            .spacing(12.0)
            .width(Length::Fill)
            .push(tips)
            .push(shortcuts);

        Row::new()
            .padding(12.0)
            .spacing(20.0)
            .width(Length::Fill)
            .push(left)
            .push(right)
            .into()
    }
}

// =========================================================================
// Job Bar — shows background job pills
// =========================================================================

pub struct JobBar<'a> {
    pub jobs: &'a [VisualJob],
}

impl JobBar<'_> {
    pub fn job_pill_id(job_id: u32) -> SourceId {
        SourceId::named(&format!("job_{}", job_id))
    }
}

impl Widget for JobBar<'_> {
    fn build(self) -> LayoutChild {
        let mut row = Row::new().spacing(8.0);

        for job in self.jobs {
            let (icon, color, bg) = match job.state {
                VisualJobState::Running => ("\u{25CF}", Color::rgb(0.3, 0.8, 0.3), Color::rgba(0.2, 0.4, 0.2, 0.6)),
                VisualJobState::Stopped => ("\u{23F8}", Color::rgb(0.9, 0.7, 0.2), Color::rgba(0.4, 0.35, 0.1, 0.6)),
            };
            let name = job.display_name();
            let click_id = Self::job_pill_id(job.id);
            row = row.push(
                Row::new()
                    .id(click_id)
                    .padding_custom(Padding::new(2.0, 6.0, 2.0, 6.0))
                    .background(bg)
                    .corner_radius(12.0)
                    .border(Color::rgba(0.5, 0.5, 0.5, 0.3), 1.0)
                    .push(TextElement::new(format!("{} {}", icon, name)).color(color)),
            );
        }

        Row::new()
            .padding_custom(Padding::new(2.0, 4.0, 2.0, 4.0))
            .width(Length::Fill)
            .push(Row::new().spacer(1.0).push(row))
            .into()
    }
}

// =========================================================================
// Input Bar — mode toggle + path + prompt + text input
// =========================================================================

pub struct NexusInputBar<'a> {
    pub input: &'a strata::TextInputState,
    pub mode: crate::blocks::InputMode,
    pub cwd: &'a str,
    pub last_exit_code: Option<i32>,
    pub cursor_visible: bool,
    pub mode_toggle_id: SourceId,
    pub line_count: usize,
}

impl Widget for NexusInputBar<'_> {
    fn build(self) -> LayoutChild {
        use crate::blocks::InputMode;
        use strata::TextInputElement;

        // Mode button
        let (mode_label, mode_color, mode_bg, prompt_char) = match self.mode {
            InputMode::Shell => ("SH", Color::rgb(0.5, 0.9, 0.5), Color::rgb(0.2, 0.3, 0.2), "$"),
            InputMode::Agent => ("AI", Color::rgb(0.7, 0.7, 1.0), Color::rgb(0.25, 0.25, 0.4), "?"),
        };

        let mode_btn = ButtonElement::new(self.mode_toggle_id, mode_label)
            .background(mode_bg)
            .text_color(mode_color)
            .corner_radius(4.0);

        // Shorten cwd for display
        let home = std::env::var("HOME").unwrap_or_default();
        let display_cwd = if self.cwd.starts_with(&home) {
            self.cwd.replacen(&home, "~", 1)
        } else {
            self.cwd.to_string()
        };

        // Prompt color based on exit code (rgb8 values from input.rs)
        let prompt_color = match self.last_exit_code {
            // rgb8(50, 205, 50) = lime green
            Some(0) | None => Color::rgb(0.196, 0.804, 0.196),
            // rgb8(220, 50, 50) = bright red
            Some(_) => Color::rgb(0.863, 0.196, 0.196),
        };

        Row::new()
            .padding_custom(Padding::new(4.0, 6.0, 4.0, 6.0))
            .spacing(6.0)
            .background(colors::BG_INPUT)
            .corner_radius(6.0)
            .border(colors::BORDER_INPUT, 1.0)
            .width(Length::Fill)
            .cross_align(CrossAxisAlignment::Center)
            .push(mode_btn)
            .push(TextElement::new(display_cwd).color(colors::TEXT_PATH))
            .push(TextElement::new(prompt_char).color(prompt_color))
            .push({
                let mut elem = TextInputElement::from_state(self.input)
                    .placeholder("Type a command...")
                    .background(Color::TRANSPARENT)
                    .border_color(Color::TRANSPARENT)
                    .focus_border_color(Color::TRANSPARENT)
                    .corner_radius(0.0)
                    .padding(Padding::new(0.0, 4.0, 0.0, 4.0))
                    .width(Length::Fill)
                    .cursor_visible(self.cursor_visible);
                if self.line_count > 1 {
                    let line_height = 18.0_f32;
                    let input_height = self.line_count as f32 * line_height + 4.0;
                    elem = elem.multiline(true).height(Length::Fixed(input_height));
                }
                elem
            })
            .into()
    }
}

// =========================================================================
// Completion Popup — shows tab completion results
// =========================================================================

pub struct CompletionPopup<'a> {
    pub completions: &'a [Completion],
    pub selected_index: Option<usize>,
    pub hovered_index: Option<usize>,
    pub scroll: &'a ScrollState,
}

impl CompletionPopup<'_> {
    /// Generate a stable SourceId for clicking a completion item.
    pub fn item_id(index: usize) -> SourceId {
        SourceId::named(&format!("comp_item_{}", index))
    }
}

impl Widget for CompletionPopup<'_> {
    fn build(self) -> LayoutChild {
        // Scrollable list of completions, max 300px tall
        let mut scroll = ScrollColumn::from_state(self.scroll)
            .spacing(0.0)
            .width(Length::Fixed(300.0))
            .height(Length::Fixed(300.0_f32.min(self.completions.len() as f32 * 26.0 + 8.0)))
            .background(Color::rgb(0.12, 0.12, 0.15))
            .corner_radius(4.0)
            .border(Color::rgb(0.3, 0.3, 0.35), 1.0);

        for (i, comp) in self.completions.iter().enumerate() {
            let is_selected = self.selected_index == Some(i);
            let is_hovered = self.hovered_index == Some(i) && !is_selected;

            // Icon from CompletionKind (matches kernel's icon() method)
            let icon = comp.kind.icon();

            // Icon colors matched from old UI input.rs completion_icon_color
            let icon_color = match comp.kind {
                CompletionKind::Directory => Color::rgb(0.4, 0.7, 1.0),
                CompletionKind::Executable | CompletionKind::NativeCommand => Color::rgb(0.4, 0.9, 0.4),
                CompletionKind::Builtin => Color::rgb(1.0, 0.8, 0.4),
                CompletionKind::Function => Color::rgb(0.8, 0.6, 1.0),
                CompletionKind::Variable => Color::rgb(1.0, 0.6, 0.6),
                _ => Color::rgb(0.7, 0.7, 0.7),
            };

            let text_color = if is_selected { Color::WHITE } else { Color::rgb(0.8, 0.8, 0.8) };
            let bg = if is_selected {
                Color::rgb(0.2, 0.4, 0.6)
            } else if is_hovered {
                Color::rgb(0.22, 0.22, 0.28)
            } else {
                Color::rgb(0.15, 0.15, 0.18)
            };

            let click_id = Self::item_id(i);
            scroll = scroll.push(
                Row::new()
                    .id(click_id)
                    .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
                    .spacing(4.0)
                    .background(bg)
                    .corner_radius(3.0)
                    .cross_align(CrossAxisAlignment::Center)
                    .push(TextElement::new(format!("{} ", icon)).color(icon_color))
                    .push(TextElement::new(&comp.display).color(text_color)),
            );
        }

        Column::new()
            .padding_custom(Padding::new(0.0, 4.0, 2.0, 4.0))
            .width(Length::Fill)
            .push(scroll)
            .into()
    }
}

// =========================================================================
// History Search Bar — Ctrl+R reverse-i-search
// =========================================================================

pub struct HistorySearchBar<'a> {
    pub query: &'a str,
    pub results: &'a [String],
    pub result_index: usize,
    pub hovered_index: Option<usize>,
    pub scroll: &'a ScrollState,
}

impl HistorySearchBar<'_> {
    /// Generate a stable SourceId for clicking a history result item.
    pub fn result_id(index: usize) -> SourceId {
        SourceId::named(&format!("hist_result_{}", index))
    }
}

impl Widget for HistorySearchBar<'_> {
    fn build(self) -> LayoutChild {
        // History search overlay matched from old UI input.rs
        let mut container = Column::new()
            .padding(10.0)
            .spacing(6.0)
            .background(Color::rgb(0.1, 0.1, 0.12))
            .corner_radius(6.0)
            .border(Color::rgb(0.3, 0.5, 0.7), 1.0)
            .width(Length::Fill);

        // Search header: label + query display
        let header = Row::new()
            .spacing(8.0)
            .cross_align(CrossAxisAlignment::Center)
            .push(TextElement::new("(reverse-i-search)").color(Color::rgb(0.6, 0.6, 0.6)))
            .push(
                // Styled query input area
                Row::new()
                    .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
                    .background(Color::rgb(0.15, 0.15, 0.18))
                    .corner_radius(4.0)
                    .border(Color::rgb(0.4, 0.6, 0.8), 1.0)
                    .width(Length::Fill)
                    .push(if self.query.is_empty() {
                        TextElement::new("Type to search...").color(Color::rgb(0.4, 0.4, 0.4))
                    } else {
                        TextElement::new(self.query).color(Color::rgb(0.9, 0.9, 0.9))
                    }),
            );

        container = container.push(header);

        // Scrollable results list, max 300px tall
        if !self.results.is_empty() {
            let row_height = 30.0_f32;
            let max_height = 300.0_f32.min(self.results.len() as f32 * row_height + 4.0);

            let mut scroll = ScrollColumn::from_state(self.scroll)
                .spacing(0.0)
                .width(Length::Fill)
                .height(Length::Fixed(max_height));

            for (i, result) in self.results.iter().enumerate() {
                let is_selected = i == self.result_index;
                let is_hovered = self.hovered_index == Some(i) && !is_selected;
                let text_color = if is_selected { Color::WHITE } else { Color::rgb(0.8, 0.8, 0.8) };
                let bg = if is_selected {
                    Color::rgb(0.2, 0.4, 0.6)
                } else if is_hovered {
                    Color::rgb(0.20, 0.20, 0.25)
                } else {
                    Color::rgb(0.12, 0.12, 0.15)
                };

                let click_id = Self::result_id(i);
                scroll = scroll.push(
                    Row::new()
                        .id(click_id)
                        .padding_custom(Padding::new(6.0, 10.0, 6.0, 10.0))
                        .background(bg)
                        .corner_radius(3.0)
                        .width(Length::Fill)
                        .push(TextElement::new(result).color(text_color)),
                );
            }

            container = container.push(scroll);
        } else if !self.query.is_empty() {
            container = container.push(
                Row::new()
                    .padding_custom(Padding::new(4.0, 10.0, 4.0, 10.0))
                    .push(TextElement::new("No matches found").color(colors::TEXT_MUTED)),
            );
        }

        // Status line
        if !self.results.is_empty() {
            let status = format!("{}/{}", self.result_index + 1, self.results.len());
            container = container.push(
                Row::new()
                    .push(TextElement::new("Esc to close, Enter to select, Ctrl+R for next").color(colors::TEXT_MUTED))
                    .spacer(1.0)
                    .push(TextElement::new(status).color(colors::TEXT_MUTED)),
            );
        }

        Column::new()
            .padding_custom(Padding::new(0.0, 4.0, 2.0, 4.0))
            .width(Length::Fill)
            .push(container)
            .into()
    }
}

// =========================================================================
// Helpers
// =========================================================================

/// Convert nexus-term color to Strata color.
fn term_color_to_strata(c: nexus_term::Color) -> Color {
    // ANSI palette matched from theme.rs ANSI_* constants
    fn ansi_color(n: u8) -> Color {
        match n {
            0  => Color::rgb(0.0, 0.0, 0.0),       // Black
            1  => Color::rgb(0.8, 0.2, 0.2),        // Red
            2  => Color::rgb(0.05, 0.74, 0.47),     // Green
            3  => Color::rgb(0.9, 0.9, 0.06),       // Yellow
            4  => Color::rgb(0.14, 0.45, 0.78),     // Blue
            5  => Color::rgb(0.74, 0.25, 0.74),     // Magenta
            6  => Color::rgb(0.07, 0.66, 0.8),      // Cyan
            7  => Color::rgb(0.9, 0.9, 0.9),        // White
            8  => Color::rgb(0.4, 0.4, 0.4),        // Bright Black
            9  => Color::rgb(0.95, 0.3, 0.3),       // Bright Red
            10 => Color::rgb(0.14, 0.82, 0.55),     // Bright Green
            11 => Color::rgb(0.96, 0.96, 0.26),     // Bright Yellow
            12 => Color::rgb(0.23, 0.56, 0.92),     // Bright Blue
            13 => Color::rgb(0.84, 0.44, 0.84),     // Bright Magenta
            14 => Color::rgb(0.16, 0.72, 0.86),     // Bright Cyan
            15 => Color::rgb(1.0, 1.0, 1.0),        // Bright White
            _ => Color::rgb(0.9, 0.9, 0.9),
        }
    }

    match c {
        nexus_term::Color::Default => Color::rgb(0.9, 0.9, 0.9),
        nexus_term::Color::Named(n) => ansi_color(n),
        nexus_term::Color::Rgb(r, g, b) => Color::rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0),
        nexus_term::Color::Indexed(n) => ansi_color(n),
    }
}

/// Render a structured Value from a native (kernel) command into the layout.
fn render_native_value(
    mut parent: Column,
    value: &Value,
    block: &Block,
    image_info: Option<(ImageHandle, u32, u32)>,
    anchor_registry: &RefCell<HashMap<SourceId, AnchorEntry>>,
    tree_expand_registry: &RefCell<HashMap<SourceId, (BlockId, PathBuf)>>,
) -> Column {
    let block_id = block.id;
    match value {
        Value::Unit => parent,

        Value::Media { content_type, metadata, .. } => {
            if content_type.starts_with("image/") {
                if let Some((handle, orig_w, orig_h)) = image_info {
                    // Scale down to fit, max 600px wide, 400px tall
                    let max_w = 600.0_f32;
                    let max_h = 400.0_f32;
                    let scale = (max_w / orig_w as f32).min(max_h / orig_h as f32).min(1.0);
                    let w = orig_w as f32 * scale;
                    let h = orig_h as f32 * scale;

                    parent = parent.image(
                        ImageElement::new(handle, w, h)
                            .corner_radius(4.0)
                            .widget_id(source_ids::image_output(block_id))
                            .cursor(CursorIcon::Grab),
                    );

                    // Label
                    let label = if let Some(ref name) = metadata.filename {
                        format!("{} ({})", name, content_type)
                    } else {
                        format!("{} {}x{}", content_type, orig_w, orig_h)
                    };
                    parent = parent.push(TextElement::new(label).color(colors::TEXT_MUTED));
                } else {
                    // Image not yet loaded
                    parent = parent.push(TextElement::new(format!("[{}: loading...]", content_type)).color(colors::TEXT_MUTED));
                }
            } else {
                // Non-image media
                let label = if let Some(ref name) = metadata.filename {
                    format!("[{}: {}]", content_type, name)
                } else {
                    format!("[{}]", content_type)
                };
                parent = parent.push(TextElement::new(label).color(colors::TEXT_MUTED));
            }
            parent
        }

        Value::Table { columns, rows } => {
            let source_id = source_ids::table(block_id);
            let mut table = TableElement::new(source_id);

            // Estimate column widths from data
            let col_widths = estimate_column_widths(columns, rows);

            // Add column headers with sort support
            for (i, col) in columns.iter().enumerate() {
                let sort_id = source_ids::table_sort(block_id, i);
                let header_name = if block.table_sort.column == Some(i) {
                    if block.table_sort.ascending {
                        format!("{} \u{25B2}", col.name) // ▲
                    } else {
                        format!("{} \u{25BC}", col.name) // ▼
                    }
                } else {
                    col.name.clone()
                };
                table = table.column_sortable(&header_name, col_widths[i], sort_id);
            }

            // Add data rows with line wrapping
            let char_w = 8.4_f32;
            let cell_padding = 16.0_f32;
            let mut anchor_idx = 0usize;
            for row in rows {
                let cells: Vec<TableCell> = row.iter().enumerate().map(|(col_idx, cell)| {
                    let text = if let Some(fmt) = columns.get(col_idx).and_then(|c| c.format) {
                        format_value_for_display(cell, fmt)
                    } else {
                        cell.to_text()
                    };
                    let col_width = col_widths.get(col_idx).copied().unwrap_or(400.0);
                    let max_chars = ((col_width - cell_padding) / char_w + 0.5).max(1.0) as usize;
                    let lines = wrap_cell_text(&text, max_chars);
                    let widget_id = if is_anchor_value(cell) {
                        let id = source_ids::anchor(block_id, anchor_idx);
                        anchor_registry.borrow_mut().insert(id, AnchorEntry {
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
                    TableCell {
                        text,
                        lines,
                        color: value_text_color(cell),
                        widget_id,
                    }
                }).collect();
                table = table.row(cells);
            }

            parent.push(table)
        }

        Value::List(items) => {
            // Check for file entries
            let file_entries: Vec<&FileEntry> = items
                .iter()
                .filter_map(|v| match v {
                    Value::FileEntry(entry) => Some(entry.as_ref()),
                    _ => None,
                })
                .collect();

            let source_id = source_ids::native(block_id);

            if file_entries.len() == items.len() && !file_entries.is_empty() {
                // Render as file list with tree expansion support
                let mut anchor_idx = 0usize;
                let mut expand_idx = 0usize;
                render_file_entries(
                    &mut parent,
                    &file_entries,
                    block,
                    0, // depth
                    &mut anchor_idx,
                    &mut expand_idx,
                    anchor_registry,
                    tree_expand_registry,
                );
                parent
            } else {
                // Generic list
                for item in items {
                    parent = parent.push(
                        TextElement::new(item.to_text()).color(colors::TEXT_PRIMARY).source(source_id),
                    );
                }
                parent
            }
        }

        Value::FileEntry(entry) => {
            let color = file_entry_color(entry);
            let display = if let Some(target) = &entry.symlink_target {
                format!("{} -> {}", entry.name, target.display())
            } else {
                entry.name.clone()
            };
            let anchor_id = source_ids::anchor(block_id, 0);
            anchor_registry.borrow_mut().insert(anchor_id, AnchorEntry {
                block_id,
                action: value_to_anchor_action(value),
                drag_payload: DragPayload::FilePath(entry.path.clone()),
            });
            parent.push(
                TextElement::new(display)
                    .color(color)
                    .widget_id(anchor_id)
                    .cursor_hint(CursorIcon::Pointer),
            )
        }

        Value::Record(fields) => {
            let source_id = source_ids::native(block_id);
            for (key, val) in fields {
                parent = parent.push(
                    Row::new()
                        .spacing(8.0)
                        .push(TextElement::new(format!("{}:", key)).color(colors::TEXT_SECONDARY).source(source_id))
                        .push(TextElement::new(val.to_text()).color(colors::TEXT_PRIMARY).source(source_id)),
                );
            }
            parent
        }

        Value::Error { message, .. } => {
            let source_id = source_ids::native(block_id);
            parent.push(TextElement::new(message).color(colors::ERROR).source(source_id))
        }

        // All other types: render as text
        _ => {
            let text = value.to_text();
            if text.is_empty() {
                parent
            } else {
                let source_id = source_ids::native(block_id);
                for line in text.lines() {
                    parent = parent.push(TextElement::new(line).color(colors::TEXT_PRIMARY).source(source_id));
                }
                parent
            }
        }
    }
}

/// Render file entries with tree expansion support.
/// Recursively renders children for expanded directories.
fn render_file_entries(
    parent: &mut Column,
    entries: &[&FileEntry],
    block: &Block,
    depth: usize,
    anchor_idx: &mut usize,
    expand_idx: &mut usize,
    anchor_registry: &RefCell<HashMap<SourceId, AnchorEntry>>,
    tree_expand_registry: &RefCell<HashMap<SourceId, (BlockId, PathBuf)>>,
) {
    let block_id = block.id;
    let indent_px = depth as f32 * 20.0;

    for entry in entries {
        let is_dir = matches!(entry.file_type, FileType::Directory);
        let is_expanded = is_dir && block.tree_state.is_expanded(&entry.path);
        let color = file_entry_color(entry);

        // Build the row: [chevron (if dir)] [name]
        let mut row = Row::new()
            .spacing(4.0)
            .cross_align(CrossAxisAlignment::Center);

        // Indentation
        if depth > 0 {
            row = row.push(TextElement::new(" ".repeat((indent_px / 8.0) as usize)));
        }

        // Expand/collapse chevron for directories
        if is_dir {
            let chevron = if is_expanded { "▼" } else { "▶" };
            let expand_id = source_ids::tree_expand(block_id, *expand_idx);
            tree_expand_registry.borrow_mut().insert(expand_id, (block_id, entry.path.clone()));
            *expand_idx += 1;

            row = row.push(
                TextElement::new(chevron)
                    .color(colors::TEXT_MUTED)
                    .widget_id(expand_id)
                    .cursor_hint(CursorIcon::Pointer),
            );
        } else {
            // Placeholder to align with directories
            row = row.push(TextElement::new("  ").color(colors::TEXT_MUTED));
        }

        // File/directory name (clickable anchor)
        let display = if let Some(target) = &entry.symlink_target {
            format!("{} -> {}", entry.name, target.display())
        } else {
            entry.name.clone()
        };

        let anchor_id = source_ids::anchor(block_id, *anchor_idx);
        let file_value = Value::FileEntry(Box::new((*entry).clone()));
        anchor_registry.borrow_mut().insert(anchor_id, AnchorEntry {
            block_id,
            action: value_to_anchor_action(&file_value),
            drag_payload: DragPayload::FilePath(entry.path.clone()),
        });
        *anchor_idx += 1;

        row = row.push(
            TextElement::new(display)
                .color(color)
                .widget_id(anchor_id)
                .cursor_hint(CursorIcon::Pointer),
        );

        *parent = std::mem::take(parent).push(row);

        // Recursively render children if expanded
        if is_expanded {
            if let Some(children) = block.tree_state.get_children(&entry.path) {
                let child_refs: Vec<&FileEntry> = children.iter().collect();
                render_file_entries(
                    parent,
                    &child_refs,
                    block,
                    depth + 1,
                    anchor_idx,
                    expand_idx,
                    anchor_registry,
                    tree_expand_registry,
                );
            } else {
                // Children not loaded yet — show loading indicator
                let mut loading_row = Row::new().spacing(4.0);
                if depth > 0 {
                    loading_row = loading_row.push(TextElement::new(" ".repeat(((depth + 1) as f32 * 20.0 / 8.0) as usize)));
                } else {
                    loading_row = loading_row.push(TextElement::new("    ")); // indent for loading
                }
                loading_row = loading_row.push(TextElement::new("Loading...").color(colors::TEXT_MUTED));
                *parent = std::mem::take(parent).push(loading_row);
            }
        }
    }
}

/// Get text color for a Value cell in a table.
fn value_text_color(value: &Value) -> Color {
    match value {
        Value::Int(_) | Value::Float(_) => Color::rgb(0.6, 0.8, 1.0),
        Value::Bool(true) => colors::SUCCESS,
        Value::Bool(false) => colors::ERROR,
        Value::Path(_) => colors::TEXT_PATH,
        Value::FileEntry(e) => file_entry_color(e),
        Value::Error { .. } => colors::ERROR,
        _ => colors::TEXT_PRIMARY,
    }
}

/// Estimate column widths based on header names and data content.
///
/// Uses the widest *line* (splitting on newlines) rather than total text length,
/// so multi-line content doesn't inflate column widths.
fn estimate_column_widths(columns: &[nexus_api::TableColumn], rows: &[Vec<Value>]) -> Vec<f32> {
    let char_w = 8.4; // approximate monospace character width
    let padding = 16.0;

    columns.iter().enumerate().map(|(i, col)| {
        let header_len = col.name.len();
        let max_data_len = rows.iter()
            .filter_map(|row| row.get(i))
            .map(|v| {
                let text = if let Some(fmt) = col.format {
                    format_value_for_display(v, fmt)
                } else {
                    v.to_text()
                };
                text.lines()
                    .map(|l| l.len())
                    .max()
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0);
        let max_len = header_len.max(max_data_len).max(4);
        (max_len as f32 * char_w + padding).min(400.0)
    }).collect()
}

/// Word-wrap text to fit within `max_chars` characters per line.
///
/// Preserves explicit newlines, breaks long lines at word boundaries,
/// and force-breaks words exceeding `max_chars`.
fn wrap_cell_text(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let mut result = Vec::new();

    for paragraph in text.split('\n') {
        if paragraph.len() <= max_chars {
            result.push(paragraph.to_string());
            continue;
        }

        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if word.len() > max_chars {
                // Force-break long words
                if !line.is_empty() {
                    result.push(line);
                    line = String::new();
                }
                let mut chars = word.chars().peekable();
                while chars.peek().is_some() {
                    let chunk: String = chars.by_ref().take(max_chars).collect();
                    result.push(chunk);
                }
                // Last chunk becomes the current line to allow appending
                if let Some(last) = result.pop() {
                    line = last;
                }
            } else if line.is_empty() {
                line = word.to_string();
            } else if line.len() + 1 + word.len() <= max_chars {
                line.push(' ');
                line.push_str(word);
            } else {
                result.push(line);
                line = word.to_string();
            }
        }
        if !line.is_empty() || paragraph.is_empty() {
            result.push(line);
        }
    }

    if result.is_empty() {
        result.push(String::new());
    }
    result
}

/// Whether a Value is anchor-worthy (clickable in the UI).
pub(crate) fn is_anchor_value(value: &Value) -> bool {
    matches!(
        value,
        Value::Path(_) | Value::FileEntry(_) | Value::Process(_) | Value::GitCommit(_)
    )
}

/// Get display color for a file entry.
fn file_entry_color(entry: &nexus_api::FileEntry) -> Color {
    match entry.file_type {
        FileType::Directory => Color::rgb(0.4, 0.6, 1.0),
        FileType::Symlink => Color::rgb(0.4, 0.9, 0.9),
        _ if entry.permissions & 0o111 != 0 => Color::rgb(0.4, 0.9, 0.4),
        _ => Color::rgb(0.8, 0.8, 0.8),
    }
}
