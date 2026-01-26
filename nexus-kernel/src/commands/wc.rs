//! The `wc` command - word, line, character, and byte count.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::fs;
use std::path::PathBuf;

pub struct WcCommand;

struct WcOptions {
    lines: bool,
    words: bool,
    chars: bool,
    bytes: bool,
    files: Vec<PathBuf>,
}

impl WcOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = WcOptions {
            lines: false,
            words: false,
            chars: false,
            bytes: false,
            files: Vec::new(),
        };

        let mut has_flags = false;

        for arg in args {
            if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") {
                has_flags = true;
                for c in arg[1..].chars() {
                    match c {
                        'l' => opts.lines = true,
                        'w' => opts.words = true,
                        'm' => opts.chars = true,
                        'c' => opts.bytes = true,
                        _ => {}
                    }
                }
            } else if arg == "--lines" {
                has_flags = true;
                opts.lines = true;
            } else if arg == "--words" {
                has_flags = true;
                opts.words = true;
            } else if arg == "--chars" {
                has_flags = true;
                opts.chars = true;
            } else if arg == "--bytes" {
                has_flags = true;
                opts.bytes = true;
            } else if !arg.starts_with('-') {
                opts.files.push(PathBuf::from(arg));
            }
        }

        // Default to all if no flags specified
        if !has_flags {
            opts.lines = true;
            opts.words = true;
            opts.bytes = true;
        }

        opts
    }
}

impl NexusCommand for WcCommand {
    fn name(&self) -> &'static str {
        "wc"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = WcOptions::parse(args);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(wc_value(stdin_value, &opts));
        }

        if !opts.files.is_empty() {
            let mut results = Vec::new();

            for path in &opts.files {
                let resolved = if path.is_absolute() {
                    path.clone()
                } else {
                    ctx.state.cwd.join(path)
                };

                match fs::read_to_string(&resolved) {
                    Ok(content) => {
                        let counts = count_string(&content, &opts);
                        results.push((path.display().to_string(), counts));
                    }
                    Err(e) => {
                        return Err(anyhow::anyhow!("{}: {}", path.display(), e));
                    }
                }
            }

            if results.len() == 1 {
                let (_, counts) = &results[0];
                return Ok(format_counts(counts, &opts));
            } else {
                // Multiple files: return table
                let mut columns = Vec::new();
                if opts.lines {
                    columns.push("lines".to_string());
                }
                if opts.words {
                    columns.push("words".to_string());
                }
                if opts.chars {
                    columns.push("chars".to_string());
                }
                if opts.bytes {
                    columns.push("bytes".to_string());
                }
                columns.push("file".to_string());

                let rows: Vec<Vec<Value>> = results
                    .iter()
                    .map(|(name, counts)| {
                        let mut row = Vec::new();
                        if opts.lines {
                            row.push(Value::Int(counts.0 as i64));
                        }
                        if opts.words {
                            row.push(Value::Int(counts.1 as i64));
                        }
                        if opts.chars {
                            row.push(Value::Int(counts.2 as i64));
                        }
                        if opts.bytes {
                            row.push(Value::Int(counts.3 as i64));
                        }
                        row.push(Value::String(name.clone()));
                        row
                    })
                    .collect();

                return Ok(Value::Table { columns, rows });
            }
        }

        Ok(Value::Unit)
    }
}

fn wc_value(value: Value, opts: &WcOptions) -> Value {
    match value {
        Value::List(items) => {
            // For lists, count is the number of items
            if opts.lines && !opts.words && !opts.chars && !opts.bytes {
                Value::Int(items.len() as i64)
            } else {
                let text = items
                    .iter()
                    .map(|v| v.to_text())
                    .collect::<Vec<_>>()
                    .join("\n");
                let counts = count_string(&text, opts);
                format_counts(&counts, opts)
            }
        }
        Value::Table { rows, .. } => {
            if opts.lines && !opts.words && !opts.chars && !opts.bytes {
                Value::Int(rows.len() as i64)
            } else {
                Value::Int(rows.len() as i64)
            }
        }
        Value::Record(entries) => {
            // For records, count entries as lines
            if opts.lines && !opts.words && !opts.chars && !opts.bytes {
                Value::Int(entries.len() as i64)
            } else {
                // Convert to text (key=value lines) and count
                let text = entries
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v.to_text()))
                    .collect::<Vec<_>>()
                    .join("\n");
                let counts = count_string(&text, opts);
                format_counts(&counts, opts)
            }
        }
        Value::String(s) => {
            let counts = count_string(&s, opts);
            format_counts(&counts, opts)
        }
        Value::Bytes(bytes) => {
            if opts.bytes && !opts.lines && !opts.words && !opts.chars {
                Value::Int(bytes.len() as i64)
            } else {
                let s = String::from_utf8_lossy(&bytes);
                let counts = count_string(&s, opts);
                format_counts(&counts, opts)
            }
        }
        Value::Media { data, .. } => {
            // Treat media as raw bytes
            if opts.bytes && !opts.lines && !opts.words && !opts.chars {
                Value::Int(data.len() as i64)
            } else {
                // For lines/words/chars, try to interpret as text (lossy)
                let s = String::from_utf8_lossy(&data);
                let counts = count_string(&s, opts);
                format_counts(&counts, opts)
            }
        }
        _ => Value::Int(0),
    }
}

fn count_string(s: &str, opts: &WcOptions) -> (usize, usize, usize, usize) {
    let lines = if opts.lines { s.lines().count() } else { 0 };
    let words = if opts.words {
        s.split_whitespace().count()
    } else {
        0
    };
    let chars = if opts.chars { s.chars().count() } else { 0 };
    let bytes = if opts.bytes { s.len() } else { 0 };
    (lines, words, chars, bytes)
}

fn format_counts(counts: &(usize, usize, usize, usize), opts: &WcOptions) -> Value {
    let mut parts = Vec::new();
    if opts.lines {
        parts.push(counts.0);
    }
    if opts.words {
        parts.push(counts.1);
    }
    if opts.chars {
        parts.push(counts.2);
    }
    if opts.bytes {
        parts.push(counts.3);
    }

    if parts.len() == 1 {
        Value::Int(parts[0] as i64)
    } else {
        Value::List(parts.into_iter().map(|n| Value::Int(n as i64)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_lines() {
        let opts = WcOptions {
            lines: true,
            words: false,
            chars: false,
            bytes: false,
            files: vec![],
        };
        let counts = count_string("hello\nworld\n", &opts);
        assert_eq!(counts.0, 2);
    }

    #[test]
    fn test_count_words() {
        let opts = WcOptions {
            lines: false,
            words: true,
            chars: false,
            bytes: false,
            files: vec![],
        };
        let counts = count_string("hello world foo", &opts);
        assert_eq!(counts.1, 3);
    }

    #[test]
    fn test_wc_list() {
        let opts = WcOptions {
            lines: true,
            words: false,
            chars: false,
            bytes: false,
            files: vec![],
        };
        let list = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let result = wc_value(list, &opts);
        assert_eq!(result, Value::Int(3));
    }
}
