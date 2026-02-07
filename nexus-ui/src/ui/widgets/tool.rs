//! Tool widget — renders agent tool invocations.
//!
//! Consolidates the 8 tool-specific builders into a single widget with
//! common header/wrapper logic and dispatched content rendering.

use similar::{ChangeTag, TextDiff};

use crate::data::agent_block::{ToolInvocation, ToolStatus};
use crate::ui::theme;
use crate::utils::text::truncate_str;
use strata::content_address::SourceId;
use strata::layout::{Column, CrossAxisAlignment, Length, Padding, Row, TextElement};

// =========================================================================
// Message types
// =========================================================================

/// Messages for tool widget interactions.
#[derive(Debug, Clone)]
pub enum ToolMessage {
    ToggleCollapse,
}

// =========================================================================
// ToolWidget
// =========================================================================

/// Tool widget — renders a single tool invocation.
pub struct ToolWidget;

impl ToolWidget {
    /// Render a tool invocation.
    ///
    /// # Arguments
    /// * `tool` - The tool invocation to render
    /// * `toggle_id` - SourceId for the toggle button (collapse/expand)
    /// * `source_id` - Base SourceId for content
    pub fn view(tool: &ToolInvocation, toggle_id: SourceId, source_id: SourceId) -> Column<'static> {
        let (status_icon, status_color) = match tool.status {
            ToolStatus::Pending => ("\u{25CF}", theme::TOOL_PENDING),   // ●
            ToolStatus::Running => ("\u{25CF}", theme::RUNNING),        // ●
            ToolStatus::Success => ("\u{25CF}", theme::SUCCESS),        // ● green
            ToolStatus::Error   => ("\u{25CF}", theme::ERROR),          // ●
        };

        let header_label = tool_header_label(tool);

        // Header: just status dot + tool name (clickable to toggle)
        let mut header = Row::new()
            .id(toggle_id)
            .spacing(4.0)
            .cross_align(CrossAxisAlignment::Center)
            .push(TextElement::new(status_icon).color(status_color))
            .push(TextElement::new(&header_label).color(theme::TOOL_ACTION).source(source_id));

        if let Some(ref msg) = tool.message {
            header = header.push(TextElement::new(msg).color(theme::TEXT_MUTED).source(source_id));
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

    /// Try to translate a click into a ToolMessage.
    /// Returns None if the click doesn't belong to this tool's widgets.
    pub fn on_click(toggle_id: SourceId, click_id: SourceId) -> Option<ToolMessage> {
        if click_id == toggle_id {
            Some(ToolMessage::ToggleCollapse)
        } else {
            None
        }
    }
}

// =========================================================================
// Header label formatting
// =========================================================================

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
// Collapsed preview
// =========================================================================

/// Build a collapsed preview showing first few lines + summary.
fn build_collapsed_preview(tool: &ToolInvocation, source_id: SourceId) -> Column<'static> {
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
                .push(TextElement::new(prefix).color(theme::TEXT_MUTED))
                .push(TextElement::new(truncate_str(line, 80)).color(theme::TOOL_OUTPUT).source(source_id)),
        );
    }

    // Show remaining lines summary with expand hint
    let remaining = lines.len().saturating_sub(preview_count);
    if remaining > 0 {
        col = col.push(
            Row::new()
                .fixed_spacer(16.0)
                .spacing(4.0)
                .push(TextElement::new(format!("… +{} lines (ctrl+o to expand)", remaining)).color(theme::TEXT_MUTED).source(source_id)),
        );
    } else if lines.is_empty() {
        // No output - show summary from tool_collapsed_summary
        if let Some(summary) = tool_collapsed_summary(tool) {
            col = col.push(
                Row::new()
                    .fixed_spacer(16.0)
                    .spacing(4.0)
                    .push(TextElement::new(format!("… {}", summary)).color(theme::TEXT_MUTED).source(source_id)),
            );
        }
    }

    col
}

// =========================================================================
// Tool body rendering (dispatched by tool type)
// =========================================================================

/// Dispatch to tool-specific body rendering.
fn build_tool_body(tool: &ToolInvocation, source_id: SourceId) -> Column<'static> {
    match tool.name.as_str() {
        "Edit" => build_edit_body(tool, source_id),
        "Read" => build_read_body(tool, source_id),
        "Bash" => build_bash_body(tool, source_id),
        "Grep" | "Glob" => build_search_body(tool, source_id),
        "Write" => build_write_body(tool, source_id),
        "Task" => build_task_body(tool, source_id),
        _ => build_generic_body(tool, source_id),
    }
}

/// Edit tool: show a unified diff with colored +/- lines.
fn build_edit_body(tool: &ToolInvocation, source_id: SourceId) -> Column<'static> {
    let old = tool.parameters.get("old_string").map(|s| s.as_str()).unwrap_or("");
    let new = tool.parameters.get("new_string").map(|s| s.as_str()).unwrap_or("");

    let mut col = Column::new().spacing(1.0);

    if !old.is_empty() || !new.is_empty() {
        let diff = TextDiff::from_lines(old, new);
        let mut diff_col = Column::new()
            .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
            .background(theme::TOOL_ARTIFACT_BG)
            .corner_radius(4.0)
            .width(Length::Fill);

        let mut line_count = 0;
        for change in diff.iter_all_changes() {
            if line_count >= 60 { break; }
            let text = change.value().trim_end_matches('\n');
            let (prefix, text_color, bg) = match change.tag() {
                ChangeTag::Insert => ("+", theme::DIFF_ADD, Some(theme::DIFF_BG_ADD)),
                ChangeTag::Delete => ("-", theme::DIFF_REMOVE, Some(theme::DIFF_BG_REMOVE)),
                ChangeTag::Equal => (" ", theme::TEXT_MUTED, None),
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
                .push(TextElement::new("\u{2514}").color(theme::TOOL_RESULT))
                .push(TextElement::new(&truncate_str(output, 200)).color(theme::TOOL_OUTPUT).source(source_id)),
        );
    }

    col
}

/// Read tool: code block with line numbers.
fn build_read_body(tool: &ToolInvocation, source_id: SourceId) -> Column<'static> {
    let mut col = Column::new().spacing(1.0);
    if let Some(ref output) = tool.output {
        let mut code_col = Column::new()
            .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
            .background(theme::TOOL_ARTIFACT_BG)
            .corner_radius(4.0)
            .width(Length::Fill);
        for (i, line) in output.lines().take(50).enumerate() {
            let numbered = format!("{:4} {}", i + 1, line);
            code_col = code_col.push(TextElement::new(&numbered).color(theme::CODE_TEXT).source(source_id));
        }
        let total = output.lines().count();
        if total > 50 {
            code_col = code_col.push(
                TextElement::new(&format!("  \u{2026} ({} more lines)", total - 50))
                    .color(theme::TEXT_MUTED).source(source_id),
            );
        }
        col = col.push(code_col);
    }
    col
}

/// Bash tool: output in a code block with optional timeout display.
fn build_bash_body(tool: &ToolInvocation, source_id: SourceId) -> Column<'static> {
    let mut col = Column::new().spacing(1.0);

    if let Some(timeout) = tool.parameters.get("timeout") {
        col = col.push(
            Row::new()
                .fixed_spacer(16.0)
                .push(TextElement::new(&format!("timeout: {}ms", timeout)).color(theme::TEXT_MUTED).source(source_id)),
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
                        .push(TextElement::new("└").color(theme::TEXT_MUTED))
                        .push(TextElement::new(*line).color(theme::TOOL_OUTPUT).source(source_id)),
                );
            } else {
                col = col.push(
                    Row::new()
                        .fixed_spacer(28.0)
                        .push(TextElement::new(*line).color(theme::TOOL_OUTPUT).source(source_id)),
                );
            }
        }

        // Show remaining lines indicator
        if lines.len() > max_lines {
            col = col.push(
                Row::new()
                    .fixed_spacer(16.0)
                    .push(TextElement::new(format!("… ({} more lines)", lines.len() - max_lines)).color(theme::TEXT_MUTED).source(source_id)),
            );
        }
    }
    col
}

/// Grep/Glob tool: results list.
fn build_search_body(tool: &ToolInvocation, source_id: SourceId) -> Column<'static> {
    let mut col = Column::new().spacing(1.0);
    if let Some(ref output) = tool.output {
        for line in output.lines().take(30) {
            col = col.push(
                Row::new()
                    .fixed_spacer(16.0)
                    .push(TextElement::new(line).color(theme::TOOL_PATH).source(source_id)),
            );
        }
        let total = output.lines().count();
        if total > 30 {
            col = col.push(
                Row::new()
                    .fixed_spacer(16.0)
                    .push(
                        TextElement::new(&format!("  \u{2026} ({} more results)", total - 30))
                            .color(theme::TEXT_MUTED).source(source_id),
                    ),
            );
        }
    }
    col
}

/// Write tool: show content being written in green.
fn build_write_body(tool: &ToolInvocation, source_id: SourceId) -> Column<'static> {
    let mut col = Column::new().spacing(1.0);
    if let Some(content) = tool.parameters.get("content") {
        let mut code_col = Column::new()
            .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
            .background(theme::TOOL_ARTIFACT_BG)
            .corner_radius(4.0)
            .width(Length::Fill);
        for (i, line) in content.lines().take(30).enumerate() {
            let numbered = format!("{:4} {}", i + 1, line);
            code_col = code_col.push(TextElement::new(&numbered).color(theme::DIFF_ADD).source(source_id));
        }
        let total = content.lines().count();
        if total > 30 {
            code_col = code_col.push(
                TextElement::new(&format!("  \u{2026} ({} more lines)", total - 30))
                    .color(theme::TEXT_MUTED).source(source_id),
            );
        }
        col = col.push(code_col);
    }
    col
}

/// Task tool: sub-agent display with left-border threading.
fn build_task_body(tool: &ToolInvocation, source_id: SourceId) -> Column<'static> {
    let mut col = Column::new().spacing(1.0);
    if let Some(ref output) = tool.output {
        // Use a Row: thin left border column + indented content
        let mut content_col = Column::new()
            .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
            .background(theme::TOOL_ARTIFACT_BG)
            .corner_radius(4.0)
            .width(Length::Fill);
        for line in output.lines().take(40) {
            content_col = content_col.push(TextElement::new(line).color(theme::TOOL_OUTPUT).source(source_id));
        }
        let total = output.lines().count();
        if total > 40 {
            content_col = content_col.push(
                TextElement::new(&format!("  \u{2026} ({} more lines)", total - 40))
                    .color(theme::TEXT_MUTED).source(source_id),
            );
        }

        // Left border line + content
        let border_line = Column::new()
            .width(Length::Fixed(2.0))
            .height(Length::Fill)
            .background(theme::TOOL_BORDER);
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
fn build_generic_body(tool: &ToolInvocation, source_id: SourceId) -> Column<'static> {
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
                    .push(TextElement::new(&format!("{}: {}", name, display_value)).color(theme::TEXT_MUTED).source(source_id)),
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
                    .push(TextElement::new(line).color(theme::TOOL_OUTPUT).source(source_id)),
            );
        }
    }

    col
}

// =========================================================================
// Helper functions
// =========================================================================

/// Shorten a file path for display.
fn shorten_path(path: &str) -> String {
    // Remove common prefixes and show just the interesting part
    let p = std::path::Path::new(path);
    if let Some(name) = p.file_name() {
        if let Some(parent) = p.parent().and_then(|p| p.file_name()) {
            return format!("{}/{}", parent.to_string_lossy(), name.to_string_lossy());
        }
        return name.to_string_lossy().to_string();
    }
    path.to_string()
}


// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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
            status: ToolStatus::Success,
            message: None,
            collapsed: false,
        }
    }

    #[test]
    fn test_header_label_read() {
        let tool = make_tool("Read", &[("file_path", "/foo/bar/baz.rs")]);
        assert_eq!(tool_header_label(&tool), "Read(bar/baz.rs)");
    }

    #[test]
    fn test_header_label_bash() {
        let tool = make_tool("Bash", &[("command", "ls -la")]);
        assert_eq!(tool_header_label(&tool), "Bash(ls -la)");
    }

    #[test]
    fn test_shorten_path() {
        assert_eq!(shorten_path("/a/b/c.rs"), "b/c.rs");
        assert_eq!(shorten_path("file.rs"), "file.rs");
    }

}
