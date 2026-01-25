//! The `head` command - output the first part of files or piped input.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

pub struct HeadCommand;

/// Options parsed from command-line arguments.
#[derive(Default)]
struct HeadOptions {
    /// Number of lines/items to show (default: 10)
    count: usize,
    /// Files to read from (empty = stdin)
    files: Vec<PathBuf>,
}

impl HeadOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = HeadOptions {
            count: 10,
            files: Vec::new(),
        };

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];

            if arg == "-n" {
                // -n N form
                if i + 1 < args.len() {
                    if let Ok(n) = args[i + 1].parse::<usize>() {
                        opts.count = n;
                    }
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-n") {
                // -nN form (no space)
                if let Ok(n) = arg[2..].parse::<usize>() {
                    opts.count = n;
                }
            } else if arg.starts_with('-') && arg.len() > 1 {
                // -N form (deprecated but common)
                if let Ok(n) = arg[1..].parse::<usize>() {
                    opts.count = n;
                }
            } else if !arg.starts_with('-') {
                // File argument
                opts.files.push(PathBuf::from(arg));
            }

            i += 1;
        }

        opts
    }
}

impl NexusCommand for HeadCommand {
    fn name(&self) -> &'static str {
        "head"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = HeadOptions::parse(args);

        // If we have piped input, process it
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(head_value(stdin_value, opts.count));
        }

        // If files specified, read from files
        if !opts.files.is_empty() {
            let mut all_lines = Vec::new();

            for path in &opts.files {
                let resolved = if path.is_absolute() {
                    path.clone()
                } else {
                    ctx.state.cwd.join(path)
                };

                match head_file(&resolved, opts.count) {
                    Ok(lines) => all_lines.extend(lines),
                    Err(e) => {
                        return Err(anyhow::anyhow!("{}: {}", path.display(), e));
                    }
                }
            }

            return Ok(Value::List(
                all_lines.into_iter().map(Value::String).collect(),
            ));
        }

        // No input and no files - return empty
        Ok(Value::Unit)
    }
}

/// Apply head to a Value, returning the first N items.
fn head_value(value: Value, count: usize) -> Value {
    match value {
        Value::List(items) => {
            Value::List(items.into_iter().take(count).collect())
        }
        Value::Table { columns, rows } => {
            Value::Table {
                columns,
                rows: rows.into_iter().take(count).collect(),
            }
        }
        Value::Record(entries) => {
            // Take first N entries from the record
            Value::Record(entries.into_iter().take(count).collect())
        }
        Value::String(s) => {
            // Take first N lines
            let lines: Vec<&str> = s.lines().take(count).collect();
            Value::String(lines.join("\n"))
        }
        Value::Bytes(bytes) => {
            // Take first N lines from bytes
            let s = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = s.lines().take(count).collect();
            Value::String(lines.join("\n"))
        }
        // For scalar values, just pass through
        other => other,
    }
}

/// Read the first N lines from a file.
fn head_file(path: &PathBuf, count: usize) -> anyhow::Result<Vec<String>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let lines: Vec<String> = reader
        .lines()
        .take(count)
        .collect::<Result<Vec<_>, _>>()?;

    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_dash_number() {
        let opts = HeadOptions::parse(&["-5".to_string()]);
        assert_eq!(opts.count, 5);
    }

    #[test]
    fn test_parse_n_number() {
        let opts = HeadOptions::parse(&["-n".to_string(), "3".to_string()]);
        assert_eq!(opts.count, 3);
    }

    #[test]
    fn test_parse_n_attached() {
        let opts = HeadOptions::parse(&["-n10".to_string()]);
        assert_eq!(opts.count, 10);
    }
}
