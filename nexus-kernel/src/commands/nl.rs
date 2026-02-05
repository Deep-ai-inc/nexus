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

            let mut new_columns = vec![nexus_api::TableColumn::new("#")];
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

/// Number typed items while preserving their original types, returning a Table.
fn number_typed_items(items: Vec<Value>, opts: &NlOptions) -> Value {
    let mut rows = Vec::new();
    let mut line_num = opts.starting_line;

    for item in items {
        let text = item.to_text();
        let should_number = match opts.body_numbering {
            NumberingStyle::All => true,
            NumberingStyle::NonEmpty => !text.trim().is_empty(),
            NumberingStyle::None => false,
        };

        let num_value = if should_number {
            let v = Value::Int(line_num);
            line_num += opts.increment;
            v
        } else {
            Value::Unit
        };

        rows.push(vec![num_value, item]);
    }

    Value::Table {
        columns: vec![
            nexus_api::TableColumn::new("#"),
            nexus_api::TableColumn::new("value"),
        ],
        rows,
    }
}

fn number_lines(lines: &[String], opts: &NlOptions) -> Value {
    let mut rows = Vec::new();
    let mut line_num = opts.starting_line;

    for line in lines {
        let should_number = match opts.body_numbering {
            NumberingStyle::All => true,
            NumberingStyle::NonEmpty => !line.trim().is_empty(),
            NumberingStyle::None => false,
        };

        let num_value = if should_number {
            let v = Value::Int(line_num);
            line_num += opts.increment;
            v
        } else {
            Value::Unit
        };

        rows.push(vec![num_value, Value::String(line.clone())]);
    }

    Value::Table {
        columns: vec![
            nexus_api::TableColumn::new("#"),
            nexus_api::TableColumn::new("line"),
        ],
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // NlOptions::parse tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_defaults() {
        let (opts, files) = NlOptions::parse(&[]);
        assert_eq!(opts.starting_line, 1);
        assert_eq!(opts.increment, 1);
        assert_eq!(opts.separator, "\t");
        assert_eq!(opts.width, 6);
        assert!(files.is_empty());
    }

    #[test]
    fn test_parse_starting_line_separate() {
        let (opts, _) = NlOptions::parse(&["-v".to_string(), "10".to_string()]);
        assert_eq!(opts.starting_line, 10);
    }

    #[test]
    fn test_parse_starting_line_attached() {
        let (opts, _) = NlOptions::parse(&["-v5".to_string()]);
        assert_eq!(opts.starting_line, 5);
    }

    #[test]
    fn test_parse_increment_separate() {
        let (opts, _) = NlOptions::parse(&["-i".to_string(), "2".to_string()]);
        assert_eq!(opts.increment, 2);
    }

    #[test]
    fn test_parse_increment_attached() {
        let (opts, _) = NlOptions::parse(&["-i3".to_string()]);
        assert_eq!(opts.increment, 3);
    }

    #[test]
    fn test_parse_separator_separate() {
        let (opts, _) = NlOptions::parse(&["-s".to_string(), ": ".to_string()]);
        assert_eq!(opts.separator, ": ");
    }

    #[test]
    fn test_parse_separator_attached() {
        let (opts, _) = NlOptions::parse(&["-s:".to_string()]);
        assert_eq!(opts.separator, ":");
    }

    #[test]
    fn test_parse_width_separate() {
        let (opts, _) = NlOptions::parse(&["-w".to_string(), "10".to_string()]);
        assert_eq!(opts.width, 10);
    }

    #[test]
    fn test_parse_width_attached() {
        let (opts, _) = NlOptions::parse(&["-w4".to_string()]);
        assert_eq!(opts.width, 4);
    }

    #[test]
    fn test_parse_body_numbering_all() {
        let (opts, _) = NlOptions::parse(&["-b".to_string(), "a".to_string()]);
        assert!(matches!(opts.body_numbering, NumberingStyle::All));
    }

    #[test]
    fn test_parse_body_numbering_none() {
        let (opts, _) = NlOptions::parse(&["-bn".to_string()]);
        assert!(matches!(opts.body_numbering, NumberingStyle::None));
    }

    #[test]
    fn test_parse_body_numbering_attached() {
        let (opts, _) = NlOptions::parse(&["-ba".to_string()]);
        assert!(matches!(opts.body_numbering, NumberingStyle::All));
    }

    #[test]
    fn test_parse_with_files() {
        let (_, files) = NlOptions::parse(&["file1.txt".to_string(), "file2.txt".to_string()]);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0], "file1.txt");
        assert_eq!(files[1], "file2.txt");
    }

    #[test]
    fn test_parse_mixed_args() {
        let (opts, files) = NlOptions::parse(&[
            "-v".to_string(), "5".to_string(),
            "-i2".to_string(),
            "file.txt".to_string(),
        ]);
        assert_eq!(opts.starting_line, 5);
        assert_eq!(opts.increment, 2);
        assert_eq!(files.len(), 1);
    }

    // -------------------------------------------------------------------------
    // number_lines tests
    // -------------------------------------------------------------------------

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

        if let Value::Table { columns, rows } = result {
            assert_eq!(columns.len(), 2);
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[0][0], Value::Int(1));
            assert_eq!(rows[0][1], Value::String("a".to_string()));
        } else {
            panic!("Expected Table");
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

        if let Value::Table { rows, .. } = result {
            assert_eq!(rows.len(), 3);
            // Empty line should have Unit for number
            assert_eq!(rows[1][0], Value::Unit);
            // Third line should be numbered 2, not 3
            assert_eq!(rows[2][0], Value::Int(2));
        } else {
            panic!("Expected Table");
        }
    }

    #[test]
    fn test_nl_starting_line() {
        let opts = NlOptions {
            starting_line: 100,
            increment: 1,
            separator: "\t".to_string(),
            width: 6,
            body_numbering: NumberingStyle::All,
        };

        let lines = vec!["a".to_string(), "b".to_string()];
        let result = number_lines(&lines, &opts);

        if let Value::Table { rows, .. } = result {
            assert_eq!(rows[0][0], Value::Int(100));
            assert_eq!(rows[1][0], Value::Int(101));
        } else {
            panic!("Expected Table");
        }
    }

    #[test]
    fn test_nl_increment() {
        let opts = NlOptions {
            starting_line: 1,
            increment: 5,
            separator: "\t".to_string(),
            width: 6,
            body_numbering: NumberingStyle::All,
        };

        let lines = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let result = number_lines(&lines, &opts);

        if let Value::Table { rows, .. } = result {
            assert_eq!(rows[0][0], Value::Int(1));
            assert_eq!(rows[1][0], Value::Int(6));
            assert_eq!(rows[2][0], Value::Int(11));
        } else {
            panic!("Expected Table");
        }
    }

    #[test]
    fn test_nl_no_numbering() {
        let opts = NlOptions {
            starting_line: 1,
            increment: 1,
            separator: "\t".to_string(),
            width: 6,
            body_numbering: NumberingStyle::None,
        };

        let lines = vec!["a".to_string(), "b".to_string()];
        let result = number_lines(&lines, &opts);

        if let Value::Table { rows, .. } = result {
            assert_eq!(rows[0][0], Value::Unit);
            assert_eq!(rows[1][0], Value::Unit);
        } else {
            panic!("Expected Table");
        }
    }

    // -------------------------------------------------------------------------
    // number_typed_items tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_number_typed_items_preserves_values() {
        let opts = NlOptions {
            starting_line: 1,
            increment: 1,
            separator: "\t".to_string(),
            width: 6,
            body_numbering: NumberingStyle::All,
        };

        let items = vec![
            Value::Int(100),
            Value::String("hello".to_string()),
            Value::Bool(true),
        ];
        let result = number_typed_items(items, &opts);

        if let Value::Table { columns, rows } = result {
            assert_eq!(columns.len(), 2);
            assert_eq!(rows.len(), 3);
            assert_eq!(rows[0][1], Value::Int(100));
            assert_eq!(rows[1][1], Value::String("hello".to_string()));
            assert_eq!(rows[2][1], Value::Bool(true));
        } else {
            panic!("Expected Table");
        }
    }

    // -------------------------------------------------------------------------
    // nl_value tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_nl_value_string() {
        let opts = NlOptions {
            starting_line: 1,
            increment: 1,
            separator: "\t".to_string(),
            width: 6,
            body_numbering: NumberingStyle::All,
        };

        let result = nl_value(Value::String("line1\nline2\n".to_string()), &opts);

        if let Value::Table { rows, .. } = result {
            assert_eq!(rows.len(), 2);
        } else {
            panic!("Expected Table");
        }
    }

    #[test]
    fn test_nl_value_table() {
        let opts = NlOptions {
            starting_line: 1,
            increment: 1,
            separator: "\t".to_string(),
            width: 6,
            body_numbering: NumberingStyle::All,
        };

        let table = Value::table(
            vec!["col1"],
            vec![
                vec![Value::String("a".to_string())],
                vec![Value::String("b".to_string())],
            ],
        );
        let result = nl_value(table, &opts);

        if let Value::Table { columns, rows } = result {
            // Original had 1 column, now has 2 (# + col1)
            assert_eq!(columns.len(), 2);
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0][0], Value::Int(1));
        } else {
            panic!("Expected Table");
        }
    }
}
