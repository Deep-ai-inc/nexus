//! history - Command history with full-text search.
//!
//! Uses SQLite for persistent, searchable command history.

use super::{CommandContext, NexusCommand};
use crate::persistence::Store;
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

        // Open the store (read-only connection)
        let store = match Store::open_default() {
            Ok(s) => s,
            Err(e) => {
                return Ok(Value::Error {
                    code: 1,
                    message: format!("Failed to open history database: {}", e),
                });
            }
        };

        // Search or list history
        let entries = if let Some(query) = search_query {
            // Full-text search
            match store.search_history(&query, limit) {
                Ok(e) => e,
                Err(e) => {
                    return Ok(Value::Error {
                        code: 1,
                        message: format!("Search failed: {}", e),
                    });
                }
            }
        } else {
            // Recent history
            match store.get_recent_history(limit) {
                Ok(e) => e,
                Err(e) => {
                    return Ok(Value::Error {
                        code: 1,
                        message: format!("Failed to get history: {}", e),
                    });
                }
            }
        };

        // Convert to structured output
        let rows: Vec<Vec<Value>> = entries
            .iter()
            .map(|entry| {
                vec![
                    Value::Int(entry.id),
                    Value::String(entry.command.clone()),
                    entry.exit_code.map(|c| Value::Int(c as i64)).unwrap_or(Value::Unit),
                    entry.duration_ms
                        .map(|d| Value::String(format_duration(d)))
                        .unwrap_or(Value::Unit),
                    Value::String(format_relative_time(entry.timestamp)),
                ]
            })
            .collect();

        Ok(Value::table(vec!["id", "command", "exit", "duration", "when"], rows))
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

/// Format duration in human-readable form.
fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let minutes = ms / 60_000;
        let seconds = (ms % 60_000) / 1000;
        format!("{}m{}s", minutes, seconds)
    }
}

/// Format timestamp as relative time.
fn format_relative_time(timestamp: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(timestamp);

    if diff.num_seconds() < 60 {
        "just now".to_string()
    } else if diff.num_minutes() < 60 {
        format!("{}m ago", diff.num_minutes())
    } else if diff.num_hours() < 24 {
        format!("{}h ago", diff.num_hours())
    } else if diff.num_days() < 7 {
        format!("{}d ago", diff.num_days())
    } else {
        timestamp.format("%Y-%m-%d").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(50), "50ms");
        assert_eq!(format_duration(1500), "1.5s");
        assert_eq!(format_duration(65000), "1m5s");
    }

    #[test]
    fn test_history_command_name() {
        let cmd = HistoryCommand;
        assert_eq!(cmd.name(), "history");
    }
}
