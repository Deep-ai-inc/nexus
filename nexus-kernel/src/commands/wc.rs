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
                let (name, counts) = &results[0];
                return Ok(format_counts_record(counts, &opts, Some(name)));
            } else {
                // Multiple files: return table with totals row
                let mut columns: Vec<&str> = Vec::new();
                if opts.lines {
                    columns.push("lines");
                }
                if opts.words {
                    columns.push("words");
                }
                if opts.chars {
                    columns.push("chars");
                }
                if opts.bytes {
                    columns.push("bytes");
                }
                columns.push("file");

                let mut total = (0usize, 0usize, 0usize, 0usize);
                let mut rows: Vec<Vec<Value>> = results
                    .iter()
                    .map(|(name, counts)| {
                        total.0 += counts.0;
                        total.1 += counts.1;
                        total.2 += counts.2;
                        total.3 += counts.3;
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

                // Add totals row
                let mut totals_row = Vec::new();
                if opts.lines {
                    totals_row.push(Value::Int(total.0 as i64));
                }
                if opts.words {
                    totals_row.push(Value::Int(total.1 as i64));
                }
                if opts.chars {
                    totals_row.push(Value::Int(total.2 as i64));
                }
                if opts.bytes {
                    totals_row.push(Value::Int(total.3 as i64));
                }
                totals_row.push(Value::String("total".to_string()));
                rows.push(totals_row);

                return Ok(Value::table(columns, rows));
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
    format_counts_record(counts, opts, None)
}

fn format_counts_record(
    counts: &(usize, usize, usize, usize),
    opts: &WcOptions,
    filename: Option<&str>,
) -> Value {
    let mut fields = Vec::new();
    if opts.lines {
        fields.push(("lines".to_string(), Value::Int(counts.0 as i64)));
    }
    if opts.words {
        fields.push(("words".to_string(), Value::Int(counts.1 as i64)));
    }
    if opts.chars {
        fields.push(("chars".to_string(), Value::Int(counts.2 as i64)));
    }
    if opts.bytes {
        fields.push(("bytes".to_string(), Value::Int(counts.3 as i64)));
    }
    if let Some(name) = filename {
        fields.push(("file".to_string(), Value::String(name.to_string())));
    }

    // Single metric with no filename: return bare int for pipeline ergonomics
    if fields.len() == 1 {
        return fields.into_iter().next().unwrap().1;
    }

    Value::Record(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // WcOptions::parse tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_no_args() {
        let opts = WcOptions::parse(&[]);
        // Default: lines, words, bytes (not chars)
        assert!(opts.lines);
        assert!(opts.words);
        assert!(!opts.chars);
        assert!(opts.bytes);
        assert!(opts.files.is_empty());
    }

    #[test]
    fn test_parse_short_flags_combined() {
        let opts = WcOptions::parse(&["-lwc".to_string()]);
        assert!(opts.lines);
        assert!(opts.words);
        assert!(opts.bytes); // -c is bytes
        assert!(!opts.chars);
    }

    #[test]
    fn test_parse_short_flags_individual() {
        let opts = WcOptions::parse(&["-l".to_string(), "-w".to_string()]);
        assert!(opts.lines);
        assert!(opts.words);
        assert!(!opts.bytes);
        assert!(!opts.chars);
    }

    #[test]
    fn test_parse_long_flags() {
        let opts = WcOptions::parse(&["--lines".to_string(), "--chars".to_string()]);
        assert!(opts.lines);
        assert!(!opts.words);
        assert!(opts.chars);
        assert!(!opts.bytes);
    }

    #[test]
    fn test_parse_chars_flag() {
        let opts = WcOptions::parse(&["-m".to_string()]);
        assert!(opts.chars);
        assert!(!opts.bytes);
    }

    #[test]
    fn test_parse_with_files() {
        let opts = WcOptions::parse(&["-l".to_string(), "file1.txt".to_string(), "file2.txt".to_string()]);
        assert!(opts.lines);
        assert_eq!(opts.files.len(), 2);
        assert_eq!(opts.files[0], PathBuf::from("file1.txt"));
        assert_eq!(opts.files[1], PathBuf::from("file2.txt"));
    }

    #[test]
    fn test_parse_ignores_unknown_flags() {
        let opts = WcOptions::parse(&["-xyz".to_string()]);
        // Unknown flags in combination are ignored
        assert!(!opts.lines);
        assert!(!opts.words);
    }

    // -------------------------------------------------------------------------
    // count_string tests
    // -------------------------------------------------------------------------

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
    fn test_count_chars() {
        let opts = WcOptions {
            lines: false,
            words: false,
            chars: true,
            bytes: false,
            files: vec![],
        };
        let counts = count_string("héllo", &opts);
        assert_eq!(counts.2, 5); // 5 characters
    }

    #[test]
    fn test_count_bytes() {
        let opts = WcOptions {
            lines: false,
            words: false,
            chars: false,
            bytes: true,
            files: vec![],
        };
        let counts = count_string("héllo", &opts);
        assert_eq!(counts.3, 6); // 6 bytes (é is 2 bytes in UTF-8)
    }

    #[test]
    fn test_count_all() {
        let opts = WcOptions {
            lines: true,
            words: true,
            chars: true,
            bytes: true,
            files: vec![],
        };
        let counts = count_string("hello world\n", &opts);
        assert_eq!(counts.0, 1); // 1 line
        assert_eq!(counts.1, 2); // 2 words
        assert_eq!(counts.2, 12); // 12 chars
        assert_eq!(counts.3, 12); // 12 bytes
    }

    #[test]
    fn test_count_empty_string() {
        let opts = WcOptions {
            lines: true,
            words: true,
            chars: true,
            bytes: true,
            files: vec![],
        };
        let counts = count_string("", &opts);
        assert_eq!(counts.0, 0);
        assert_eq!(counts.1, 0);
        assert_eq!(counts.2, 0);
        assert_eq!(counts.3, 0);
    }

    // -------------------------------------------------------------------------
    // format_counts tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_format_counts_single_metric() {
        let opts = WcOptions {
            lines: true,
            words: false,
            chars: false,
            bytes: false,
            files: vec![],
        };
        let result = format_counts(&(10, 0, 0, 0), &opts);
        assert_eq!(result, Value::Int(10));
    }

    #[test]
    fn test_format_counts_multiple_metrics() {
        let opts = WcOptions {
            lines: true,
            words: true,
            chars: false,
            bytes: false,
            files: vec![],
        };
        let result = format_counts(&(10, 20, 0, 0), &opts);
        if let Value::Record(fields) = result {
            assert_eq!(fields.len(), 2);
            assert!(fields.iter().any(|(k, v)| k == "lines" && *v == Value::Int(10)));
            assert!(fields.iter().any(|(k, v)| k == "words" && *v == Value::Int(20)));
        } else {
            panic!("Expected Record");
        }
    }

    #[test]
    fn test_format_counts_record_with_filename() {
        let opts = WcOptions {
            lines: true,
            words: false,
            chars: false,
            bytes: false,
            files: vec![],
        };
        let result = format_counts_record(&(10, 0, 0, 0), &opts, Some("test.txt"));
        if let Value::Record(fields) = result {
            assert_eq!(fields.len(), 2);
            assert!(fields.iter().any(|(k, _)| k == "file"));
        } else {
            panic!("Expected Record with filename");
        }
    }

    // -------------------------------------------------------------------------
    // wc_value tests
    // -------------------------------------------------------------------------

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

    #[test]
    fn test_wc_table() {
        let opts = WcOptions {
            lines: true,
            words: false,
            chars: false,
            bytes: false,
            files: vec![],
        };
        let table = Value::table(
            vec!["col1", "col2"],
            vec![
                vec![Value::Int(1), Value::Int(2)],
                vec![Value::Int(3), Value::Int(4)],
            ],
        );
        let result = wc_value(table, &opts);
        assert_eq!(result, Value::Int(2)); // 2 rows
    }

    #[test]
    fn test_wc_string() {
        let opts = WcOptions {
            lines: true,
            words: true,
            chars: false,
            bytes: false,
            files: vec![],
        };
        let result = wc_value(Value::String("hello world\nfoo bar\n".to_string()), &opts);
        if let Value::Record(fields) = result {
            assert!(fields.iter().any(|(k, v)| k == "lines" && *v == Value::Int(2)));
            assert!(fields.iter().any(|(k, v)| k == "words" && *v == Value::Int(4)));
        } else {
            panic!("Expected Record");
        }
    }

    #[test]
    fn test_wc_bytes() {
        let opts = WcOptions {
            lines: false,
            words: false,
            chars: false,
            bytes: true,
            files: vec![],
        };
        let result = wc_value(Value::Bytes(vec![1, 2, 3, 4, 5]), &opts);
        assert_eq!(result, Value::Int(5));
    }

    #[test]
    fn test_wc_record() {
        let opts = WcOptions {
            lines: true,
            words: false,
            chars: false,
            bytes: false,
            files: vec![],
        };
        let record = Value::Record(vec![
            ("key1".to_string(), Value::String("val1".to_string())),
            ("key2".to_string(), Value::String("val2".to_string())),
        ]);
        let result = wc_value(record, &opts);
        assert_eq!(result, Value::Int(2)); // 2 entries
    }
}
