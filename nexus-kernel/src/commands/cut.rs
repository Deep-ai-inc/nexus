//! The `cut` command - remove sections from each line.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::collections::HashSet;

pub struct CutCommand;

struct CutOptions {
    delimiter: char,
    fields: Vec<usize>,
    characters: Vec<usize>,
    bytes: Vec<usize>,
    output_delimiter: Option<String>,
    complement: bool,
}

impl CutOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = CutOptions {
            delimiter: '\t',
            fields: Vec::new(),
            characters: Vec::new(),
            bytes: Vec::new(),
            output_delimiter: None,
            complement: false,
        };

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];

            if arg == "-d" || arg == "--delimiter" {
                if i + 1 < args.len() {
                    let d = &args[i + 1];
                    if !d.is_empty() {
                        opts.delimiter = d.chars().next().unwrap();
                    }
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-d") {
                let d = &arg[2..];
                if !d.is_empty() {
                    opts.delimiter = d.chars().next().unwrap();
                }
            } else if arg == "-f" || arg == "--fields" {
                if i + 1 < args.len() {
                    opts.fields = parse_range(&args[i + 1]);
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-f") {
                opts.fields = parse_range(&arg[2..]);
            } else if arg == "-c" || arg == "--characters" {
                if i + 1 < args.len() {
                    opts.characters = parse_range(&args[i + 1]);
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-c") {
                opts.characters = parse_range(&arg[2..]);
            } else if arg == "-b" || arg == "--bytes" {
                if i + 1 < args.len() {
                    opts.bytes = parse_range(&args[i + 1]);
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-b") {
                opts.bytes = parse_range(&arg[2..]);
            } else if arg == "--output-delimiter" {
                if i + 1 < args.len() {
                    opts.output_delimiter = Some(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            } else if arg == "--complement" {
                opts.complement = true;
            }

            i += 1;
        }

        opts
    }
}

fn parse_range(s: &str) -> Vec<usize> {
    let mut result = Vec::new();

    for part in s.split(',') {
        if part.contains('-') {
            let mut parts = part.split('-');
            let start: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(1);
            let end: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(start);
            for i in start..=end {
                result.push(i);
            }
        } else if let Ok(n) = part.parse::<usize>() {
            result.push(n);
        }
    }

    result
}

impl NexusCommand for CutCommand {
    fn name(&self) -> &'static str {
        "cut"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = CutOptions::parse(args);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(cut_value(stdin_value, &opts));
        }

        Ok(Value::Unit)
    }
}

fn cut_value(value: Value, opts: &CutOptions) -> Value {
    match value {
        Value::List(items) => {
            Value::List(
                items
                    .into_iter()
                    .map(|item| cut_item(item, opts))
                    .collect(),
            )
        }
        Value::Table { columns, rows } => {
            if !opts.fields.is_empty() {
                // Select specific columns
                let field_set: HashSet<usize> = opts.fields.iter().copied().collect();
                let selected_indices: Vec<usize> = if opts.complement {
                    (1..=columns.len())
                        .filter(|i| !field_set.contains(i))
                        .map(|i| i - 1)
                        .collect()
                } else {
                    opts.fields.iter().map(|i| i.saturating_sub(1)).collect()
                };

                let new_columns: Vec<String> = selected_indices
                    .iter()
                    .filter_map(|&i| columns.get(i).cloned())
                    .collect();

                let new_rows: Vec<Vec<Value>> = rows
                    .into_iter()
                    .map(|row| {
                        selected_indices
                            .iter()
                            .filter_map(|&i| row.get(i).cloned())
                            .collect()
                    })
                    .collect();

                Value::Table {
                    columns: new_columns,
                    rows: new_rows,
                }
            } else {
                Value::Table { columns, rows }
            }
        }
        Value::String(s) => {
            let lines: Vec<String> = s.lines().map(|line| cut_line(line, opts)).collect();
            Value::String(lines.join("\n"))
        }
        other => other,
    }
}

fn cut_item(item: Value, opts: &CutOptions) -> Value {
    match item {
        Value::String(s) => Value::String(cut_line(&s, opts)),
        other => {
            let text = other.to_text();
            Value::String(cut_line(&text, opts))
        }
    }
}

fn cut_line(line: &str, opts: &CutOptions) -> String {
    let default_delim = opts.delimiter.to_string();
    let output_delim = opts
        .output_delimiter
        .as_deref()
        .unwrap_or(&default_delim);

    if !opts.fields.is_empty() {
        let fields: Vec<&str> = line.split(opts.delimiter).collect();
        let field_set: HashSet<usize> = opts.fields.iter().copied().collect();

        let selected: Vec<&str> = if opts.complement {
            fields
                .iter()
                .enumerate()
                .filter(|(i, _)| !field_set.contains(&(i + 1)))
                .map(|(_, s)| *s)
                .collect()
        } else {
            opts.fields
                .iter()
                .filter_map(|&i| fields.get(i.saturating_sub(1)).copied())
                .collect()
        };

        selected.join(output_delim)
    } else if !opts.characters.is_empty() {
        let chars: Vec<char> = line.chars().collect();
        let char_set: HashSet<usize> = opts.characters.iter().copied().collect();

        let selected: String = if opts.complement {
            chars
                .iter()
                .enumerate()
                .filter(|(i, _)| !char_set.contains(&(i + 1)))
                .map(|(_, c)| *c)
                .collect()
        } else {
            opts.characters
                .iter()
                .filter_map(|&i| chars.get(i.saturating_sub(1)).copied())
                .collect()
        };

        selected
    } else if !opts.bytes.is_empty() {
        let bytes = line.as_bytes();
        let byte_set: HashSet<usize> = opts.bytes.iter().copied().collect();

        let selected: Vec<u8> = if opts.complement {
            bytes
                .iter()
                .enumerate()
                .filter(|(i, _)| !byte_set.contains(&(i + 1)))
                .map(|(_, b)| *b)
                .collect()
        } else {
            opts.bytes
                .iter()
                .filter_map(|&i| bytes.get(i.saturating_sub(1)).copied())
                .collect()
        };

        String::from_utf8_lossy(&selected).to_string()
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cut_fields() {
        let opts = CutOptions {
            delimiter: ',',
            fields: vec![1, 3],
            characters: vec![],
            bytes: vec![],
            output_delimiter: None,
            complement: false,
        };
        let result = cut_line("a,b,c,d", &opts);
        assert_eq!(result, "a,c");
    }

    #[test]
    fn test_cut_characters() {
        let opts = CutOptions {
            delimiter: '\t',
            fields: vec![],
            characters: vec![1, 3, 5],
            bytes: vec![],
            output_delimiter: None,
            complement: false,
        };
        let result = cut_line("abcde", &opts);
        assert_eq!(result, "ace");
    }

    #[test]
    fn test_parse_range() {
        assert_eq!(parse_range("1,3,5"), vec![1, 3, 5]);
        assert_eq!(parse_range("1-3"), vec![1, 2, 3]);
        assert_eq!(parse_range("1,3-5"), vec![1, 3, 4, 5]);
    }
}
