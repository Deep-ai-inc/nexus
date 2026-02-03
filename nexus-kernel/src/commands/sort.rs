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
    by_fields: Vec<String>,    // --by field1,field2 for multi-key typed data
    by_key: Option<usize>,     // --key N for column-index sorting (1-based)
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
            by_fields: Vec::new(),
            by_key: None,
        };

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            if arg.starts_with('-') && !arg.starts_with("--") {
                // Check for -k N (short flag with separate arg)
                if arg == "-k" {
                    if i + 1 < args.len() {
                        if let Ok(n) = args[i + 1].parse::<usize>() {
                            opts.by_key = Some(n);
                        } else {
                            // Not a number — treat as field name
                            opts.by_fields = args[i + 1].split(',').map(|s| s.to_string()).collect();
                        }
                        i += 1;
                    }
                } else {
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
                }
            } else {
                match arg.as_str() {
                    "--reverse" => opts.reverse = true,
                    "--numeric-sort" => opts.numeric = true,
                    "--size" => opts.by_size = true,
                    "--time" => opts.by_time = true,
                    "--ignore-case" => opts.ignore_case = true,
                    "--unique" => opts.unique = true,
                    "--key" => {
                        if i + 1 < args.len() {
                            if let Ok(n) = args[i + 1].parse::<usize>() {
                                opts.by_key = Some(n);
                            }
                            i += 1;
                        }
                    }
                    "--by" => {
                        if i + 1 < args.len() {
                            opts.by_fields = args[i + 1].split(',').map(|s| s.to_string()).collect();
                            i += 1;
                        }
                    }
                    _ => {
                        // Treat bare argument as field name for --by
                        if !arg.starts_with('-') && opts.by_fields.is_empty() {
                            opts.by_fields = arg.split(',').map(|s| s.to_string()).collect();
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
            // Determine which column(s) to sort by
            let sort_col = if let Some(key) = opts.by_key {
                // --key N is 1-based
                key.saturating_sub(1)
            } else if !opts.by_fields.is_empty() {
                // --by field_name: find the column index by name
                columns
                    .iter()
                    .position(|c| c.name.eq_ignore_ascii_case(&opts.by_fields[0]))
                    .unwrap_or(0)
            } else {
                0
            };

            rows.sort_by(|a, b| {
                let cmp = compare_values(
                    a.get(sort_col).unwrap_or(&Value::Unit),
                    b.get(sort_col).unwrap_or(&Value::Unit),
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
        let cmp = if !opts.by_fields.is_empty() {
            // Multi-key sort: compare by each field in order, tiebreak with next field
            let mut result = Ordering::Equal;
            for field in &opts.by_fields {
                result = compare_by_field(a, b, field, opts);
                if result != Ordering::Equal {
                    break;
                }
            }
            result
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
            by_fields: vec![],
            by_key: None,
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
            by_fields: vec![],
            by_key: None,
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
            by_fields: vec![],
            by_key: None,
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
            by_fields: vec!["cpu".to_string()],
            by_key: None,
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
            by_fields: vec!["cpu".to_string()],
            by_key: None,
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
            by_fields: vec![],
            by_key: None,
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
            by_fields: vec![],
            by_key: None,
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
            by_fields: vec![],
            by_key: None,
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
    fn test_sort_table_by_key() {
        use nexus_api::TableColumn;
        let table = Value::Table {
            columns: vec![TableColumn::new("name"), TableColumn::new("age"), TableColumn::new("city")],
            rows: vec![
                vec![Value::String("Charlie".into()), Value::Int(30), Value::String("NYC".into())],
                vec![Value::String("Alice".into()), Value::Int(25), Value::String("LA".into())],
                vec![Value::String("Bob".into()), Value::Int(35), Value::String("SF".into())],
            ],
        };
        // Sort by column 2 (age, 1-based)
        let opts = SortOptions {
            reverse: false, numeric: false, by_size: false, by_time: false,
            ignore_case: false, unique: false, by_fields: vec![], by_key: Some(2),
        };
        let result = sort_value(table, &opts);
        if let Value::Table { rows, .. } = result {
            assert_eq!(rows[0][1], Value::Int(25));  // Alice
            assert_eq!(rows[1][1], Value::Int(30));  // Charlie
            assert_eq!(rows[2][1], Value::Int(35));  // Bob
        } else {
            panic!("Expected Table");
        }
    }

    #[test]
    fn test_sort_table_by_column_name() {
        use nexus_api::TableColumn;
        let table = Value::Table {
            columns: vec![TableColumn::new("name"), TableColumn::new("score")],
            rows: vec![
                vec![Value::String("B".into()), Value::Int(80)],
                vec![Value::String("A".into()), Value::Int(90)],
                vec![Value::String("C".into()), Value::Int(70)],
            ],
        };
        // Sort by "score" column name
        let opts = SortOptions {
            reverse: false, numeric: false, by_size: false, by_time: false,
            ignore_case: false, unique: false, by_fields: vec!["score".to_string()], by_key: None,
        };
        let result = sort_value(table, &opts);
        if let Value::Table { rows, .. } = result {
            assert_eq!(rows[0][1], Value::Int(70));
            assert_eq!(rows[1][1], Value::Int(80));
            assert_eq!(rows[2][1], Value::Int(90));
        } else {
            panic!("Expected Table");
        }
    }

    #[test]
    fn test_sort_multi_key() {
        use nexus_api::{ProcessInfo, ProcessStatus};

        fn test_proc(command: &str, cpu: f64, mem: u64) -> ProcessInfo {
            ProcessInfo {
                pid: 1, ppid: 0, user: "root".to_string(), group: None,
                command: command.to_string(), args: vec![],
                cpu_percent: cpu, mem_bytes: mem, mem_percent: 1.0, virtual_size: 0,
                status: ProcessStatus::Running, started: None, cpu_time: 0,
                tty: None, nice: None, priority: 0, pgid: None, sid: None,
                tpgid: None, threads: None, wchan: None, flags: None,
                is_session_leader: None, has_foreground: None,
            }
        }

        let processes = Value::List(vec![
            Value::Process(Box::new(test_proc("a", 50.0, 2000))),
            Value::Process(Box::new(test_proc("b", 50.0, 1000))),
            Value::Process(Box::new(test_proc("c", 10.0, 3000))),
        ]);

        // Multi-key: sort by cpu, then by mem as tiebreak
        let opts = SortOptions {
            reverse: false, numeric: false, by_size: false, by_time: false,
            ignore_case: false, unique: false, by_fields: vec!["cpu".to_string(), "mem".to_string()], by_key: None,
        };
        let result = sort_value(processes, &opts);
        if let Value::List(items) = result {
            match (&items[0], &items[1], &items[2]) {
                (Value::Process(a), Value::Process(b), Value::Process(c)) => {
                    assert_eq!(a.command, "c");  // cpu=10
                    assert_eq!(b.command, "b");  // cpu=50, mem=1000
                    assert_eq!(c.command, "a");  // cpu=50, mem=2000
                }
                _ => panic!("Expected Process values"),
            }
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
            by_fields: vec![],
            by_key: None,
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
