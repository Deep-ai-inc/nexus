//! The `tail` command - output the last part of files or piped input.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

pub struct TailCommand;

struct TailOptions {
    count: usize,
    files: Vec<PathBuf>,
}

impl TailOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = TailOptions {
            count: 10,
            files: Vec::new(),
        };

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];

            if arg == "-n" {
                if i + 1 < args.len() {
                    if let Ok(n) = args[i + 1].parse::<usize>() {
                        opts.count = n;
                    }
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-n") {
                if let Ok(n) = arg[2..].parse::<usize>() {
                    opts.count = n;
                }
            } else if arg.starts_with('-') && arg.len() > 1 {
                if let Ok(n) = arg[1..].parse::<usize>() {
                    opts.count = n;
                }
            } else if !arg.starts_with('-') {
                opts.files.push(PathBuf::from(arg));
            }

            i += 1;
        }

        opts
    }
}

impl NexusCommand for TailCommand {
    fn name(&self) -> &'static str {
        "tail"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = TailOptions::parse(args);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(tail_value(stdin_value, opts.count));
        }

        if !opts.files.is_empty() {
            let mut all_lines = Vec::new();

            for path in &opts.files {
                let resolved = if path.is_absolute() {
                    path.clone()
                } else {
                    ctx.state.cwd.join(path)
                };

                match tail_file(&resolved, opts.count) {
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

        Ok(Value::Unit)
    }
}

fn tail_value(value: Value, count: usize) -> Value {
    match value {
        Value::List(items) => {
            let len = items.len();
            let skip = len.saturating_sub(count);
            Value::List(items.into_iter().skip(skip).collect())
        }
        Value::Table { columns, rows } => {
            let len = rows.len();
            let skip = len.saturating_sub(count);
            Value::Table {
                columns,
                rows: rows.into_iter().skip(skip).collect(),
            }
        }
        Value::Record(entries) => {
            let len = entries.len();
            let skip = len.saturating_sub(count);
            Value::Record(entries.into_iter().skip(skip).collect())
        }
        Value::String(s) => {
            let lines: Vec<&str> = s.lines().collect();
            let len = lines.len();
            let skip = len.saturating_sub(count);
            Value::String(lines.into_iter().skip(skip).collect::<Vec<_>>().join("\n"))
        }
        Value::Bytes(bytes) => {
            let s = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = s.lines().collect();
            let len = lines.len();
            let skip = len.saturating_sub(count);
            Value::String(lines.into_iter().skip(skip).collect::<Vec<_>>().join("\n"))
        }
        Value::Media { data, .. } => {
            // Treat media as bytes, take last N lines (lossy UTF-8)
            let s = String::from_utf8_lossy(&data);
            let lines: Vec<&str> = s.lines().collect();
            let len = lines.len();
            let skip = len.saturating_sub(count);
            Value::String(lines.into_iter().skip(skip).collect::<Vec<_>>().join("\n"))
        }
        other => other,
    }
}

fn tail_file(path: &PathBuf, count: usize) -> anyhow::Result<Vec<String>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader.lines().collect::<Result<Vec<_>, _>>()?;
    let len = lines.len();
    let skip = len.saturating_sub(count);
    Ok(lines.into_iter().skip(skip).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // TailOptions::parse tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_dash_number() {
        let opts = TailOptions::parse(&["-5".to_string()]);
        assert_eq!(opts.count, 5);
    }

    #[test]
    fn test_parse_n_separate() {
        let opts = TailOptions::parse(&["-n".to_string(), "3".to_string()]);
        assert_eq!(opts.count, 3);
    }

    #[test]
    fn test_parse_n_attached() {
        let opts = TailOptions::parse(&["-n7".to_string()]);
        assert_eq!(opts.count, 7);
    }

    #[test]
    fn test_parse_default() {
        let opts = TailOptions::parse(&[]);
        assert_eq!(opts.count, 10);
        assert!(opts.files.is_empty());
    }

    #[test]
    fn test_parse_with_files() {
        let opts = TailOptions::parse(&["file1.txt".to_string(), "file2.txt".to_string()]);
        assert_eq!(opts.files.len(), 2);
    }

    #[test]
    fn test_parse_mixed() {
        let opts = TailOptions::parse(&["-n".to_string(), "20".to_string(), "log.txt".to_string()]);
        assert_eq!(opts.count, 20);
        assert_eq!(opts.files.len(), 1);
    }

    // -------------------------------------------------------------------------
    // tail_value tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_tail_value_list() {
        let list = Value::List(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
            Value::Int(4),
            Value::Int(5),
        ]);
        let result = tail_value(list, 3);
        if let Value::List(items) = result {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], Value::Int(3));
            assert_eq!(items[2], Value::Int(5));
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_tail_value_list_more_than_available() {
        let list = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let result = tail_value(list, 10);
        if let Value::List(items) = result {
            assert_eq!(items.len(), 2);
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_tail_value_table() {
        let table = Value::table(
            vec!["col1"],
            vec![
                vec![Value::Int(1)],
                vec![Value::Int(2)],
                vec![Value::Int(3)],
            ],
        );
        let result = tail_value(table, 2);
        if let Value::Table { rows, .. } = result {
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0][0], Value::Int(2));
            assert_eq!(rows[1][0], Value::Int(3));
        } else {
            panic!("Expected table");
        }
    }

    #[test]
    fn test_tail_value_record() {
        let record = Value::Record(vec![
            ("a".to_string(), Value::Int(1)),
            ("b".to_string(), Value::Int(2)),
            ("c".to_string(), Value::Int(3)),
        ]);
        let result = tail_value(record, 2);
        if let Value::Record(entries) = result {
            assert_eq!(entries.len(), 2);
            assert_eq!(entries[0].0, "b");
            assert_eq!(entries[1].0, "c");
        } else {
            panic!("Expected record");
        }
    }

    #[test]
    fn test_tail_value_string() {
        let s = Value::String("line1\nline2\nline3\nline4".to_string());
        let result = tail_value(s, 2);
        if let Value::String(text) = result {
            assert_eq!(text, "line3\nline4");
        } else {
            panic!("Expected string");
        }
    }

    #[test]
    fn test_tail_value_bytes() {
        let bytes = Value::Bytes(b"line1\nline2\nline3".to_vec());
        let result = tail_value(bytes, 2);
        if let Value::String(text) = result {
            assert_eq!(text, "line2\nline3");
        } else {
            panic!("Expected string");
        }
    }

    #[test]
    fn test_tail_value_scalar_passthrough() {
        let int = Value::Int(42);
        let result = tail_value(int, 5);
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn test_tail_command_name() {
        let cmd = TailCommand;
        assert_eq!(cmd.name(), "tail");
    }
}
