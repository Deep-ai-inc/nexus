//! fc - Fix command (history manipulation) - read-only simplified version.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

pub struct FcCommand;

impl NexusCommand for FcCommand {
    fn name(&self) -> &'static str {
        "fc"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut list_mode = false;
        let mut reverse = false;
        let mut suppress_numbers = false;
        let mut first: Option<i32> = None;
        let mut last: Option<i32> = None;

        // Parse options
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            match arg.as_str() {
                "-l" => list_mode = true,
                "-r" => reverse = true,
                "-n" => suppress_numbers = true,
                "-s" => {
                    // Re-execute command - not supported in simplified version
                    anyhow::bail!("fc -s: re-execution not supported");
                }
                "-e" => {
                    // Editor - not supported in simplified version
                    i += 1; // Skip editor name
                }
                arg if !arg.starts_with('-') => {
                    // Range specification
                    if first.is_none() {
                        first = arg.parse().ok();
                    } else if last.is_none() {
                        last = arg.parse().ok();
                    }
                }
                _ => {}
            }
            i += 1;
        }

        // Default to list mode for this read-only implementation
        if !list_mode {
            list_mode = true;
        }

        // Read history from shell history file if available
        let history = read_history_file();

        if history.is_empty() {
            return Ok(Value::List(vec![]));
        }

        // Determine range
        let len = history.len() as i32;
        let start = first.unwrap_or(-16).max(-len);
        let end = last.unwrap_or(-1).max(-len);

        // Convert negative indices to positive
        let start_idx = if start < 0 { (len + start) as usize } else { (start - 1) as usize };
        let end_idx = if end < 0 { (len + end) as usize } else { (end - 1) as usize };

        let start_idx = start_idx.min(history.len() - 1);
        let end_idx = end_idx.min(history.len() - 1);

        let (start_idx, end_idx) = if start_idx <= end_idx {
            (start_idx, end_idx)
        } else {
            (end_idx, start_idx)
        };

        let mut entries: Vec<Value> = history[start_idx..=end_idx]
            .iter()
            .enumerate()
            .map(|(i, cmd)| {
                if suppress_numbers {
                    Value::String(cmd.clone())
                } else {
                    Value::Record(vec![
                        ("num".to_string(), Value::Int((start_idx + i + 1) as i64)),
                        ("command".to_string(), Value::String(cmd.clone())),
                    ])
                }
            })
            .collect();

        if reverse {
            entries.reverse();
        }

        Ok(Value::List(entries))
    }
}

/// Read history from the shell history file.
fn read_history_file() -> Vec<String> {
    // Try common history file locations
    let home = std::env::var("HOME").unwrap_or_default();

    let history_files = [
        format!("{}/.nexus_history", home),
        format!("{}/.bash_history", home),
        format!("{}/.zsh_history", home),
    ];

    for path in &history_files {
        if let Ok(content) = std::fs::read_to_string(path) {
            let entries: Vec<String> = content
                .lines()
                .filter(|line| !line.is_empty())
                .filter(|line| !line.starts_with(':')) // Skip zsh timestamps
                .map(|s| {
                    // Handle zsh extended history format
                    if let Some(idx) = s.find(';') {
                        s[idx + 1..].to_string()
                    } else {
                        s.to_string()
                    }
                })
                .collect();

            if !entries.is_empty() {
                return entries;
            }
        }
    }

    Vec::new()
}

/// Parse history content into entries (exported for testing).
fn parse_history_content(content: &str) -> Vec<String> {
    content
        .lines()
        .filter(|line| !line.is_empty())
        .filter(|line| !line.starts_with(':')) // Skip zsh timestamps
        .map(|s| {
            // Handle zsh extended history format
            if let Some(idx) = s.find(';') {
                s[idx + 1..].to_string()
            } else {
                s.to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_fc_command_name() {
        let cmd = FcCommand;
        assert_eq!(cmd.name(), "fc");
    }

    #[test]
    fn test_fc_list_mode() {
        let cmd = FcCommand;
        let mut test_ctx = TestContext::new_default();

        // This will try to read history from files, which may or may not exist
        let result = cmd
            .execute(&["-l".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::List(_) => {} // Success - may be empty or have entries
            _ => panic!("Expected List"),
        }
    }

    #[test]
    fn test_fc_s_not_supported() {
        let cmd = FcCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&["-s".to_string()], &mut test_ctx.ctx());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not supported"));
    }

    #[test]
    fn test_parse_history_simple() {
        let content = "ls -la\ncd /tmp\necho hello\n";
        let entries = parse_history_content(content);

        assert_eq!(entries, vec!["ls -la", "cd /tmp", "echo hello"]);
    }

    #[test]
    fn test_parse_history_skip_empty_lines() {
        let content = "ls\n\ncd\n\n\npwd\n";
        let entries = parse_history_content(content);

        assert_eq!(entries, vec!["ls", "cd", "pwd"]);
    }

    #[test]
    fn test_parse_history_skip_zsh_timestamps() {
        let content = ": 1234567890:0;ls -la\n: 1234567891:0;cd /tmp\necho hello\n";
        let entries = parse_history_content(content);

        // Lines starting with ':' are filtered out
        assert_eq!(entries, vec!["echo hello"]);
    }

    #[test]
    fn test_parse_history_zsh_extended_format() {
        let content = "1234567890;ls -la\n1234567891;cd /tmp\n";
        let entries = parse_history_content(content);

        // Text after ';' is the command
        assert_eq!(entries, vec!["ls -la", "cd /tmp"]);
    }
}
