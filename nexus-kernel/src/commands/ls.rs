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
                Value::String(format_name(&e)),
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

fn format_name(entry: &FileEntry) -> String {
    if let Some(target) = &entry.symlink_target {
        format!("{} -> {}", entry.name, target.display())
    } else {
        entry.name.clone()
    }
}
