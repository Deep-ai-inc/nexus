//! Shell context injection for agent mode.
//!
//! This module builds shell context that gets prepended to user messages when
//! routing to the AI agent. The context is injected per-message (not in the
//! system prompt) to preserve prefix caching semantics - the system prompt
//! remains stable across requests.
//!
//! Context includes:
//! - Current working directory
//! - Last command output (serialized for LLM)
//! - Recent command history

use crate::data::Block;
use nexus_api::BlockState;
use nexus_term::TerminalGrid;
use std::rc::Rc;

/// Maximum number of recent commands to include in history.
const MAX_HISTORY_ENTRIES: usize = 10;

/// Maximum length of last output to include (to avoid blowing up context).
const MAX_OUTPUT_LENGTH: usize = 4000;

/// Build shell context from the current Nexus state.
///
/// This returns a formatted string that gets prepended to the user's message
/// when routing to the agent. The format is designed for LLM consumption.
pub fn build_shell_context(
    cwd: &str,
    blocks: &[Block],
    command_history: &[String],
) -> String {
    let mut ctx = String::new();

    // Header
    ctx.push_str("<shell_context>\n");

    // Current working directory (most important for agent orientation)
    ctx.push_str(&format!("cwd: {}\n", cwd));

    // Last command and output (critical for understanding what just happened)
    if let Some(last_block) = find_last_completed_block(blocks) {
        ctx.push_str("\nlast_command:\n");
        ctx.push_str(&format!("  $ {}\n", last_block.command));
        ctx.push_str(&format!("  exit_code: {}\n", exit_code_from_state(&last_block.state)));

        // Include output (native structured or terminal text)
        if let Some(ref value) = last_block.structured_output {
            let output = value.to_text();
            if !output.is_empty() {
                ctx.push_str("  output:\n");
                ctx.push_str(&indent_text(&truncate_output(&output), "    "));
                ctx.push('\n');
            }
        } else {
            // Fall back to terminal text - extract from grid
            let grid = last_block.parser.grid();
            let term_text = extract_text_from_grid(&grid);
            if !term_text.is_empty() {
                ctx.push_str("  output:\n");
                ctx.push_str(&indent_text(&truncate_output(&term_text), "    "));
                ctx.push('\n');
            }
        }
    }

    // Recent history (for pattern understanding)
    if !command_history.is_empty() {
        ctx.push_str("\nrecent_history:\n");
        let start = command_history.len().saturating_sub(MAX_HISTORY_ENTRIES);
        for cmd in &command_history[start..] {
            ctx.push_str(&format!("  - {}\n", cmd));
        }
    }

    ctx.push_str("</shell_context>\n\n");

    ctx
}

/// Build a minimal context for quick queries (less token usage).
#[allow(dead_code)]
pub fn build_minimal_context(cwd: &str) -> String {
    format!("<shell_context>\ncwd: {}\n</shell_context>\n\n", cwd)
}

/// Find the last completed (non-running) block.
fn find_last_completed_block(blocks: &[Block]) -> Option<&Block> {
    blocks.iter().rev().find(|b| !matches!(b.state, BlockState::Running))
}

/// Extract exit code from block state.
fn exit_code_from_state(state: &BlockState) -> i32 {
    match state {
        BlockState::Running => -1,
        BlockState::Success => 0,
        BlockState::Failed(code) => *code,
    }
}

/// Indent each line of text with the given prefix.
fn indent_text(text: &str, indent: &str) -> String {
    text.lines()
        .map(|line| format!("{}{}", indent, line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Truncate output if too long, adding an ellipsis marker.
fn truncate_output(output: &str) -> String {
    if output.len() <= MAX_OUTPUT_LENGTH {
        output.to_string()
    } else {
        // Find a safe char boundary at or before MAX_OUTPUT_LENGTH
        let mut end = MAX_OUTPUT_LENGTH;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        let truncated = &output[..end];
        // Try to cut at a line boundary for cleaner output
        if let Some(last_newline) = truncated.rfind('\n') {
            format!("{}\n... (output truncated)", &truncated[..last_newline])
        } else {
            format!("{}... (output truncated)", truncated)
        }
    }
}

/// Extract text content from a terminal grid.
fn extract_text_from_grid(grid: &Rc<TerminalGrid>) -> String {
    let mut lines = Vec::new();
    for row in 0..grid.rows() {
        let mut line = String::new();
        for col in 0..grid.cols() {
            if let Some(cell) = grid.get(col, row) {
                if cell.flags.wide_char_spacer { continue; }
                cell.push_grapheme(&mut line);
            }
        }
        // Trim trailing whitespace from each line
        let trimmed = line.trim_end();
        if !trimmed.is_empty() || !lines.is_empty() {
            lines.push(trimmed.to_string());
        }
    }
    // Remove trailing empty lines
    while lines.last().map_or(false, |l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Block;
    use nexus_api::{BlockId, Value};

    fn make_test_block(id: u64, command: &str, state: BlockState, structured_output: Option<Value>) -> Block {
        let mut block = Block::new(BlockId(id), command.to_string());
        block.state = state;
        block.structured_output = structured_output;
        block
    }

    #[test]
    fn test_build_shell_context_basic() {
        let blocks = vec![
            make_test_block(1, "ls -la", BlockState::Success, Some(Value::String("file1.txt\nfile2.txt".to_string()))),
        ];
        let history = vec!["cd /tmp".to_string(), "ls -la".to_string()];

        let ctx = build_shell_context("/home/user", &blocks, &history);

        assert!(ctx.contains("cwd: /home/user"));
        assert!(ctx.contains("$ ls -la"));
        assert!(ctx.contains("exit_code: 0"));
        assert!(ctx.contains("file1.txt"));
        assert!(ctx.contains("cd /tmp"));
    }

    #[test]
    fn test_build_shell_context_empty() {
        let ctx = build_shell_context("/tmp", &[], &[]);

        assert!(ctx.contains("cwd: /tmp"));
        assert!(!ctx.contains("last_command"));
        assert!(!ctx.contains("recent_history"));
    }

    #[test]
    fn test_truncate_output() {
        let short = "hello world";
        assert_eq!(truncate_output(short), short);

        let long = "x".repeat(5000);
        let truncated = truncate_output(&long);
        assert!(truncated.len() < long.len());
        assert!(truncated.contains("truncated"));
    }
}
