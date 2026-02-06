//! Nexus Widget Structs for Strata
//!
//! Production UI components that render real Nexus data (Block, AgentBlock, etc.)
//! using Strata's layout primitives. Each widget takes references to backend
//! data models and builds a layout tree.

use std::cell::RefCell;
use std::collections::HashMap;

use nexus_api::{BlockId, BlockState, FileEntry, FileType, Value, format_value_for_display};
use nexus_kernel::{Completion, CompletionKind};

use similar::{ChangeTag, TextDiff};

use crate::agent_block::{AgentBlock, AgentBlockState, ToolInvocation, ToolStatus};
use crate::blocks::Block;
use strata::content_address::SourceId;
use crate::nexus_app::drag_state::DragPayload;
use crate::nexus_app::shell::{AnchorEntry, ClickAction, register_anchor, register_tree_toggle, value_to_anchor_action, semantic_text_for_value};
use crate::nexus_app::source_ids;
use strata::gpu::ImageHandle;
use strata::layout::containers::{
    ButtonElement, Column, CrossAxisAlignment, ImageElement, LayoutChild, Length, Padding, Row,
    ScrollColumn, TerminalElement, TextElement, Widget,
};
use strata::layout_snapshot::{CursorIcon, RunStyle, TextRun, UnderlineStyle};
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
    /// Unified click registry — populated during rendering so click/drag
    /// handling can do O(1) lookups without re-iterating the Value tree.
    pub(crate) click_registry: &'a RefCell<HashMap<SourceId, ClickAction>>,
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
        } else if block.view_state.is_some() {
            // Exit button for active viewers (top, less, man, tree)
            let exit_id = source_ids::viewer_exit(block.id);
            header = header.push(
                ButtonElement::new(exit_id, "Exit")
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

        // Extract terminal content from parser.
        // Alt-screen apps (vim, htop) get viewport only; normal-screen apps
        // (including running ones like Claude Code) get full scrollback.
        let grid = if block.parser.is_alternate_screen() {
            block.parser.grid()
        } else {
            block.parser.grid_with_scrollback()
        };
        let content_rows = grid.content_rows();

        // Debounce shrink for running non-alt-screen blocks to mask
        // clear+reprint flicker (e.g. Claude Code doing \x1b[3J + \x1b[2J).
        let content_rows = if block.is_running() && !block.parser.is_alternate_screen() {
            let peak = block.peak_content_rows.load(std::sync::atomic::Ordering::Relaxed);
            if content_rows >= peak {
                block.peak_content_rows.store(content_rows, std::sync::atomic::Ordering::Relaxed);
                content_rows
            } else if content_rows < peak / 2 {
                // Dramatic shrink (clear+reprint mid-cycle): hold at peak
                peak
            } else {
                // Moderate shrink (real content reduction): follow it
                block.peak_content_rows.store(content_rows, std::sync::atomic::Ordering::Relaxed);
                content_rows
            }
        } else {
            block.peak_content_rows.store(0, std::sync::atomic::Ordering::Relaxed);
            content_rows
        };

        let cols = grid.cols();

        let mut content = Column::new()
            .id(source_ids::block_container(block.id))
            .padding(6.0)
            .spacing(4.0)
            .background(colors::BG_BLOCK)
            .corner_radius(4.0)
            .width(Length::Fill);

        if self.is_focused {
            content = content.border(Color::rgb(0.3, 0.7, 1.0), 2.0);
        }

        content = content.push(header);

        // Render output: stream_latest replaces native_output when present (e.g. top),
        // otherwise show native_output (e.g. ls, git status).
        if let Some(ref latest) = block.stream_latest {
            content = render_native_value(content, latest, block, self.image_info, self.click_registry);
        } else if let Some(value) = &block.native_output {
            content = render_native_value(content, value, block, self.image_info, self.click_registry);
        }

        // Render stream log: collapse history into a single text block for performance,
        // only render the latest entry as a full widget.
        if !block.stream_log.is_empty() {
            let source_id = source_ids::native(block.id);
            let visible_count = 50.min(block.stream_log.len());
            let start = block.stream_log.len() - visible_count;

            // History entries → single pre-rendered text element (cheap to layout)
            if visible_count > 1 {
                let mut history_text = String::new();
                for entry in block.stream_log.iter().skip(start).take(visible_count - 1) {
                    if !history_text.is_empty() {
                        history_text.push('\n');
                    }
                    history_text.push_str(&entry.to_text());
                }
                content = content.push(
                    TextElement::new(history_text)
                        .color(colors::TEXT_MUTED)
                        .source(source_id),
                );
            }

            // Latest entry → full widget rendering (may have colors, structure)
            if let Some(latest) = block.stream_log.back() {
                content = render_native_value(content, latest, block, self.image_info, self.click_registry);
            }
        }

        if block.native_output.is_none() && block.stream_latest.is_none() && block.stream_log.is_empty() && content_rows > 0 {
            let source_id = source_ids::shell_term(block.id);
            let mut term = TerminalElement::new(source_id, cols, content_rows)
                .cell_size(8.4, 18.0);

            // Extract styled text runs from the grid
            let default_fg_packed = Color::rgb(0.9, 0.9, 0.9).pack();
            let default_bg_packed: u32 = 0;
            for row in grid.rows_iter() {
                let mut runs: Vec<TextRun> = Vec::new();
                let mut run_text = String::new();
                let mut run_fg: u32 = default_fg_packed;
                let mut run_bg: u32 = default_bg_packed;
                let mut run_style = RunStyle::default();
                let mut run_col: u16 = 0;
                let mut run_cells: u16 = 0;
                let mut col: u16 = 0;

                // Flush helper: pushes current run if non-empty
                macro_rules! flush_run {
                    ($runs:expr, $text:expr, $fg:expr, $bg:expr, $col:expr, $cells:expr, $style:expr) => {
                        if !$text.is_empty() {
                            $runs.push(TextRun {
                                text: std::mem::take(&mut $text),
                                fg: $fg,
                                bg: $bg,
                                col_offset: $col,
                                cell_len: $cells,
                                style: $style,
                            });
                            #[allow(unused_assignments)]
                            { $cells = 0; }
                        }
                    };
                }

                for cell in row {
                    if cell.flags.wide_char_spacer {
                        continue;
                    }

                    // Hidden cells: flush current run and skip (creates a gap)
                    if cell.flags.hidden {
                        flush_run!(runs, run_text, run_fg, run_bg, run_col, run_cells, run_style);
                        col += if cell.flags.wide_char { 2 } else { 1 };
                        run_col = col;
                        continue;
                    }

                    let cell_width: u16 = if cell.flags.wide_char { 2 } else { 1 };

                    let (fg_packed, bg_packed) = if cell.flags.inverse {
                        let resolved_fg = if matches!(cell.fg, nexus_term::Color::Default) {
                            Color::rgb(0.9, 0.9, 0.9)
                        } else {
                            term_color_to_strata(cell.fg)
                        };
                        let resolved_bg = if matches!(cell.bg, nexus_term::Color::Default) {
                            Color::rgb(0.12, 0.12, 0.12)
                        } else {
                            term_color_to_strata(cell.bg)
                        };
                        (resolved_bg.pack(), resolved_fg.pack())
                    } else {
                        let fg = term_color_to_strata(cell.fg).pack();
                        let bg = if matches!(cell.bg, nexus_term::Color::Default) {
                            0u32
                        } else {
                            term_color_to_strata(cell.bg).pack()
                        };
                        (fg, bg)
                    };
                    let style = RunStyle {
                        bold: cell.flags.bold,
                        italic: cell.flags.italic,
                        underline: match cell.flags.underline {
                            nexus_term::UnderlineStyle::None => UnderlineStyle::None,
                            nexus_term::UnderlineStyle::Single => UnderlineStyle::Single,
                            nexus_term::UnderlineStyle::Double => UnderlineStyle::Double,
                            nexus_term::UnderlineStyle::Curly => UnderlineStyle::Curly,
                            nexus_term::UnderlineStyle::Dotted => UnderlineStyle::Dotted,
                            nexus_term::UnderlineStyle::Dashed => UnderlineStyle::Dashed,
                        },
                        strikethrough: cell.flags.strikethrough,
                        dim: cell.flags.dim,
                    };

                    // Check if this cell continues the current run (packed u32 comparison)
                    let same_attrs = fg_packed == run_fg && bg_packed == run_bg && style == run_style;

                    if !same_attrs {
                        flush_run!(runs, run_text, run_fg, run_bg, run_col, run_cells, run_style);
                        run_col = col;
                        run_fg = fg_packed;
                        run_bg = bg_packed;
                        run_style = style;
                    } else if run_text.is_empty() {
                        run_col = col;
                        run_fg = fg_packed;
                        run_bg = bg_packed;
                        run_style = style;
                    }

                    cell.push_grapheme(&mut run_text);
                    run_cells += cell_width;
                    col += cell_width;
                }

                // Flush last run
                flush_run!(runs, run_text, run_fg, run_bg, run_col, run_cells, run_style);

                term = term.row(runs);
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

        // Query line (Claude Code style: > prefix with subtle badge)
        let query_source = source_ids::agent_query(block.id);
        let query_badge = Row::new()
            .padding_custom(Padding::new(2.0, 8.0, 2.0, 8.0))
            .background(Color::rgba(1.0, 1.0, 1.0, 0.06))
            .corner_radius(4.0)
            .spacing(6.0)
            .push(TextElement::new(">").color(colors::TEXT_MUTED).source(query_source))
            .push(TextElement::new(&block.query).color(colors::TEXT_PRIMARY).source(query_source));
        content = content.push(query_badge);

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

        // Response text (Claude Code style: bullet prefix)
        if !block.response.is_empty() {
            let response_source = source_ids::agent_response(block.id);
            content = content.push(
                Row::new()
                    .spacing(6.0)
                    .cross_align(CrossAxisAlignment::Start)
                    .push(TextElement::new("\u{25CF}").color(colors::TEXT_MUTED)) // ●
                    .push(crate::markdown::render(&block.response, response_source)),
            );
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

/// Generate a smart summary for collapsed tool output (Claude Code style: +N lines).
fn tool_collapsed_summary(tool: &ToolInvocation) -> Option<String> {
    let output = tool.output.as_deref()?;

    match tool.name.as_str() {
        "Read" => {
            let lines = output.lines().count();
            Some(format!("+{} lines", lines))
        }
        "Edit" => Some("applied".to_string()),
        "Write" => {
            let lines = tool.parameters.get("content")
                .map(|c| c.lines().count())
                .unwrap_or(0);
            Some(format!("+{} lines", lines))
        }
        "Bash" => {
            let lines = output.lines().count();
            if lines == 0 {
                Some("(no output)".to_string())
            } else if lines == 1 {
                Some(truncate_str(output.trim(), 60))
            } else {
                Some(format!("+{} lines", lines))
            }
        }
        "Grep" => {
            let lines = output.lines().count();
            Some(format!("+{} results", lines))
        }
        "Glob" => {
            let files = output.lines().count();
            Some(format!("+{} files", files))
        }
        "Task" => {
            let chars = output.len();
            if chars >= 1000 {
                Some(format!("+{:.1}k chars", chars as f64 / 1000.0))
            } else {
                Some(format!("+{} chars", chars))
            }
        }
        _ => {
            let lines = output.lines().count();
            if lines > 0 {
                Some(format!("+{} lines", lines))
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
        let lines: Vec<&str> = output.lines().collect();
        let max_lines = 30;

        // First line gets tree prefix, rest are just indented (matching Claude Code)
        for (i, line) in lines.iter().take(max_lines).enumerate() {
            if i == 0 {
                col = col.push(
                    Row::new()
                        .fixed_spacer(16.0)
                        .spacing(4.0)
                        .push(TextElement::new("└").color(colors::TEXT_MUTED))
                        .push(TextElement::new(*line).color(colors::TOOL_OUTPUT).source(source_id)),
                );
            } else {
                col = col.push(
                    Row::new()
                        .fixed_spacer(28.0)
                        .push(TextElement::new(*line).color(colors::TOOL_OUTPUT).source(source_id)),
                );
            }
        }

        // Show remaining lines indicator
        if lines.len() > max_lines {
            col = col.push(
                Row::new()
                    .fixed_spacer(16.0)
                    .push(TextElement::new(format!("… ({} more lines)", lines.len() - max_lines)).color(colors::TEXT_MUTED).source(source_id)),
            );
        }
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

/// Build a tool invocation widget (Claude Code style).
fn build_tool_widget(tool: &ToolInvocation, toggle_id: SourceId, source_id: SourceId) -> Column {
    let (status_icon, status_color) = match tool.status {
        ToolStatus::Pending => ("\u{25CF}", colors::TOOL_PENDING),   // ●
        ToolStatus::Running => ("\u{25CF}", colors::RUNNING),        // ●
        ToolStatus::Success => ("\u{25CF}", colors::SUCCESS),        // ● green
        ToolStatus::Error   => ("\u{25CF}", colors::ERROR),          // ●
    };

    let header_label = tool_header_label(tool);

    // Header: just status dot + tool name (clickable to toggle)
    let mut header = Row::new()
        .id(toggle_id)
        .spacing(4.0)
        .cross_align(CrossAxisAlignment::Center)
        .push(TextElement::new(status_icon).color(status_color))
        .push(TextElement::new(&header_label).color(colors::TOOL_ACTION).source(source_id));

    if let Some(ref msg) = tool.message {
        header = header.push(TextElement::new(msg).color(colors::TEXT_MUTED).source(source_id));
    }

    let mut col = Column::new().spacing(2.0);
    col = col.push(header);

    // Collapsed: show first few lines with tree chars, then "… +N lines" summary
    if tool.collapsed {
        col = col.push(build_collapsed_preview(tool, source_id));
    } else {
        col = col.push(build_tool_body(tool, source_id));
    }

    col
}

/// Build a collapsed preview showing first few lines + summary.
fn build_collapsed_preview(tool: &ToolInvocation, source_id: SourceId) -> Column {
    let mut col = Column::new().spacing(1.0);

    let output = tool.output.as_deref().unwrap_or("");
    let lines: Vec<&str> = output.lines().collect();
    let preview_count = 2; // Show first 2 lines

    // Show first few lines with tree character prefix
    for (i, line) in lines.iter().take(preview_count).enumerate() {
        let is_last = i == preview_count - 1 && lines.len() <= preview_count;
        let prefix = if is_last { "└" } else { "├" };
        col = col.push(
            Row::new()
                .fixed_spacer(16.0)
                .spacing(4.0)
                .push(TextElement::new(prefix).color(colors::TEXT_MUTED))
                .push(TextElement::new(truncate_str(line, 80)).color(colors::TOOL_OUTPUT).source(source_id)),
        );
    }

    // Show remaining lines summary with expand hint
    let remaining = lines.len().saturating_sub(preview_count);
    if remaining > 0 {
        col = col.push(
            Row::new()
                .fixed_spacer(16.0)
                .spacing(4.0)
                .push(TextElement::new(format!("… +{} lines (ctrl+o to expand)", remaining)).color(colors::TEXT_MUTED).source(source_id)),
        );
    } else if lines.is_empty() {
        // No output - show summary from tool_collapsed_summary
        if let Some(summary) = tool_collapsed_summary(tool) {
            col = col.push(
                Row::new()
                    .fixed_spacer(16.0)
                    .spacing(4.0)
                    .push(TextElement::new(format!("… {}", summary)).color(colors::TEXT_MUTED).source(source_id)),
            );
        }
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
            // 216-color cube (indices 16-231)
            16..=231 => {
                let idx = n - 16;
                let r = (idx / 36) % 6;
                let g = (idx / 6) % 6;
                let b = idx % 6;
                let to_val = |v: u8| if v == 0 { 0.0 } else { (55.0 + v as f32 * 40.0) / 255.0 };
                Color::rgb(to_val(r), to_val(g), to_val(b))
            }
            // Grayscale (indices 232-255)
            232..=255 => {
                let gray = (8.0 + (n - 232) as f32 * 10.0) / 255.0;
                Color::rgb(gray, gray, gray)
            }
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
    click_registry: &RefCell<HashMap<SourceId, ClickAction>>,
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
            let _t0 = std::time::Instant::now();
            let source_id = source_ids::table(block_id);

            let char_w = 8.4_f32;
            let cell_padding = 16.0_f32;
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
                (max_len as f32 * char_w + cell_padding).min(400.0)
            }).collect();

            // Build VirtualTableElement — lightweight, no wrapping
            let mut table = strata::layout::containers::VirtualTableElement::new(source_id);

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

            // Build lightweight VirtualCell rows — no wrapping, no line splitting
            let mut anchor_idx = 0usize;
            for (_row_idx, row) in rows.iter().enumerate() {
                let cells: Vec<strata::layout::containers::VirtualCell> = row.iter().enumerate().map(|(col_idx, cell)| {
                    let text = if let Some(fmt) = columns.get(col_idx).and_then(|c| c.format) {
                        format_value_for_display(cell, fmt)
                    } else {
                        cell.to_text()
                    };
                    let widget_id = if is_anchor_value(cell) {
                        let id = source_ids::anchor(block_id, anchor_idx);
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
                    strata::layout::containers::VirtualCell {
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
                    click_registry,
                );
                parent
            } else {
                // Generic list — recurse for structured types, inline for simple ones
                let has_structured = items.iter().any(|v| matches!(v,
                    Value::Domain(_) |
                    Value::GitStatus(_) | Value::GitCommit(_) | Value::Record(_) |
                    Value::Table { .. }
                ));
                if has_structured {
                    for item in items {
                        parent = render_native_value(parent, item, block, None, click_registry);
                    }
                    parent
                } else {
                    for item in items {
                        parent = parent.push(
                            TextElement::new(item.to_text()).color(colors::TEXT_PRIMARY).source(source_id),
                        );
                    }
                    parent
                }
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
            register_anchor(click_registry, anchor_id, AnchorEntry {
                block_id,
                action: value_to_anchor_action(value),
                drag_payload: DragPayload::FilePath(entry.path.clone()),
            });
            let source_id = source_ids::native(block_id);
            parent.push(
                TextElement::new(display)
                    .color(color)
                    .source(source_id)
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

        Value::Domain(domain) => {
            render_domain_value(parent, domain, block, image_info, click_registry)
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

/// Render a domain-specific value (FileOp, Tree, DiffFile, etc.).
fn render_domain_value(
    mut parent: Column,
    domain: &nexus_api::DomainValue,
    block: &Block,
    image_info: Option<(ImageHandle, u32, u32)>,
    click_registry: &RefCell<HashMap<SourceId, ClickAction>>,
) -> Column {
    use nexus_api::DomainValue;
    let block_id = block.id;
    let source_id = source_ids::native(block_id);

    match domain {
        DomainValue::FileOp(info) => {
            let (icon, phase_color) = match info.phase {
                nexus_api::FileOpPhase::Planning => ("\u{1F50D}", colors::WARNING),
                nexus_api::FileOpPhase::Executing => ("\u{25B6}", colors::RUNNING),
                nexus_api::FileOpPhase::Completed => ("\u{2714}", colors::SUCCESS),
                nexus_api::FileOpPhase::Failed => ("\u{2718}", colors::ERROR),
            };
            let op_label = match info.op_type {
                nexus_api::FileOpKind::Copy => "Copy",
                nexus_api::FileOpKind::Move => "Move",
                nexus_api::FileOpKind::Remove => "Remove",
                nexus_api::FileOpKind::Chmod => "Chmod",
                nexus_api::FileOpKind::Chown => "Chown",
            };
            parent = parent.push(
                TextElement::new(format!("{} {} {:?}", icon, op_label, info.phase))
                    .color(phase_color)
                    .source(source_id),
            );
            if let Some(total) = info.total_bytes {
                if total > 0 {
                    let pct = (info.bytes_processed as f64 / total as f64 * 100.0).min(100.0);
                    let bar_len = 40;
                    let filled = (pct / 100.0 * bar_len as f64) as usize;
                    let bar: String = "\u{2588}".repeat(filled)
                        + &"\u{2591}".repeat(bar_len - filled);
                    parent = parent.push(
                        TextElement::new(format!("[{}] {:.1}%", bar, pct))
                            .color(colors::TEXT_PRIMARY)
                            .source(source_id),
                    );
                }
            } else if info.phase == nexus_api::FileOpPhase::Planning {
                parent = parent.push(
                    TextElement::new("[estimating...]".to_string())
                        .color(colors::TEXT_MUTED)
                        .source(source_id),
                );
            }
            let files_str = if let Some(total) = info.files_total {
                format!("{}/{} files", info.files_processed, total)
            } else {
                format!("{} files processed", info.files_processed)
            };
            let bytes_str = if let Some(total) = info.total_bytes {
                format!(", {}/{} bytes", info.bytes_processed, total)
            } else {
                String::new()
            };
            parent = parent.push(
                TextElement::new(format!("{}{}", files_str, bytes_str))
                    .color(colors::TEXT_SECONDARY)
                    .source(source_id),
            );
            // Throughput + ETA based on cumulative rate
            if info.phase == nexus_api::FileOpPhase::Executing && info.start_time_ms > 0 {
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                let elapsed_s = (now_ms.saturating_sub(info.start_time_ms)) as f64 / 1000.0;
                if elapsed_s > 0.1 {
                    let throughput_str = if let Some(total_bytes) = info.total_bytes {
                        if total_bytes > 0 {
                            let rate = info.bytes_processed as f64 / elapsed_s;
                            let remaining_bytes = total_bytes.saturating_sub(info.bytes_processed);
                            let eta_s = if rate > 0.0 { remaining_bytes as f64 / rate } else { 0.0 };
                            format!("  {}/s ETA: {}", nexus_api::format_size(rate as u64), format_eta(eta_s))
                        } else {
                            String::new()
                        }
                    } else if let Some(files_total) = info.files_total {
                        if files_total > 0 {
                            let rate = info.files_processed as f64 / elapsed_s;
                            let remaining = files_total.saturating_sub(info.files_processed);
                            let eta_s = if rate > 0.0 { remaining as f64 / rate } else { 0.0 };
                            format!("  {:.0} files/s ETA: {}", rate, format_eta(eta_s))
                        } else {
                            String::new()
                        }
                    } else {
                        format!("  {:.1}s elapsed", elapsed_s)
                    };
                    if !throughput_str.is_empty() {
                        parent = parent.push(
                            TextElement::new(throughput_str)
                                .color(colors::TEXT_MUTED)
                                .source(source_id),
                        );
                    }
                }
            }
            if let Some(ref current) = info.current_file {
                parent = parent.push(
                    TextElement::new(format!("  {}", current.display()))
                        .color(colors::TEXT_PATH)
                        .source(source_id),
                );
            }
            for err in &info.errors {
                parent = parent.push(
                    TextElement::new(format!("  error: {}: {}", err.path.display(), err.message))
                        .color(colors::ERROR)
                        .source(source_id),
                );
            }
            parent
        }

        DomainValue::Tree(tree) => {
            for node in &tree.nodes {
                let indent = if node.depth == 0 {
                    String::new()
                } else {
                    let prefix = "    ".repeat(node.depth.saturating_sub(1));
                    let is_last = tree.nodes.iter()
                        .filter(|n| n.parent == node.parent && n.depth == node.depth)
                        .last()
                        .map(|n| n.id == node.id)
                        .unwrap_or(true);
                    if is_last {
                        format!("{}\u{2514}\u{2500}\u{2500} ", prefix)
                    } else {
                        format!("{}\u{251C}\u{2500}\u{2500} ", prefix)
                    }
                };
                let color = match node.node_type {
                    nexus_api::FileType::Directory => colors::TEXT_PATH,
                    _ => colors::TEXT_PRIMARY,
                };
                parent = parent.push(
                    TextElement::new(format!("{}{}", indent, node.name))
                        .color(color)
                        .source(source_id),
                );
            }
            parent
        }

        DomainValue::DiffFile(diff) => {
            let stats_str = format!("+{} -{}", diff.additions, diff.deletions);
            parent = parent.push(
                Row::new()
                    .spacing(8.0)
                    .push(TextElement::new(&diff.file_path).color(colors::TEXT_PRIMARY).source(source_id))
                    .push(TextElement::new(format!("  +{}", diff.additions)).color(colors::DIFF_ADD).source(source_id))
                    .push(TextElement::new(format!("-{}", diff.deletions)).color(colors::DIFF_REMOVE).source(source_id)),
            );
            for hunk in &diff.hunks {
                parent = parent.push(
                    TextElement::new(format!("@@ -{},{} +{},{} @@ {}",
                        hunk.old_start, hunk.old_count,
                        hunk.new_start, hunk.new_count,
                        hunk.header))
                        .color(colors::TEXT_PATH)
                        .source(source_id),
                );
                for line in &hunk.lines {
                    let (prefix, color) = match line.kind {
                        nexus_api::DiffLineKind::Context => (" ", colors::TEXT_MUTED),
                        nexus_api::DiffLineKind::Addition => ("+", colors::DIFF_ADD),
                        nexus_api::DiffLineKind::Deletion => ("-", colors::DIFF_REMOVE),
                    };
                    parent = parent.push(
                        TextElement::new(format!("{}{}", prefix, line.content))
                            .color(color)
                            .source(source_id),
                    );
                }
            }
            let _ = stats_str;
            parent
        }

        DomainValue::NetEvent(evt) => {
            let (icon, color) = if evt.success {
                ("\u{2714}", colors::SUCCESS)
            } else {
                ("\u{2718}", colors::ERROR)
            };
            let ip_str = evt.ip.as_ref().map(|ip| format!(" ({})", ip)).unwrap_or_default();
            let rtt_str = evt.rtt_ms.map(|r| format!(" {:.1}ms", r)).unwrap_or_default();
            parent.push(
                TextElement::new(format!("{} {}{}{}", icon, evt.host, ip_str, rtt_str))
                    .color(color)
                    .source(source_id),
            )
        }

        DomainValue::DnsAnswer(dns) => {
            parent = parent.push(
                TextElement::new(format!(";; {} {} query", dns.query, dns.record_type))
                    .color(colors::TEXT_SECONDARY)
                    .source(source_id),
            );
            for record in &dns.answers {
                parent = parent.push(
                    TextElement::new(format!("  {} {} IN {} {}",
                        record.name, record.ttl, record.record_type, record.data))
                        .color(colors::TEXT_PRIMARY)
                        .source(source_id),
                );
            }
            parent = parent.push(
                TextElement::new(format!(";; Query time: {:.0} msec, Server: {}",
                    dns.query_time_ms, dns.server))
                    .color(colors::TEXT_MUTED)
                    .source(source_id),
            );
            parent
        }

        DomainValue::HttpResponse(resp) => {
            let status_color = if resp.status_code < 300 {
                colors::SUCCESS
            } else if resp.status_code < 400 {
                colors::WARNING
            } else {
                colors::ERROR
            };
            parent = parent.push(
                TextElement::new(format!("{} {} {} ({:.0}ms)",
                    resp.method, resp.status_code, resp.status_text, resp.timing.total_ms))
                    .color(status_color)
                    .source(source_id),
            );
            // Timing waterfall
            {
                let t = &resp.timing;
                let phases: Vec<(&str, Option<f64>, Color)> = vec![
                    ("DNS",     t.dns_ms,      Color::rgb(0.4, 0.7, 1.0)),
                    ("Connect", t.connect_ms,  Color::rgb(0.5, 0.8, 0.5)),
                    ("TLS",     t.tls_ms,      Color::rgb(0.8, 0.6, 1.0)),
                    ("TTFB",    t.ttfb_ms,     Color::rgb(1.0, 0.8, 0.3)),
                    ("Transfer",t.transfer_ms, Color::rgb(0.3, 0.9, 0.9)),
                ];
                let has_phases = phases.iter().any(|(_, v, _)| v.is_some());
                if has_phases {
                    let total = t.total_ms.max(0.001);
                    let bar_width = 40usize;
                    let mut waterfall = String::with_capacity(bar_width);
                    let mut legend_parts = Vec::new();
                    for (label, ms_opt, _color) in &phases {
                        if let Some(ms) = ms_opt {
                            let fraction = ms / total;
                            let chars = (fraction * bar_width as f64).round().max(0.0) as usize;
                            let ch = match *label {
                                "DNS" => 'D',
                                "Connect" => 'C',
                                "TLS" => 'S',
                                "TTFB" => 'W',
                                "Transfer" => 'T',
                                _ => '?',
                            };
                            for _ in 0..chars { waterfall.push(ch); }
                            legend_parts.push(format!("{}:{:.0}ms", label, ms));
                        }
                    }
                    // Pad to bar_width
                    while waterfall.len() < bar_width {
                        waterfall.push('\u{2591}');
                    }
                    parent = parent.push(
                        TextElement::new(format!("  [{}] {:.0}ms", waterfall, total))
                            .color(colors::TEXT_MUTED)
                            .source(source_id),
                    );
                    parent = parent.push(
                        TextElement::new(format!("  {}", legend_parts.join(" | ")))
                            .color(colors::TEXT_MUTED)
                            .source(source_id),
                    );
                }
            }
            for (name, value) in resp.headers.iter().take(10) {
                parent = parent.push(
                    TextElement::new(format!("  {}: {}", name, value))
                        .color(colors::TEXT_SECONDARY)
                        .source(source_id),
                );
            }
            if let Some(ref preview) = resp.body_preview {
                parent = parent.push(
                    TextElement::new("").source(source_id),
                );
                for line in preview.lines().take(20) {
                    parent = parent.push(
                        TextElement::new(line).color(colors::TEXT_PRIMARY).source(source_id),
                    );
                }
                if resp.body_truncated {
                    parent = parent.push(
                        TextElement::new(format!("[truncated, {} bytes total]", resp.body_len))
                            .color(colors::TEXT_MUTED)
                            .source(source_id),
                    );
                }
            } else if resp.body_len > 0 {
                parent = parent.push(
                    TextElement::new(format!("[binary, {} bytes]", resp.body_len))
                        .color(colors::TEXT_MUTED)
                        .source(source_id),
                );
            }
            parent
        }

        DomainValue::Interactive(req) => {
            // Check if this is a DiffViewer
            if let Some(crate::blocks::ViewState::DiffViewer { scroll_line, current_file, collapsed_indices }) = &block.view_state {
                if let Value::List(items) = &req.content {
                    return render_diff_viewer(parent, items, *scroll_line, *current_file, collapsed_indices, source_id);
                }
            }
            render_native_value(parent, &req.content, block, image_info, click_registry)
        }

        DomainValue::BlobChunk(chunk) => {
            let size = chunk.total_size.unwrap_or(chunk.data.len() as u64);
            let src = chunk.source.as_deref().unwrap_or("binary");
            parent = parent.push(
                TextElement::new(format!("[{}: {} {}]", src, chunk.content_type, nexus_api::format_size(size)))
                    .color(colors::TEXT_MUTED)
                    .source(source_id),
            );
            parent
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
    click_registry: &RefCell<HashMap<SourceId, ClickAction>>,
) {
    let block_id = block.id;
    let indent_px = depth as f32 * 20.0;

    for entry in entries {
        let is_dir = matches!(entry.file_type, FileType::Directory);
        let is_expanded = is_dir && block.file_tree().map_or(false, |t| t.is_expanded(&entry.path));
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
            let chevron = if is_expanded { "\u{25BC}" } else { "\u{25B6}" };
            let expand_id = source_ids::tree_expand(block_id, *expand_idx);
            register_tree_toggle(click_registry, expand_id, block_id, entry.path.clone());
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
        register_anchor(click_registry, anchor_id, AnchorEntry {
            block_id,
            action: value_to_anchor_action(&file_value),
            drag_payload: DragPayload::FilePath(entry.path.clone()),
        });
        *anchor_idx += 1;

        let source_id = source_ids::native(block_id);
        row = row.push(
            TextElement::new(display)
                .color(color)
                .source(source_id)
                .widget_id(anchor_id)
                .cursor_hint(CursorIcon::Pointer),
        );

        *parent = std::mem::take(parent).push(row);

        // Recursively render children if expanded
        if is_expanded {
            if let Some(children) = block.file_tree().and_then(|t| t.get_children(&entry.path)) {
                let child_refs: Vec<&FileEntry> = children.iter().collect();
                render_file_entries(
                    parent,
                    &child_refs,
                    block,
                    depth + 1,
                    anchor_idx,
                    expand_idx,
                    click_registry,
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
fn render_diff_viewer(
    mut parent: Column,
    items: &[Value],
    scroll_line: usize,
    current_file: usize,
    collapsed_indices: &std::collections::HashSet<usize>,
    source_id: strata::SourceId,
) -> Column {
    use nexus_api::DomainValue;

    // Header with keybinding hints
    parent = parent.push(
        TextElement::new("j/k: scroll | n/p: next/prev file | space: toggle | q: quit")
            .color(colors::TEXT_MUTED)
            .source(source_id),
    );

    let viewport_height = 50;
    let viewport_start = scroll_line;
    let viewport_end = scroll_line + viewport_height;

    // First pass: count total lines per file to find viewport boundaries.
    // This avoids allocating strings for lines outside the viewport.
    struct FileSpan {
        file_idx: usize,
        line_start: usize,
        line_count: usize,
    }
    let mut spans: Vec<FileSpan> = Vec::new();
    let mut total_lines = 0usize;

    for (file_idx, item) in items.iter().enumerate() {
        let diff = match item {
            Value::Domain(d) => match d.as_ref() {
                DomainValue::DiffFile(diff) => diff,
                _ => continue,
            },
            _ => continue,
        };

        let is_collapsed = collapsed_indices.contains(&file_idx);
        // 1 line for header + hunks + blank separator
        let mut count = 1; // header
        if !is_collapsed {
            for hunk in &diff.hunks {
                count += 1 + hunk.lines.len(); // hunk header + diff lines
            }
            count += 1; // blank separator
        }
        spans.push(FileSpan { file_idx, line_start: total_lines, line_count: count });
        total_lines += count;
    }

    // Second pass: only generate text for lines within the viewport.
    // Collect DiffFile references matching the order of spans.
    let diffs: Vec<&nexus_api::DiffFileInfo> = items.iter().filter_map(|item| {
        if let Value::Domain(d) = item {
            if let DomainValue::DiffFile(diff) = d.as_ref() {
                return Some(diff);
            }
        }
        None
    }).collect();

    for (span_idx, span) in spans.iter().enumerate() {
        let span_end = span.line_start + span.line_count;

        // Skip files entirely before viewport
        if span_end <= viewport_start {
            continue;
        }
        // Stop once past viewport
        if span.line_start >= viewport_end {
            break;
        }

        let item = diffs[span_idx];
        let mut line_num = span.line_start;

        let is_collapsed = collapsed_indices.contains(&span.file_idx);

        // Header line
        if line_num >= viewport_start && line_num < viewport_end {
            let cursor = if span.file_idx == current_file { "\u{25B6} " } else { "  " };
            let collapse_marker = if is_collapsed { "\u{25B8}" } else { "\u{25BE}" };
            let header_color = if span.file_idx == current_file {
                Color::rgb(1.0, 1.0, 0.6)
            } else {
                colors::TEXT_PATH
            };
            let old_path_suffix = if let Some(ref old) = item.old_path {
                format!(" (from {})", old)
            } else {
                String::new()
            };
            parent = parent.push(
                TextElement::new(format!("{}{} {} (+{} -{}){}", cursor, collapse_marker,
                    item.file_path, item.additions, item.deletions, old_path_suffix))
                    .color(header_color)
                    .source(source_id),
            );
        }
        line_num += 1;

        if is_collapsed {
            continue;
        }

        for hunk in &item.hunks {
            // Hunk header
            if line_num >= viewport_start && line_num < viewport_end {
                parent = parent.push(
                    TextElement::new(format!("@@ -{},{} +{},{} @@ {}",
                        hunk.old_start, hunk.old_count,
                        hunk.new_start, hunk.new_count, hunk.header))
                        .color(Color::rgb(0.5, 0.5, 1.0))
                        .source(source_id),
                );
            }
            line_num += 1;

            for diff_line in &hunk.lines {
                if line_num >= viewport_start && line_num < viewport_end {
                    let (prefix, color) = match diff_line.kind {
                        nexus_api::DiffLineKind::Addition => ("+", Color::rgb(0.4, 0.9, 0.4)),
                        nexus_api::DiffLineKind::Deletion => ("-", Color::rgb(0.9, 0.4, 0.4)),
                        nexus_api::DiffLineKind::Context => (" ", colors::TEXT_SECONDARY),
                    };
                    parent = parent.push(
                        TextElement::new(format!("{}{}", prefix, diff_line.content))
                            .color(color)
                            .source(source_id),
                    );
                }
                line_num += 1;
                if line_num >= viewport_end { break; }
            }
            if line_num >= viewport_end { break; }
        }

        // Blank separator
        if line_num >= viewport_start && line_num < viewport_end {
            parent = parent.push(
                TextElement::new("")
                    .color(colors::TEXT_PRIMARY)
                    .source(source_id),
            );
        }
    }

    // Footer with position info
    let end = viewport_end.min(total_lines);
    if total_lines > viewport_height {
        parent = parent.push(
            TextElement::new(format!("  [{}-{}/{}]", scroll_line + 1, end, total_lines))
                .color(colors::TEXT_MUTED)
                .source(source_id),
        );
    }

    parent
}

fn format_eta(seconds: f64) -> String {
    if seconds < 1.0 {
        "<1s".to_string()
    } else if seconds < 60.0 {
        format!("{}s", seconds as u64)
    } else if seconds < 3600.0 {
        let m = (seconds / 60.0) as u64;
        let s = (seconds % 60.0) as u64;
        format!("{}m {}s", m, s)
    } else {
        let h = (seconds / 3600.0) as u64;
        let m = ((seconds % 3600.0) / 60.0) as u64;
        format!("{}h {}m", h, m)
    }
}

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
/// Word-wrap text to fit within `max_chars` characters per line.
///
/// Preserves explicit newlines, breaks long lines at word boundaries,
/// and force-breaks words exceeding `max_chars`.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // =========================================================================
    // format_tokens tests
    // =========================================================================

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(format_tokens(0), "0 tokens");
        assert_eq!(format_tokens(1), "1 tokens");
        assert_eq!(format_tokens(100), "100 tokens");
        assert_eq!(format_tokens(999), "999 tokens");
    }

    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(format_tokens(1_000), "1.0k tokens");
        assert_eq!(format_tokens(1_500), "1.5k tokens");
        assert_eq!(format_tokens(10_000), "10.0k tokens");
        assert_eq!(format_tokens(999_999), "1000.0k tokens");
    }

    #[test]
    fn test_format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M tokens");
        assert_eq!(format_tokens(1_500_000), "1.5M tokens");
        assert_eq!(format_tokens(10_000_000), "10.0M tokens");
    }

    // =========================================================================
    // shorten_path tests
    // =========================================================================

    #[test]
    fn test_shorten_path_short() {
        assert_eq!(shorten_path("file.txt"), "file.txt");
        assert_eq!(shorten_path("/foo"), "/foo");
        assert_eq!(shorten_path("foo/bar"), "foo/bar");
    }

    #[test]
    fn test_shorten_path_long() {
        assert_eq!(shorten_path("/a/b/c"), "…/b/c");
        assert_eq!(shorten_path("/home/user/projects/file.txt"), "…/projects/file.txt");
        assert_eq!(shorten_path("/very/long/deeply/nested/path"), "…/nested/path");
    }

    #[test]
    fn test_shorten_path_trailing_slash() {
        // Filter removes empty parts from trailing slash
        assert_eq!(shorten_path("/a/b/c/"), "…/b/c");
    }

    // =========================================================================
    // truncate_str tests
    // =========================================================================

    #[test]
    fn test_truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_str_exact() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn test_truncate_str_long() {
        assert_eq!(truncate_str("hello world", 5), "hello…");
        assert_eq!(truncate_str("abcdefghij", 3), "abc…");
    }

    #[test]
    fn test_truncate_str_empty() {
        assert_eq!(truncate_str("", 5), "");
        assert_eq!(truncate_str("", 0), "");
    }

    // =========================================================================
    // format_eta tests
    // =========================================================================

    #[test]
    fn test_format_eta_subsecond() {
        assert_eq!(format_eta(0.0), "<1s");
        assert_eq!(format_eta(0.5), "<1s");
        assert_eq!(format_eta(0.99), "<1s");
    }

    #[test]
    fn test_format_eta_seconds() {
        assert_eq!(format_eta(1.0), "1s");
        assert_eq!(format_eta(30.0), "30s");
        assert_eq!(format_eta(59.9), "59s");
    }

    #[test]
    fn test_format_eta_minutes() {
        assert_eq!(format_eta(60.0), "1m 0s");
        assert_eq!(format_eta(90.0), "1m 30s");
        assert_eq!(format_eta(125.0), "2m 5s");
        assert_eq!(format_eta(3599.0), "59m 59s");
    }

    #[test]
    fn test_format_eta_hours() {
        assert_eq!(format_eta(3600.0), "1h 0m");
        assert_eq!(format_eta(3660.0), "1h 1m");
        assert_eq!(format_eta(7200.0), "2h 0m");
        assert_eq!(format_eta(7320.0), "2h 2m");
    }

    // =========================================================================
    // tool_header_label tests
    // =========================================================================

    fn make_tool(name: &str, params: &[(&str, &str)]) -> ToolInvocation {
        let mut parameters = HashMap::new();
        for (k, v) in params {
            parameters.insert(k.to_string(), v.to_string());
        }
        ToolInvocation {
            id: "test".to_string(),
            name: name.to_string(),
            parameters,
            output: None,
            message: None,
            status: crate::agent_block::ToolStatus::Pending,
            collapsed: false,
        }
    }

    #[test]
    fn test_tool_header_label_read() {
        let tool = make_tool("Read", &[("file_path", "/home/user/file.txt")]);
        assert_eq!(tool_header_label(&tool), "Read(…/user/file.txt)");
    }

    #[test]
    fn test_tool_header_label_edit() {
        let tool = make_tool("Edit", &[("file_path", "/home/user/src/main.rs")]);
        assert_eq!(tool_header_label(&tool), "Update(…/src/main.rs)");
    }

    #[test]
    fn test_tool_header_label_write() {
        let tool = make_tool("Write", &[("file_path", "output.txt")]);
        assert_eq!(tool_header_label(&tool), "Write(output.txt)");
    }

    #[test]
    fn test_tool_header_label_bash() {
        let tool = make_tool("Bash", &[("command", "ls -la")]);
        assert_eq!(tool_header_label(&tool), "Bash(ls -la)");
    }

    #[test]
    fn test_tool_header_label_bash_long() {
        let long_cmd = "a".repeat(100);
        let tool = make_tool("Bash", &[("command", &long_cmd)]);
        let result = tool_header_label(&tool);
        assert!(result.starts_with("Bash("));
        assert!(result.contains("…")); // Should be truncated
    }

    #[test]
    fn test_tool_header_label_bash_multiline() {
        let tool = make_tool("Bash", &[("command", "line1\nline2\nline3")]);
        assert_eq!(tool_header_label(&tool), "Bash(line1)");
    }

    #[test]
    fn test_tool_header_label_grep() {
        let tool = make_tool("Grep", &[("pattern", "TODO"), ("path", "/src")]);
        assert_eq!(tool_header_label(&tool), "Search(\"TODO\", /src)");
    }

    #[test]
    fn test_tool_header_label_glob() {
        let tool = make_tool("Glob", &[("pattern", "**/*.rs")]);
        assert_eq!(tool_header_label(&tool), "Glob(**/*.rs)");
    }

    #[test]
    fn test_tool_header_label_task() {
        let tool = make_tool("Task", &[("description", "Find all tests")]);
        assert_eq!(tool_header_label(&tool), "Task(Find all tests)");
    }

    #[test]
    fn test_tool_header_label_todo_write() {
        let tool = make_tool("TodoWrite", &[]);
        assert_eq!(tool_header_label(&tool), "TodoWrite");
    }

    #[test]
    fn test_tool_header_label_unknown() {
        let tool = make_tool("CustomTool", &[]);
        assert_eq!(tool_header_label(&tool), "CustomTool");
    }

    // =========================================================================
    // tool_collapsed_summary tests
    // =========================================================================

    fn make_tool_with_output(name: &str, params: &[(&str, &str)], output: &str) -> ToolInvocation {
        let mut tool = make_tool(name, params);
        tool.output = Some(output.to_string());
        tool
    }

    #[test]
    fn test_tool_collapsed_summary_no_output() {
        let tool = make_tool("Read", &[]);
        assert!(tool_collapsed_summary(&tool).is_none());
    }

    #[test]
    fn test_tool_collapsed_summary_read() {
        let tool = make_tool_with_output("Read", &[], "line1\nline2\nline3");
        assert_eq!(tool_collapsed_summary(&tool), Some("+3 lines".to_string()));
    }

    #[test]
    fn test_tool_collapsed_summary_edit() {
        let tool = make_tool_with_output("Edit", &[], "anything");
        assert_eq!(tool_collapsed_summary(&tool), Some("applied".to_string()));
    }

    #[test]
    fn test_tool_collapsed_summary_write() {
        let tool = make_tool_with_output("Write", &[("content", "a\nb\nc\nd")], "success");
        assert_eq!(tool_collapsed_summary(&tool), Some("+4 lines".to_string()));
    }

    #[test]
    fn test_tool_collapsed_summary_bash_empty() {
        let tool = make_tool_with_output("Bash", &[], "");
        assert_eq!(tool_collapsed_summary(&tool), Some("(no output)".to_string()));
    }

    #[test]
    fn test_tool_collapsed_summary_bash_single_line() {
        let tool = make_tool_with_output("Bash", &[], "Hello world");
        assert_eq!(tool_collapsed_summary(&tool), Some("Hello world".to_string()));
    }
}
