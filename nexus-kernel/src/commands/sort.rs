//! The `sort` command - sort lines or structured data.
//!
//! Supports typed data with field-based sorting:
//! - `ps | sort --by cpu` - sort processes by CPU usage
//! - `ls | sort --by size` - sort files by size
//! - `git log | sort --by date` - sort commits by date

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::cmp::Ordering;

pub struct SortCommand;

struct SortOptions {
    reverse: bool,
    numeric: bool,
    by_size: bool,
    by_time: bool,
    ignore_case: bool,
    unique: bool,
    by_field: Option<String>,  // --by <field> for typed data
}

impl SortOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = SortOptions {
            reverse: false,
            numeric: false,
            by_size: false,
            by_time: false,
            ignore_case: false,
            unique: false,
            by_field: None,
        };

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            if arg.starts_with('-') && !arg.starts_with("--") {
                for c in arg[1..].chars() {
                    match c {
                        'r' => opts.reverse = true,
                        'n' => opts.numeric = true,
                        'S' => opts.by_size = true,
                        't' => opts.by_time = true,
                        'f' => opts.ignore_case = true,
                        'u' => opts.unique = true,
                        _ => {}
                    }
                }
            } else {
                match arg.as_str() {
                    "--reverse" => opts.reverse = true,
                    "--numeric-sort" => opts.numeric = true,
                    "--size" => opts.by_size = true,
                    "--time" => opts.by_time = true,
                    "--ignore-case" => opts.ignore_case = true,
                    "--unique" => opts.unique = true,
                    "--by" | "-k" => {
                        if i + 1 < args.len() {
                            opts.by_field = Some(args[i + 1].clone());
                            i += 1;
                        }
                    }
                    _ => {
                        // Treat bare argument as field name for --by
                        if !arg.starts_with('-') && opts.by_field.is_none() {
                            opts.by_field = Some(arg.clone());
                        }
                    }
                }
            }
            i += 1;
        }

        opts
    }
}

impl NexusCommand for SortCommand {
    fn name(&self) -> &'static str {
        "sort"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = SortOptions::parse(args);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(sort_value(stdin_value, &opts));
        }

        Ok(Value::Unit)
    }
}

fn sort_value(value: Value, opts: &SortOptions) -> Value {
    match value {
        Value::List(mut items) => {
            // Check if all items are FileEntry
            let all_file_entries = items
                .iter()
                .all(|v| matches!(v, Value::FileEntry(_)));

            if all_file_entries {
                sort_file_entries(&mut items, opts);
            } else {
                sort_generic(&mut items, opts);
            }

            if opts.unique {
                items.dedup_by(|a, b| a.to_text() == b.to_text());
            }

            Value::List(items)
        }
        Value::Table { columns, mut rows } => {
            // Sort table rows by first column
            rows.sort_by(|a, b| {
                let cmp = compare_values(
                    a.first().unwrap_or(&Value::Unit),
                    b.first().unwrap_or(&Value::Unit),
                    opts,
                );
                if opts.reverse {
                    cmp.reverse()
                } else {
                    cmp
                }
            });

            if opts.unique {
                rows.dedup_by(|a, b| {
                    a.first().map(|v| v.to_text()) == b.first().map(|v| v.to_text())
                });
            }

            Value::Table { columns, rows }
        }
        Value::String(s) => {
            let mut lines: Vec<&str> = s.lines().collect();
            lines.sort_by(|a, b| {
                let cmp = if opts.ignore_case {
                    a.to_lowercase().cmp(&b.to_lowercase())
                } else if opts.numeric {
                    let na = a.trim().parse::<f64>().unwrap_or(f64::MAX);
                    let nb = b.trim().parse::<f64>().unwrap_or(f64::MAX);
                    na.partial_cmp(&nb).unwrap_or(Ordering::Equal)
                } else {
                    a.cmp(b)
                };
                if opts.reverse {
                    cmp.reverse()
                } else {
                    cmp
                }
            });

            if opts.unique {
                lines.dedup();
            }

            Value::String(lines.join("\n"))
        }
        other => other,
    }
}

fn sort_file_entries(items: &mut [Value], opts: &SortOptions) {
    items.sort_by(|a, b| {
        let (ea, eb) = match (a, b) {
            (Value::FileEntry(ea), Value::FileEntry(eb)) => (ea.as_ref(), eb.as_ref()),
            _ => return Ordering::Equal,
        };

        let cmp = if opts.by_size {
            ea.size.cmp(&eb.size)
        } else if opts.by_time {
            ea.modified.cmp(&eb.modified)
        } else if opts.ignore_case {
            ea.name.to_lowercase().cmp(&eb.name.to_lowercase())
        } else {
            ea.name.cmp(&eb.name)
        };

        if opts.reverse {
            cmp.reverse()
        } else {
            cmp
        }
    });
}

fn sort_generic(items: &mut [Value], opts: &SortOptions) {
    items.sort_by(|a, b| {
        let cmp = if let Some(ref field) = opts.by_field {
            // Sort by specific field for typed data
            compare_by_field(a, b, field, opts)
        } else {
            compare_values(a, b, opts)
        };
        if opts.reverse {
            cmp.reverse()
        } else {
            cmp
        }
    });
}

/// Compare two values by extracting a specific field.
fn compare_by_field(a: &Value, b: &Value, field: &str, opts: &SortOptions) -> Ordering {
    let val_a = a.get_field(field);
    let val_b = b.get_field(field);

    match (val_a, val_b) {
        (Some(va), Some(vb)) => compare_values(&va, &vb, opts),
        (Some(_), None) => Ordering::Less,    // Values with field come first
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn compare_values(a: &Value, b: &Value, opts: &SortOptions) -> Ordering {
    match (a, b) {
        (Value::Int(ia), Value::Int(ib)) => ia.cmp(ib),
        (Value::Float(fa), Value::Float(fb)) => fa.partial_cmp(fb).unwrap_or(Ordering::Equal),
        (Value::Int(ia), Value::Float(fb)) => (*ia as f64).partial_cmp(fb).unwrap_or(Ordering::Equal),
        (Value::Float(fa), Value::Int(ib)) => fa.partial_cmp(&(*ib as f64)).unwrap_or(Ordering::Equal),
        (Value::String(sa), Value::String(sb)) => {
            if opts.numeric {
                let na = sa.trim().parse::<f64>().unwrap_or(f64::MAX);
                let nb = sb.trim().parse::<f64>().unwrap_or(f64::MAX);
                na.partial_cmp(&nb).unwrap_or(Ordering::Equal)
            } else if opts.ignore_case {
                sa.to_lowercase().cmp(&sb.to_lowercase())
            } else {
                sa.cmp(sb)
            }
        }
        _ => a.to_text().cmp(&b.to_text()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sort_list() {
        let list = Value::List(vec![
            Value::String("banana".to_string()),
            Value::String("apple".to_string()),
            Value::String("cherry".to_string()),
        ]);
        let opts = SortOptions {
            reverse: false,
            numeric: false,
            by_size: false,
            by_time: false,
            ignore_case: false,
            unique: false,
            by_field: None,
        };
        let result = sort_value(list, &opts);
        if let Value::List(items) = result {
            assert_eq!(items[0].to_text(), "apple");
            assert_eq!(items[1].to_text(), "banana");
            assert_eq!(items[2].to_text(), "cherry");
        }
    }

    #[test]
    fn test_sort_reverse() {
        let list = Value::List(vec![Value::Int(1), Value::Int(3), Value::Int(2)]);
        let opts = SortOptions {
            reverse: true,
            numeric: false,
            by_size: false,
            by_time: false,
            ignore_case: false,
            unique: false,
            by_field: None,
        };
        let result = sort_value(list, &opts);
        if let Value::List(items) = result {
            assert_eq!(items[0], Value::Int(3));
            assert_eq!(items[1], Value::Int(2));
            assert_eq!(items[2], Value::Int(1));
        }
    }

    #[test]
    fn test_sort_numeric() {
        let list = Value::List(vec![
            Value::String("10".to_string()),
            Value::String("2".to_string()),
            Value::String("1".to_string()),
        ]);
        let opts = SortOptions {
            reverse: false,
            numeric: true,
            by_size: false,
            by_time: false,
            ignore_case: false,
            unique: false,
            by_field: None,
        };
        let result = sort_value(list, &opts);
        if let Value::List(items) = result {
            assert_eq!(items[0].to_text(), "1");
            assert_eq!(items[1].to_text(), "2");
            assert_eq!(items[2].to_text(), "10");
        }
    }

    #[test]
    fn test_sort_process_by_cpu() {
        use nexus_api::{ProcessInfo, ProcessStatus};

        let processes = Value::List(vec![
            Value::Process(Box::new(ProcessInfo {
                pid: 1,
                ppid: 0,
                user: "root".to_string(),
                command: "low".to_string(),
                args: vec![],
                cpu_percent: 10.0,
                mem_bytes: 1000,
                mem_percent: 1.0,
                status: ProcessStatus::Running,
                started: None,
            })),
            Value::Process(Box::new(ProcessInfo {
                pid: 2,
                ppid: 0,
                user: "root".to_string(),
                command: "high".to_string(),
                args: vec![],
                cpu_percent: 90.0,
                mem_bytes: 2000,
                mem_percent: 2.0,
                status: ProcessStatus::Running,
                started: None,
            })),
            Value::Process(Box::new(ProcessInfo {
                pid: 3,
                ppid: 0,
                user: "root".to_string(),
                command: "medium".to_string(),
                args: vec![],
                cpu_percent: 50.0,
                mem_bytes: 1500,
                mem_percent: 1.5,
                status: ProcessStatus::Running,
                started: None,
            })),
        ]);

        // Sort by CPU ascending
        let opts = SortOptions {
            reverse: false,
            numeric: false,
            by_size: false,
            by_time: false,
            ignore_case: false,
            unique: false,
            by_field: Some("cpu".to_string()),
        };
        let result = sort_value(processes.clone(), &opts);

        if let Value::List(items) = result {
            // Should be sorted: low (10%), medium (50%), high (90%)
            match (&items[0], &items[1], &items[2]) {
                (Value::Process(a), Value::Process(b), Value::Process(c)) => {
                    assert_eq!(a.command, "low");
                    assert_eq!(b.command, "medium");
                    assert_eq!(c.command, "high");
                }
                _ => panic!("Expected Process values"),
            }
        } else {
            panic!("Expected List");
        }

        // Sort by CPU descending
        let opts = SortOptions {
            reverse: true,
            numeric: false,
            by_size: false,
            by_time: false,
            ignore_case: false,
            unique: false,
            by_field: Some("cpu".to_string()),
        };
        let result = sort_value(processes, &opts);

        if let Value::List(items) = result {
            match (&items[0], &items[1], &items[2]) {
                (Value::Process(a), Value::Process(b), Value::Process(c)) => {
                    assert_eq!(a.command, "high");
                    assert_eq!(b.command, "medium");
                    assert_eq!(c.command, "low");
                }
                _ => panic!("Expected Process values"),
            }
        }
    }
}
