//! The `ls` command - list directory contents.

use super::{CommandContext, NexusCommand};
use nexus_api::{DisplayFormat, FileEntry, TableColumn, Value};
use std::path::PathBuf;

pub struct LsCommand;

/// Options parsed from command-line arguments.
#[derive(Default)]
struct LsOptions {
    /// Show hidden files (starting with .)
    all: bool,
    /// Almost all - like -a but exclude . and ..
    almost_all: bool,
    /// Long format (detailed)
    long: bool,
    /// Human-readable sizes
    human_readable: bool,
    /// Sort by modification time
    sort_by_time: bool,
    /// Reverse sort order
    reverse: bool,
    /// List directories themselves, not contents
    directory: bool,
    /// Paths to list
    paths: Vec<PathBuf>,
}

impl LsOptions {
    fn parse(args: &[String]) -> anyhow::Result<Self> {
        let mut opts = LsOptions::default();

        for arg in args {
            if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") {
                // Short options (can be combined: -la)
                for c in arg[1..].chars() {
                    match c {
                        'a' => opts.all = true,
                        'A' => opts.almost_all = true,
                        'l' => opts.long = true,
                        'h' => opts.human_readable = true,
                        't' => opts.sort_by_time = true,
                        'r' => opts.reverse = true,
                        'd' => opts.directory = true,
                        _ => {} // Ignore unknown for now
                    }
                }
            } else if arg.starts_with("--") {
                // Long options
                match arg.as_str() {
                    "--all" => opts.all = true,
                    "--almost-all" => opts.almost_all = true,
                    "--human-readable" => opts.human_readable = true,
                    "--reverse" => opts.reverse = true,
                    "--directory" => opts.directory = true,
                    _ => {} // Ignore unknown
                }
            } else {
                // Path argument
                opts.paths.push(PathBuf::from(arg));
            }
        }

        Ok(opts)
    }

    fn show_hidden(&self) -> bool {
        self.all || self.almost_all
    }

    fn show_dot_entries(&self) -> bool {
        self.all && !self.almost_all
    }
}

impl NexusCommand for LsCommand {
    fn name(&self) -> &'static str {
        "ls"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = LsOptions::parse(args)?;

        // Default to current directory if no paths specified
        let paths = if opts.paths.is_empty() {
            vec![ctx.state.cwd.clone()]
        } else {
            // Resolve relative paths against cwd
            opts.paths
                .iter()
                .map(|p| {
                    if p.is_absolute() {
                        p.clone()
                    } else {
                        ctx.state.cwd.join(p)
                    }
                })
                .collect()
        };

        let mut all_entries = Vec::new();

        for path in &paths {
            if opts.directory || !path.is_dir() {
                // List the path itself
                if let Ok(entry) = FileEntry::from_path(path.clone()) {
                    all_entries.push(entry);
                }
            } else {
                // List directory contents
                let entries = list_directory(path, &opts)?;
                all_entries.extend(entries);
            }
        }

        // Sort entries
        sort_entries(&mut all_entries, &opts);

        // Convert to Value
        if opts.long {
            // Long format: return as a table
            Ok(entries_to_table(all_entries, &opts))
        } else {
            // Simple format: return as a list of FileEntry values
            Ok(Value::List(
                all_entries.into_iter().map(Value::from).collect(),
            ))
        }
    }
}

fn list_directory(path: &PathBuf, opts: &LsOptions) -> anyhow::Result<Vec<FileEntry>> {
    let mut entries = Vec::new();

    // Add . and .. if -a (not -A)
    if opts.show_dot_entries() {
        if let Ok(entry) = FileEntry::from_path(path.clone()) {
            let mut dot = entry.clone();
            dot.name = ".".to_string();
            entries.push(dot);
        }
        if let Some(parent) = path.parent() {
            if let Ok(entry) = FileEntry::from_path(parent.to_path_buf()) {
                let mut dotdot = entry;
                dotdot.name = "..".to_string();
                entries.push(dotdot);
            }
        }
    }

    // Read directory contents
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();

        if let Ok(file_entry) = FileEntry::from_path(path) {
            // Skip hidden files unless -a or -A
            if !opts.show_hidden() && file_entry.is_hidden {
                continue;
            }
            entries.push(file_entry);
        }
    }

    Ok(entries)
}

fn sort_entries(entries: &mut [FileEntry], opts: &LsOptions) {
    if opts.sort_by_time {
        entries.sort_by(|a, b| {
            let cmp = b.modified.cmp(&a.modified); // Newest first by default
            if opts.reverse {
                cmp.reverse()
            } else {
                cmp
            }
        });
    } else {
        // Alphabetical by name (case-insensitive)
        entries.sort_by(|a, b| {
            let cmp = a.name.to_lowercase().cmp(&b.name.to_lowercase());
            if opts.reverse {
                cmp.reverse()
            } else {
                cmp
            }
        });
    }
}

fn entries_to_table(mut entries: Vec<FileEntry>, opts: &LsOptions) -> Value {
    // Resolve uid/gid to names
    resolve_owner_group(&mut entries);

    // Build columns with format hints based on options
    // The -h flag sets HumanBytes format on size column - data stays as Int!
    let columns = vec![
        TableColumn::new("permissions"),
        TableColumn::new("nlink"),
        TableColumn::new("owner"),
        TableColumn::new("group"),
        if opts.human_readable {
            TableColumn::with_format("size", DisplayFormat::HumanBytes)
        } else {
            TableColumn::new("size")
        },
        TableColumn::new("modified"),
        TableColumn::new("name"),
    ];

    let rows: Vec<Vec<Value>> = entries
        .into_iter()
        .map(|e| {
            let owner = e.owner.as_deref()
                .or_else(|| e.uid.map(|_| ""))
                .unwrap_or("-");
            let owner_str = if owner.is_empty() {
                e.uid.map(|u| u.to_string()).unwrap_or_else(|| "-".to_string())
            } else {
                owner.to_string()
            };
            let group = e.group.as_deref()
                .or_else(|| e.gid.map(|_| ""))
                .unwrap_or("-");
            let group_str = if group.is_empty() {
                e.gid.map(|g| g.to_string()).unwrap_or_else(|| "-".to_string())
            } else {
                group.to_string()
            };

            vec![
                Value::String(format_permissions(e.permissions)),
                Value::Int(e.nlink.unwrap_or(1) as i64),
                Value::String(owner_str),
                Value::String(group_str),
                // Always store raw bytes - formatting happens at render time!
                Value::Int(e.size as i64),
                Value::String(format_time(e.modified)),
                // Keep as FileEntry so rendering can make it clickable
                Value::FileEntry(Box::new(e)),
            ]
        })
        .collect();

    Value::Table { columns, rows }
}

/// Resolve uid/gid to names using libc on Unix. Uses a per-call cache.
#[cfg(unix)]
fn resolve_owner_group(entries: &mut [FileEntry]) {
    use std::collections::HashMap;

    let mut uid_cache: HashMap<u32, String> = HashMap::new();
    let mut gid_cache: HashMap<u32, String> = HashMap::new();

    for entry in entries.iter_mut() {
        if let Some(uid) = entry.uid {
            let name = uid_cache.entry(uid).or_insert_with(|| {
                resolve_username(uid).unwrap_or_else(|| uid.to_string())
            });
            entry.owner = Some(name.clone());
        }
        if let Some(gid) = entry.gid {
            let name = gid_cache.entry(gid).or_insert_with(|| {
                resolve_groupname(gid).unwrap_or_else(|| gid.to_string())
            });
            entry.group = Some(name.clone());
        }
    }
}

#[cfg(not(unix))]
fn resolve_owner_group(_entries: &mut [FileEntry]) {
    // No uid/gid resolution on non-Unix platforms
}

#[cfg(unix)]
fn resolve_username(uid: u32) -> Option<String> {
    use std::ffi::CStr;
    let mut buf_size = 1024usize;
    loop {
        let mut buf = vec![0u8; buf_size];
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();

        let ret = unsafe {
            libc::getpwuid_r(
                uid,
                &mut pwd,
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            )
        };

        if ret == 0 && !result.is_null() {
            let name = unsafe { CStr::from_ptr(pwd.pw_name) };
            return name.to_str().ok().map(|s| s.to_string());
        } else if ret == libc::ERANGE && buf_size < 65536 {
            buf_size *= 2;
            continue;
        } else {
            return None;
        }
    }
}

#[cfg(unix)]
fn resolve_groupname(gid: u32) -> Option<String> {
    use std::ffi::CStr;
    let mut buf_size = 1024usize;
    loop {
        let mut buf = vec![0u8; buf_size];
        let mut grp: libc::group = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::group = std::ptr::null_mut();

        let ret = unsafe {
            libc::getgrgid_r(
                gid,
                &mut grp,
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            )
        };

        if ret == 0 && !result.is_null() {
            let name = unsafe { CStr::from_ptr(grp.gr_name) };
            return name.to_str().ok().map(|s| s.to_string());
        } else if ret == libc::ERANGE && buf_size < 65536 {
            buf_size *= 2;
            continue;
        } else {
            return None;
        }
    }
}

fn format_permissions(mode: u32) -> String {
    let file_type = match (mode >> 12) & 0xF {
        0o04 => 'd', // directory
        0o10 => '-', // regular file
        0o12 => 'l', // symlink
        0o01 => 'p', // fifo
        0o02 => 'c', // char device
        0o06 => 'b', // block device
        0o14 => 's', // socket
        _ => '?',
    };

    let perms = [
        if mode & 0o400 != 0 { 'r' } else { '-' },
        if mode & 0o200 != 0 { 'w' } else { '-' },
        if mode & 0o100 != 0 { 'x' } else { '-' },
        if mode & 0o040 != 0 { 'r' } else { '-' },
        if mode & 0o020 != 0 { 'w' } else { '-' },
        if mode & 0o010 != 0 { 'x' } else { '-' },
        if mode & 0o004 != 0 { 'r' } else { '-' },
        if mode & 0o002 != 0 { 'w' } else { '-' },
        if mode & 0o001 != 0 { 'x' } else { '-' },
    ];

    std::iter::once(file_type).chain(perms).collect()
}

fn format_time(ts: Option<u64>) -> String {
    match ts {
        Some(secs) => {
            // Simple formatting - just show timestamp for now
            // In a real implementation, would use chrono for proper formatting
            use std::time::{SystemTime, UNIX_EPOCH};

            let now = SystemTime::now();

            // If within the last 6 months, show "HH:MM"
            // Otherwise show year
            let six_months_ago = now
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs().saturating_sub(180 * 24 * 3600))
                .unwrap_or(0);

            if secs > six_months_ago {
                // Recent: show time
                let time_of_day = secs % 86400;
                let hours = time_of_day / 3600;
                let minutes = (time_of_day % 3600) / 60;
                format!("{:02}:{:02}", hours, minutes)
            } else {
                // Old: show year
                let years_since_1970 = secs / (365 * 24 * 3600);
                format!("{}", 1970 + years_since_1970)
            }
        }
        None => "?".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // LsOptions::parse tests
    // =========================================================================

    #[test]
    fn test_parse_no_args() {
        let opts = LsOptions::parse(&[]).unwrap();
        assert!(!opts.all);
        assert!(!opts.almost_all);
        assert!(!opts.long);
        assert!(!opts.human_readable);
        assert!(!opts.sort_by_time);
        assert!(!opts.reverse);
        assert!(opts.paths.is_empty());
    }

    #[test]
    fn test_parse_short_options_individual() {
        let opts = LsOptions::parse(&["-a".to_string()]).unwrap();
        assert!(opts.all);

        let opts = LsOptions::parse(&["-l".to_string()]).unwrap();
        assert!(opts.long);

        let opts = LsOptions::parse(&["-h".to_string()]).unwrap();
        assert!(opts.human_readable);

        let opts = LsOptions::parse(&["-t".to_string()]).unwrap();
        assert!(opts.sort_by_time);

        let opts = LsOptions::parse(&["-r".to_string()]).unwrap();
        assert!(opts.reverse);

        let opts = LsOptions::parse(&["-A".to_string()]).unwrap();
        assert!(opts.almost_all);

        let opts = LsOptions::parse(&["-d".to_string()]).unwrap();
        assert!(opts.directory);
    }

    #[test]
    fn test_parse_short_options_combined() {
        let opts = LsOptions::parse(&["-la".to_string()]).unwrap();
        assert!(opts.long);
        assert!(opts.all);

        let opts = LsOptions::parse(&["-lah".to_string()]).unwrap();
        assert!(opts.long);
        assert!(opts.all);
        assert!(opts.human_readable);

        let opts = LsOptions::parse(&["-ltr".to_string()]).unwrap();
        assert!(opts.long);
        assert!(opts.sort_by_time);
        assert!(opts.reverse);
    }

    #[test]
    fn test_parse_long_options() {
        let opts = LsOptions::parse(&["--all".to_string()]).unwrap();
        assert!(opts.all);

        let opts = LsOptions::parse(&["--almost-all".to_string()]).unwrap();
        assert!(opts.almost_all);

        let opts = LsOptions::parse(&["--human-readable".to_string()]).unwrap();
        assert!(opts.human_readable);

        let opts = LsOptions::parse(&["--reverse".to_string()]).unwrap();
        assert!(opts.reverse);

        let opts = LsOptions::parse(&["--directory".to_string()]).unwrap();
        assert!(opts.directory);
    }

    #[test]
    fn test_parse_paths() {
        let opts = LsOptions::parse(&["/tmp".to_string()]).unwrap();
        assert_eq!(opts.paths.len(), 1);
        assert_eq!(opts.paths[0], PathBuf::from("/tmp"));

        let opts = LsOptions::parse(&["/tmp".to_string(), "/var".to_string()]).unwrap();
        assert_eq!(opts.paths.len(), 2);
    }

    #[test]
    fn test_parse_mixed_options_and_paths() {
        let opts = LsOptions::parse(&[
            "-la".to_string(),
            "/tmp".to_string(),
            "--human-readable".to_string(),
            "/var".to_string(),
        ]).unwrap();
        assert!(opts.long);
        assert!(opts.all);
        assert!(opts.human_readable);
        assert_eq!(opts.paths.len(), 2);
    }

    #[test]
    fn test_parse_ignores_unknown_options() {
        let opts = LsOptions::parse(&["-xyz".to_string()]).unwrap();
        // Should not panic, just ignores unknown
        assert!(!opts.all);

        let opts = LsOptions::parse(&["--unknown-option".to_string()]).unwrap();
        assert!(!opts.all);
    }

    // =========================================================================
    // LsOptions predicates
    // =========================================================================

    #[test]
    fn test_show_hidden() {
        let mut opts = LsOptions::default();
        assert!(!opts.show_hidden());

        opts.all = true;
        assert!(opts.show_hidden());

        opts.all = false;
        opts.almost_all = true;
        assert!(opts.show_hidden());

        opts.all = true;
        opts.almost_all = true;
        assert!(opts.show_hidden());
    }

    #[test]
    fn test_show_dot_entries() {
        let mut opts = LsOptions::default();
        assert!(!opts.show_dot_entries());

        opts.all = true;
        assert!(opts.show_dot_entries());

        // -A should NOT show . and ..
        opts.all = false;
        opts.almost_all = true;
        assert!(!opts.show_dot_entries());

        // -a with -A should NOT show . and .. (almost_all overrides)
        opts.all = true;
        opts.almost_all = true;
        assert!(!opts.show_dot_entries());
    }

    // =========================================================================
    // format_permissions tests
    // =========================================================================

    #[test]
    fn test_format_permissions_regular_file() {
        // Regular file with rwxr-xr-x (0100755)
        let mode = 0o100755;
        assert_eq!(format_permissions(mode), "-rwxr-xr-x");
    }

    #[test]
    fn test_format_permissions_directory() {
        // Directory with rwxr-xr-x (0040755)
        let mode = 0o040755;
        assert_eq!(format_permissions(mode), "drwxr-xr-x");
    }

    #[test]
    fn test_format_permissions_symlink() {
        // Symlink with rwxrwxrwx (0120777)
        let mode = 0o120777;
        assert_eq!(format_permissions(mode), "lrwxrwxrwx");
    }

    #[test]
    fn test_format_permissions_readonly() {
        // Regular file with r--r--r-- (0100444)
        let mode = 0o100444;
        assert_eq!(format_permissions(mode), "-r--r--r--");
    }

    #[test]
    fn test_format_permissions_no_perms() {
        // Regular file with no permissions (0100000)
        let mode = 0o100000;
        assert_eq!(format_permissions(mode), "----------");
    }

    #[test]
    fn test_format_permissions_all_perms() {
        // Directory with rwxrwxrwx (0040777)
        let mode = 0o040777;
        assert_eq!(format_permissions(mode), "drwxrwxrwx");
    }

    #[test]
    fn test_format_permissions_special_types() {
        // FIFO (0010644)
        assert_eq!(format_permissions(0o010644), "prw-r--r--");
        // Char device (0020644)
        assert_eq!(format_permissions(0o020644), "crw-r--r--");
        // Block device (0060644)
        assert_eq!(format_permissions(0o060644), "brw-r--r--");
        // Socket (0140755)
        assert_eq!(format_permissions(0o140755), "srwxr-xr-x");
    }

    // =========================================================================
    // format_time tests
    // =========================================================================

    #[test]
    fn test_format_time_none() {
        assert_eq!(format_time(None), "?");
    }

    #[test]
    fn test_format_time_old_shows_year() {
        // Very old timestamp (1980) should show year
        let ts_1980 = 315532800; // Jan 1, 1980
        let result = format_time(Some(ts_1980));
        assert!(result.parse::<u64>().is_ok(), "Old date should show year, got: {}", result);
        assert!(result.starts_with("19") || result.starts_with("20"));
    }

    #[test]
    fn test_format_time_recent_shows_time() {
        // Current timestamp should show HH:MM
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let result = format_time(Some(now));
        assert!(result.contains(':'), "Recent date should show time HH:MM, got: {}", result);
        assert_eq!(result.len(), 5); // "HH:MM"
    }

    #[test]
    fn test_format_time_format_is_valid() {
        // Test that time format is always valid
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let result = format_time(Some(now));
        // Should be either "HH:MM" or a year
        assert!(
            result.contains(':') || result.parse::<u64>().is_ok(),
            "Invalid format: {}", result
        );
    }

    // =========================================================================
    // sort_entries tests
    // =========================================================================

    fn make_entry(name: &str, modified: Option<u64>) -> FileEntry {
        FileEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            file_type: nexus_api::FileType::File,
            is_hidden: name.starts_with('.'),
            is_symlink: false,
            symlink_target: None,
            size: 0,
            modified,
            accessed: None,
            created: None,
            permissions: 0o100644,
            uid: None,
            gid: None,
            owner: None,
            group: None,
            nlink: Some(1),
        }
    }

    #[test]
    fn test_sort_entries_alphabetical() {
        let mut entries = vec![
            make_entry("zebra", None),
            make_entry("apple", None),
            make_entry("mango", None),
        ];
        let opts = LsOptions::default();
        sort_entries(&mut entries, &opts);

        assert_eq!(entries[0].name, "apple");
        assert_eq!(entries[1].name, "mango");
        assert_eq!(entries[2].name, "zebra");
    }

    #[test]
    fn test_sort_entries_alphabetical_case_insensitive() {
        let mut entries = vec![
            make_entry("Zebra", None),
            make_entry("apple", None),
            make_entry("Mango", None),
        ];
        let opts = LsOptions::default();
        sort_entries(&mut entries, &opts);

        assert_eq!(entries[0].name, "apple");
        assert_eq!(entries[1].name, "Mango");
        assert_eq!(entries[2].name, "Zebra");
    }

    #[test]
    fn test_sort_entries_reverse() {
        let mut entries = vec![
            make_entry("apple", None),
            make_entry("zebra", None),
            make_entry("mango", None),
        ];
        let mut opts = LsOptions::default();
        opts.reverse = true;
        sort_entries(&mut entries, &opts);

        assert_eq!(entries[0].name, "zebra");
        assert_eq!(entries[1].name, "mango");
        assert_eq!(entries[2].name, "apple");
    }

    #[test]
    fn test_sort_entries_by_time() {
        let mut entries = vec![
            make_entry("old", Some(1000)),
            make_entry("newest", Some(3000)),
            make_entry("middle", Some(2000)),
        ];
        let mut opts = LsOptions::default();
        opts.sort_by_time = true;
        sort_entries(&mut entries, &opts);

        // Newest first by default
        assert_eq!(entries[0].name, "newest");
        assert_eq!(entries[1].name, "middle");
        assert_eq!(entries[2].name, "old");
    }

    #[test]
    fn test_sort_entries_by_time_reverse() {
        let mut entries = vec![
            make_entry("old", Some(1000)),
            make_entry("newest", Some(3000)),
            make_entry("middle", Some(2000)),
        ];
        let mut opts = LsOptions::default();
        opts.sort_by_time = true;
        opts.reverse = true;
        sort_entries(&mut entries, &opts);

        // Oldest first with reverse
        assert_eq!(entries[0].name, "old");
        assert_eq!(entries[1].name, "middle");
        assert_eq!(entries[2].name, "newest");
    }

    #[test]
    fn test_sort_entries_empty() {
        let mut entries: Vec<FileEntry> = vec![];
        let opts = LsOptions::default();
        sort_entries(&mut entries, &opts);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_sort_entries_single() {
        let mut entries = vec![make_entry("only", None)];
        let opts = LsOptions::default();
        sort_entries(&mut entries, &opts);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "only");
    }
}

