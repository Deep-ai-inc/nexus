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
            natural_cmp_case_insensitive(&ea.name, &eb.name)
        } else {
            natural_cmp(&ea.name, &eb.name)
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
        // Numeric types - always compare numerically
        (Value::Int(ia), Value::Int(ib)) => ia.cmp(ib),
        (Value::Float(fa), Value::Float(fb)) => fa.partial_cmp(fb).unwrap_or(Ordering::Equal),
        (Value::Int(ia), Value::Float(fb)) => (*ia as f64).partial_cmp(fb).unwrap_or(Ordering::Equal),
        (Value::Float(fa), Value::Int(ib)) => fa.partial_cmp(&(*ib as f64)).unwrap_or(Ordering::Equal),

        // Strings - smart comparison based on content and options
        (Value::String(sa), Value::String(sb)) => {
            if opts.numeric {
                // Explicit numeric sort requested
                let na = sa.trim().parse::<f64>().unwrap_or(f64::MAX);
                let nb = sb.trim().parse::<f64>().unwrap_or(f64::MAX);
                na.partial_cmp(&nb).unwrap_or(Ordering::Equal)
            } else if opts.ignore_case {
                natural_cmp_case_insensitive(sa, sb)
            } else {
                // Default: smart string comparison (numeric if both are numbers, else natural sort)
                smart_string_cmp(sa, sb)
            }
        }

        // FileEntry - natural sort by name, or by size/time if requested
        (Value::FileEntry(a), Value::FileEntry(b)) => {
            if opts.by_size {
                a.size.cmp(&b.size)
            } else if opts.by_time {
                a.modified.cmp(&b.modified)
            } else {
                natural_cmp(&a.name, &b.name)
            }
        }

        // Process - sort by command name with natural sort
        (Value::Process(a), Value::Process(b)) => natural_cmp(&a.command, &b.command),

        // GitCommit - sort by date
        (Value::GitCommit(a), Value::GitCommit(b)) => a.date.cmp(&b.date),

        // Path - natural sort
        (Value::Path(a), Value::Path(b)) => natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()),

        // Cross-type string/number comparison
        (Value::String(s), Value::Int(i)) => {
            if let Ok(n) = s.trim().parse::<i64>() {
                n.cmp(i)
            } else {
                Ordering::Greater
            }
        }
        (Value::Int(i), Value::String(s)) => {
            if let Ok(n) = s.trim().parse::<i64>() {
                i.cmp(&n)
            } else {
                Ordering::Less
            }
        }

        // Fallback: natural sort on text representation
        _ => natural_cmp(&a.to_text(), &b.to_text()),
    }
}

/// Smart string comparison: if both are pure numbers, compare numerically.
/// Otherwise use natural sort.
fn smart_string_cmp(a: &str, b: &str) -> Ordering {
    match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
        (Ok(na), Ok(nb)) => na.partial_cmp(&nb).unwrap_or(Ordering::Equal),
        _ => natural_cmp(a, b),
    }
}

/// Natural sort - handles embedded numbers correctly.
/// "file2" < "file10", "v1.9" < "v1.10"
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();

    loop {
        match (a_chars.peek(), b_chars.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&ac), Some(&bc)) => {
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let a_num = collect_number(&mut a_chars);
                    let b_num = collect_number(&mut b_chars);
                    match a_num.cmp(&b_num) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                }
                match ac.cmp(&bc) {
                    Ordering::Equal => {
                        a_chars.next();
                        b_chars.next();
                    }
                    other => return other,
                }
            }
        }
    }
}

/// Natural sort with case-insensitive comparison.
fn natural_cmp_case_insensitive(a: &str, b: &str) -> Ordering {
    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();

    loop {
        match (a_chars.peek(), b_chars.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(&ac), Some(&bc)) => {
                if ac.is_ascii_digit() && bc.is_ascii_digit() {
                    let a_num = collect_number(&mut a_chars);
                    let b_num = collect_number(&mut b_chars);
                    match a_num.cmp(&b_num) {
                        Ordering::Equal => continue,
                        other => return other,
                    }
                }
                match ac.to_ascii_lowercase().cmp(&bc.to_ascii_lowercase()) {
                    Ordering::Equal => {
                        a_chars.next();
                        b_chars.next();
                    }
                    other => return other,
                }
            }
        }
    }
}

/// Collect consecutive digits into a number.
fn collect_number(chars: &mut std::iter::Peekable<std::str::Chars>) -> u64 {
    let mut num: u64 = 0;
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            num = num.saturating_mul(10).saturating_add((c as u64) - ('0' as u64));
            chars.next();
        } else {
            break;
        }
    }
    num
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

        // Helper to create test process
        fn test_proc(pid: u32, command: &str, cpu: f64) -> ProcessInfo {
            ProcessInfo {
                pid, ppid: 0, user: "root".to_string(), group: None,
                command: command.to_string(), args: vec![],
                cpu_percent: cpu, mem_bytes: 1000, mem_percent: 1.0, virtual_size: 0,
                status: ProcessStatus::Running, started: None, cpu_time: 0,
                tty: None, nice: None, priority: 0, pgid: None, sid: None,
                tpgid: None, threads: None, wchan: None, flags: None,
                is_session_leader: None, has_foreground: None,
            }
        }

        let processes = Value::List(vec![
            Value::Process(Box::new(test_proc(1, "low", 10.0))),
            Value::Process(Box::new(test_proc(2, "high", 90.0))),
            Value::Process(Box::new(test_proc(3, "medium", 50.0))),
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

    #[test]
    fn test_smart_string_numeric() {
        // Pure numeric strings should sort numerically even without -n flag
        let list = Value::List(vec![
            Value::String("10".to_string()),
            Value::String("2".to_string()),
            Value::String("1".to_string()),
            Value::String("20".to_string()),
        ]);
        let opts = SortOptions {
            reverse: false,
            numeric: false, // NOT using -n flag
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
            assert_eq!(items[3].to_text(), "20");
        }
    }

    #[test]
    fn test_natural_sort_filenames() {
        // Natural sort handles embedded numbers: file2 < file10
        let list = Value::List(vec![
            Value::String("file10.txt".to_string()),
            Value::String("file2.txt".to_string()),
            Value::String("file1.txt".to_string()),
            Value::String("file20.txt".to_string()),
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
            assert_eq!(items[0].to_text(), "file1.txt");
            assert_eq!(items[1].to_text(), "file2.txt");
            assert_eq!(items[2].to_text(), "file10.txt");
            assert_eq!(items[3].to_text(), "file20.txt");
        }
    }

    #[test]
    fn test_natural_sort_versions() {
        // Version-like strings: v1.9 < v1.10
        let list = Value::List(vec![
            Value::String("v1.10".to_string()),
            Value::String("v1.2".to_string()),
            Value::String("v1.9".to_string()),
            Value::String("v2.0".to_string()),
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
            assert_eq!(items[0].to_text(), "v1.2");
            assert_eq!(items[1].to_text(), "v1.9");
            assert_eq!(items[2].to_text(), "v1.10");
            assert_eq!(items[3].to_text(), "v2.0");
        }
    }

    #[test]
    fn test_file_entry_natural_sort() {
        use nexus_api::{FileEntry, FileType};
        use std::path::PathBuf;

        fn test_file(name: &str) -> FileEntry {
            FileEntry {
                name: name.to_string(),
                path: PathBuf::from(name),
                file_type: FileType::File,
                size: 0,
                modified: None,
                accessed: None,
                created: None,
                permissions: 0o644,
                is_hidden: false,
                is_symlink: false,
                symlink_target: None,
                uid: None,
                gid: None,
                owner: None,
                group: None,
                nlink: None,
            }
        }

        let list = Value::List(vec![
            Value::FileEntry(Box::new(test_file("doc10.pdf"))),
            Value::FileEntry(Box::new(test_file("doc2.pdf"))),
            Value::FileEntry(Box::new(test_file("doc1.pdf"))),
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
            match (&items[0], &items[1], &items[2]) {
                (Value::FileEntry(a), Value::FileEntry(b), Value::FileEntry(c)) => {
                    assert_eq!(a.name, "doc1.pdf");
                    assert_eq!(b.name, "doc2.pdf");
                    assert_eq!(c.name, "doc10.pdf");
                }
                _ => panic!("Expected FileEntry values"),
            }
        }
    }
}
