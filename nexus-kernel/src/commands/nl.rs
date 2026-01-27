//! The `nl` command - number lines.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::fs;
use std::path::PathBuf;

pub struct NlCommand;

struct NlOptions {
    starting_line: i64,
    increment: i64,
    separator: String,
    width: usize,
    body_numbering: NumberingStyle,
}

#[derive(Clone, Copy)]
enum NumberingStyle {
    All,       // -b a: number all lines
    NonEmpty,  // -b t: number non-empty lines (default)
    None,      // -b n: no numbering
}

impl NlOptions {
    fn parse(args: &[String]) -> (Self, Vec<String>) {
        let mut opts = NlOptions {
            starting_line: 1,
            increment: 1,
            separator: "\t".to_string(),
            width: 6,
            body_numbering: NumberingStyle::NonEmpty,
        };

        let mut files = Vec::new();
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];

            if arg == "-v" || arg == "--starting-line-number" {
                if i + 1 < args.len() {
                    opts.starting_line = args[i + 1].parse().unwrap_or(1);
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-v") {
                opts.starting_line = arg[2..].parse().unwrap_or(1);
            } else if arg == "-i" || arg == "--line-increment" {
                if i + 1 < args.len() {
                    opts.increment = args[i + 1].parse().unwrap_or(1);
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-i") {
                opts.increment = arg[2..].parse().unwrap_or(1);
            } else if arg == "-s" || arg == "--number-separator" {
                if i + 1 < args.len() {
                    opts.separator = args[i + 1].clone();
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-s") {
                opts.separator = arg[2..].to_string();
            } else if arg == "-w" || arg == "--number-width" {
                if i + 1 < args.len() {
                    opts.width = args[i + 1].parse().unwrap_or(6);
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-w") {
                opts.width = arg[2..].parse().unwrap_or(6);
            } else if arg == "-b" || arg == "--body-numbering" {
                if i + 1 < args.len() {
                    opts.body_numbering = match args[i + 1].as_str() {
                        "a" => NumberingStyle::All,
                        "t" => NumberingStyle::NonEmpty,
                        "n" => NumberingStyle::None,
                        _ => NumberingStyle::NonEmpty,
                    };
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-b") {
                opts.body_numbering = match &arg[2..] {
                    "a" => NumberingStyle::All,
                    "t" => NumberingStyle::NonEmpty,
                    "n" => NumberingStyle::None,
                    _ => NumberingStyle::NonEmpty,
                };
            } else if !arg.starts_with('-') {
                files.push(arg.clone());
            }

            i += 1;
        }

        (opts, files)
    }
}

impl NexusCommand for NlCommand {
    fn name(&self) -> &'static str {
        "nl"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let (opts, files) = NlOptions::parse(args);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(nl_value(stdin_value, &opts));
        }

        if files.is_empty() {
            return Ok(Value::Unit);
        }

        // Read from files
        let mut all_lines = Vec::new();
        for file in &files {
            let path = if PathBuf::from(file).is_absolute() {
                PathBuf::from(file)
            } else {
                ctx.state.cwd.join(file)
            };

            match fs::read_to_string(&path) {
                Ok(content) => {
                    all_lines.extend(content.lines().map(|s| s.to_string()));
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("{}: {}", file, e));
                }
            }
        }

        Ok(number_lines(&all_lines, &opts))
    }
}

fn nl_value(value: Value, opts: &NlOptions) -> Value {
    match value {
        Value::List(items) => {
            // Preserve typed values with line numbers
            number_typed_items(items, opts)
        }
        Value::String(s) => {
            let lines: Vec<String> = s.lines().map(|l| l.to_string()).collect();
            number_lines(&lines, opts)
        }
        Value::Table { columns, rows } => {
            // Number rows in a table
            let mut new_rows = Vec::new();
            let mut line_num = opts.starting_line;

            for row in rows {
                let numbered_row: Vec<Value> = std::iter::once(Value::Int(line_num))
                    .chain(row.into_iter())
                    .collect();
                new_rows.push(numbered_row);
                line_num += opts.increment;
            }

            let mut new_columns = vec!["#".to_string()];
            new_columns.extend(columns);

            Value::Table {
                columns: new_columns,
                rows: new_rows,
            }
        }
        other => {
            let text = other.to_text();
            let lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
            number_lines(&lines, opts)
        }
    }
}

/// Number typed items while preserving their original types
fn number_typed_items(items: Vec<Value>, opts: &NlOptions) -> Value {
    let mut result = Vec::new();
    let mut line_num = opts.starting_line;

    for item in items {
        let text = item.to_text();
        let should_number = match opts.body_numbering {
            NumberingStyle::All => true,
            NumberingStyle::NonEmpty => !text.trim().is_empty(),
            NumberingStyle::None => false,
        };

        if should_number {
            // Return a record with line number and original typed value
            result.push(Value::Record(vec![
                ("line".to_string(), Value::Int(line_num)),
                ("value".to_string(), item),
            ]));
            line_num += opts.increment;
        } else {
            // No line number, but still include the item
            result.push(Value::Record(vec![
                ("line".to_string(), Value::Unit),
                ("value".to_string(), item),
            ]));
        }
    }

    Value::List(result)
}

fn number_lines(lines: &[String], opts: &NlOptions) -> Value {
    let mut result = Vec::new();
    let mut line_num = opts.starting_line;

    for line in lines {
        let should_number = match opts.body_numbering {
            NumberingStyle::All => true,
            NumberingStyle::NonEmpty => !line.trim().is_empty(),
            NumberingStyle::None => false,
        };

        let numbered_line = if should_number {
            let num_str = format!("{:>width$}", line_num, width = opts.width);
            let formatted = format!("{}{}{}", num_str, opts.separator, line);
            line_num += opts.increment;
            formatted
        } else {
            format!("{:>width$}{}{}", "", opts.separator, line, width = opts.width)
        };

        result.push(Value::String(numbered_line));
    }

    Value::List(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nl_basic() {
        let opts = NlOptions {
            starting_line: 1,
            increment: 1,
            separator: "\t".to_string(),
            width: 6,
            body_numbering: NumberingStyle::All,
        };

        let lines = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let result = number_lines(&lines, &opts);

        if let Value::List(items) = result {
            assert_eq!(items.len(), 3);
            assert!(items[0].to_text().contains("1"));
            assert!(items[0].to_text().contains("a"));
        }
    }

    #[test]
    fn test_nl_skip_empty() {
        let opts = NlOptions {
            starting_line: 1,
            increment: 1,
            separator: "\t".to_string(),
            width: 6,
            body_numbering: NumberingStyle::NonEmpty,
        };

        let lines = vec!["a".to_string(), "".to_string(), "c".to_string()];
        let result = number_lines(&lines, &opts);

        if let Value::List(items) = result {
            assert_eq!(items.len(), 3);
            // Third line should be numbered 2, not 3
            assert!(items[2].to_text().contains("2"));
        }
    }
}
