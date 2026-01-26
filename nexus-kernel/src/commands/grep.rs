//! The `grep` command - search for patterns.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use regex::Regex;
use std::fs;
use std::path::PathBuf;

pub struct GrepCommand;

struct GrepOptions {
    pattern: Option<String>,
    invert: bool,
    ignore_case: bool,
    count: bool,
    line_numbers: bool,
    only_matching: bool,
    fixed_string: bool,
    files: Vec<PathBuf>,
}

impl GrepOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = GrepOptions {
            pattern: None,
            invert: false,
            ignore_case: false,
            count: false,
            line_numbers: false,
            only_matching: false,
            fixed_string: false,
            files: Vec::new(),
        };

        let mut positional = Vec::new();

        for arg in args {
            if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") {
                for c in arg[1..].chars() {
                    match c {
                        'v' => opts.invert = true,
                        'i' => opts.ignore_case = true,
                        'c' => opts.count = true,
                        'n' => opts.line_numbers = true,
                        'o' => opts.only_matching = true,
                        'F' => opts.fixed_string = true,
                        'E' => {} // Extended regex is default
                        _ => {}
                    }
                }
            } else if arg.starts_with("--") {
                match arg.as_str() {
                    "--invert-match" => opts.invert = true,
                    "--ignore-case" => opts.ignore_case = true,
                    "--count" => opts.count = true,
                    "--line-number" => opts.line_numbers = true,
                    "--only-matching" => opts.only_matching = true,
                    "--fixed-strings" => opts.fixed_string = true,
                    _ => {}
                }
            } else {
                positional.push(arg.clone());
            }
        }

        if !positional.is_empty() {
            opts.pattern = Some(positional.remove(0));
        }
        for p in positional {
            opts.files.push(PathBuf::from(p));
        }

        opts
    }

    fn matches(&self, text: &str) -> bool {
        let pattern = match &self.pattern {
            Some(p) => p,
            None => return true,
        };

        let matched = if self.fixed_string {
            if self.ignore_case {
                text.to_lowercase().contains(&pattern.to_lowercase())
            } else {
                text.contains(pattern)
            }
        } else {
            let regex_pattern = if self.ignore_case {
                format!("(?i){}", pattern)
            } else {
                pattern.clone()
            };
            match Regex::new(&regex_pattern) {
                Ok(re) => re.is_match(text),
                Err(_) => text.contains(pattern),
            }
        };

        if self.invert {
            !matched
        } else {
            matched
        }
    }
}

impl NexusCommand for GrepCommand {
    fn name(&self) -> &'static str {
        "grep"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = GrepOptions::parse(args);

        if opts.pattern.is_none() {
            return Err(anyhow::anyhow!("Usage: grep PATTERN [FILE...]"));
        }

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(grep_value(stdin_value, &opts));
        }

        if !opts.files.is_empty() {
            let mut all_matches = Vec::new();

            for path in &opts.files {
                let resolved = if path.is_absolute() {
                    path.clone()
                } else {
                    ctx.state.cwd.join(path)
                };

                match fs::read_to_string(&resolved) {
                    Ok(content) => {
                        for (i, line) in content.lines().enumerate() {
                            if opts.matches(line) {
                                if opts.line_numbers {
                                    all_matches.push(format!("{}:{}", i + 1, line));
                                } else {
                                    all_matches.push(line.to_string());
                                }
                            }
                        }
                    }
                    Err(e) => {
                        return Err(anyhow::anyhow!("{}: {}", path.display(), e));
                    }
                }
            }

            if opts.count {
                return Ok(Value::Int(all_matches.len() as i64));
            }

            return Ok(Value::List(
                all_matches.into_iter().map(Value::String).collect(),
            ));
        }

        Ok(Value::Unit)
    }
}

fn grep_value(value: Value, opts: &GrepOptions) -> Value {
    match value {
        Value::List(items) => {
            let filtered: Vec<Value> = items
                .into_iter()
                .enumerate()
                .filter_map(|(i, item)| {
                    let text = match &item {
                        Value::FileEntry(entry) => entry.name.clone(),
                        Value::String(s) => s.clone(),
                        other => other.to_text(),
                    };

                    if opts.matches(&text) {
                        if opts.line_numbers {
                            Some(Value::String(format!("{}:{}", i + 1, text)))
                        } else {
                            Some(item)
                        }
                    } else {
                        None
                    }
                })
                .collect();

            if opts.count {
                Value::Int(filtered.len() as i64)
            } else {
                Value::List(filtered)
            }
        }
        Value::Table { columns, rows } => {
            let filtered_rows: Vec<Vec<Value>> = rows
                .into_iter()
                .filter(|row| {
                    row.iter()
                        .any(|cell| opts.matches(&cell.to_text()))
                })
                .collect();

            if opts.count {
                Value::Int(filtered_rows.len() as i64)
            } else {
                Value::Table {
                    columns,
                    rows: filtered_rows,
                }
            }
        }
        Value::String(s) => {
            let lines: Vec<&str> = s
                .lines()
                .filter(|line| opts.matches(line))
                .collect();

            if opts.count {
                Value::Int(lines.len() as i64)
            } else {
                Value::String(lines.join("\n"))
            }
        }
        Value::Media { data, .. } => {
            // Treat media as bytes, grep through lines (lossy UTF-8)
            let s = String::from_utf8_lossy(&data);
            let lines: Vec<&str> = s
                .lines()
                .filter(|line| opts.matches(line))
                .collect();

            if opts.count {
                Value::Int(lines.len() as i64)
            } else {
                Value::String(lines.join("\n"))
            }
        }
        other => {
            if opts.matches(&other.to_text()) {
                other
            } else {
                Value::Unit
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grep_list() {
        let list = Value::List(vec![
            Value::String("apple".to_string()),
            Value::String("banana".to_string()),
            Value::String("apricot".to_string()),
        ]);
        let opts = GrepOptions {
            pattern: Some("ap".to_string()),
            invert: false,
            ignore_case: false,
            count: false,
            line_numbers: false,
            only_matching: false,
            fixed_string: false,
            files: vec![],
        };
        let result = grep_value(list, &opts);
        if let Value::List(items) = result {
            assert_eq!(items.len(), 2);
        }
    }

    #[test]
    fn test_grep_invert() {
        let list = Value::List(vec![
            Value::String("apple".to_string()),
            Value::String("banana".to_string()),
        ]);
        let opts = GrepOptions {
            pattern: Some("apple".to_string()),
            invert: true,
            ignore_case: false,
            count: false,
            line_numbers: false,
            only_matching: false,
            fixed_string: false,
            files: vec![],
        };
        let result = grep_value(list, &opts);
        if let Value::List(items) = result {
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].to_text(), "banana");
        }
    }

    #[test]
    fn test_grep_count() {
        let list = Value::List(vec![
            Value::String("apple".to_string()),
            Value::String("banana".to_string()),
            Value::String("apricot".to_string()),
        ]);
        let opts = GrepOptions {
            pattern: Some("a".to_string()),
            invert: false,
            ignore_case: false,
            count: true,
            line_numbers: false,
            only_matching: false,
            fixed_string: false,
            files: vec![],
        };
        let result = grep_value(list, &opts);
        assert_eq!(result, Value::Int(3));
    }
}
