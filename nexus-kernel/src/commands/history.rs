//! history - Command history from native shell history file.
//!
//! Reads from ~/.zsh_history or ~/.bash_history (auto-detected).

use super::{CommandContext, NexusCommand};
use crate::shell_history::ShellHistory;
use nexus_api::Value;

pub struct HistoryCommand;

impl NexusCommand for HistoryCommand {
    fn name(&self) -> &'static str {
        "history"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // Parse arguments
        let mut search_query: Option<String> = None;
        let mut limit: usize = 50;
        let mut show_all = false;

        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "-n" | "--limit" => {
                    if i + 1 < args.len() {
                        limit = args[i + 1].parse().unwrap_or(50);
                        i += 1;
                    }
                }
                "-a" | "--all" => {
                    show_all = true;
                }
                "search" => {
                    // history search <query>
                    if i + 1 < args.len() {
                        search_query = Some(args[i + 1..].join(" "));
                        break;
                    }
                }
                arg if !arg.starts_with('-') && search_query.is_none() => {
                    // Bare argument is treated as search query
                    search_query = Some(arg.to_string());
                }
                _ => {}
            }
            i += 1;
        }

        if show_all {
            limit = 10000; // Large limit for "all"
        }

        // Open native shell history (read-only snapshot)
        let hist = match ShellHistory::open() {
            Some(h) => h,
            None => {
                return Ok(Value::Error {
                    code: 1,
                    message: "Could not detect shell history file".to_string(),
                });
            }
        };

        // Search or list history
        let entries = if let Some(query) = search_query {
            hist.search(&query, limit)
        } else {
            hist.recent(limit)
        };

        // Convert to structured output
        let rows: Vec<Vec<Value>> = entries
            .iter()
            .map(|entry| {
                vec![Value::String(entry.command.clone())]
            })
            .collect();

        Ok(Value::table(vec!["command"], rows))
    }
}

/// fc - Fix command (POSIX compatibility, uses same history).
pub struct FcCommand;

impl NexusCommand for FcCommand {
    fn name(&self) -> &'static str {
        "fc"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // fc -l is equivalent to history
        let mut new_args = Vec::new();
        let mut list_mode = false;

        for arg in args {
            match arg.as_str() {
                "-l" => list_mode = true,
                "-s" => anyhow::bail!("fc -s: re-execution not supported"),
                "-e" => {} // Skip editor flag
                _ => new_args.push(arg.clone()),
            }
        }

        if list_mode || args.is_empty() {
            // Delegate to history command
            let history = HistoryCommand;
            history.execute(&new_args, ctx)
        } else {
            Ok(Value::Error {
                code: 1,
                message: "fc: only list mode (-l) is supported".to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_command_name() {
        let cmd = HistoryCommand;
        assert_eq!(cmd.name(), "history");
    }

    #[test]
    fn test_fc_command_name() {
        let cmd = FcCommand;
        assert_eq!(cmd.name(), "fc");
    }
}
