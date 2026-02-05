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
        Value::Media { data, .. } => {
            // Treat media as bytes, take first N lines (lossy UTF-8)
            let s = String::from_utf8_lossy(&data);
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

    // -------------------------------------------------------------------------
    // HeadOptions::parse tests
    // -------------------------------------------------------------------------

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

    #[test]
    fn test_parse_default() {
        let opts = HeadOptions::parse(&[]);
        assert_eq!(opts.count, 10);
        assert!(opts.files.is_empty());
    }

    #[test]
    fn test_parse_with_files() {
        let opts = HeadOptions::parse(&["file1.txt".to_string(), "file2.txt".to_string()]);
        assert_eq!(opts.files.len(), 2);
        assert_eq!(opts.files[0], PathBuf::from("file1.txt"));
    }

    #[test]
    fn test_parse_mixed() {
        let opts = HeadOptions::parse(&["-n".to_string(), "5".to_string(), "file.txt".to_string()]);
        assert_eq!(opts.count, 5);
        assert_eq!(opts.files.len(), 1);
    }

    // -------------------------------------------------------------------------
    // head_value tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_head_value_list() {
        let list = Value::List(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
            Value::Int(4),
            Value::Int(5),
        ]);
        let result = head_value(list, 3);
        if let Value::List(items) = result {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], Value::Int(1));
            assert_eq!(items[2], Value::Int(3));
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_head_value_list_more_than_available() {
        let list = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let result = head_value(list, 10);
        if let Value::List(items) = result {
            assert_eq!(items.len(), 2);
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_head_value_table() {
        let table = Value::table(
            vec!["col1"],
            vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(3)],
            ],
        );
        let result = head_value(table, 2);
        if let Value::Table { rows, .. } = result {
            assert_eq!(rows.len(), 2);
        } else {
            panic!("Expected table");
        }
    }

    #[test]
    fn test_head_value_record() {
        let record = Value::Record(vec![
            ("a".to_string(), Value::Int(1)),
            ("b".to_string(), Value::Int(2)),
            ("c".to_string(), Value::Int(3)),
        ]);
        let result = head_value(record, 2);
        if let Value::Record(entries) = result {
            assert_eq!(entries.len(), 2);
        } else {
            panic!("Expected record");
        }
    }

    #[test]
    fn test_head_value_string() {
        let s = Value::String("line1\nline2\nline3\nline4".to_string());
        let result = head_value(s, 2);
        if let Value::String(text) = result {
            assert_eq!(text, "line1\nline2");
        } else {
            panic!("Expected string");
        }
    }

    #[test]
    fn test_head_value_bytes() {
        let bytes = Value::Bytes(b"line1\nline2\nline3".to_vec());
        let result = head_value(bytes, 2);
        if let Value::String(text) = result {
            assert_eq!(text, "line1\nline2");
        } else {
            panic!("Expected string");
        }
    }

    #[test]
    fn test_head_value_scalar_passthrough() {
        let int = Value::Int(42);
        let result = head_value(int, 5);
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn test_head_command_name() {
        let cmd = HeadCommand;
        assert_eq!(cmd.name(), "head");
    }
}
