//! The `df` command - report file system disk space usage.

use super::{CommandContext, NexusCommand};
use nexus_api::{DisplayFormat, TableColumn, Value};
use std::path::PathBuf;

#[cfg(unix)]
use nix::libc;

pub struct DfCommand;

struct DfOptions {
    /// Print human-readable sizes
    human_readable: bool,
    /// Include all filesystems (including pseudo-filesystems)
    all: bool,
    /// Use 1K block size (default)
    block_1k: bool,
}

impl DfOptions {
    fn parse(args: &[String]) -> (Self, Vec<PathBuf>) {
        let mut opts = DfOptions {
            human_readable: false,
            all: false,
            block_1k: true,
        };

        let mut paths = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-h" | "--human-readable" => opts.human_readable = true,
                "-a" | "--all" => opts.all = true,
                "-k" => opts.block_1k = true,
                s if !s.starts_with('-') => paths.push(PathBuf::from(s)),
                _ => {}
            }
        }

        (opts, paths)
    }
}

impl NexusCommand for DfCommand {
    fn name(&self) -> &'static str {
        "df"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let (opts, paths) = DfOptions::parse(args);

        let columns = if opts.human_readable {
            vec![
                TableColumn::new("filesystem"),
                TableColumn::with_format("size", DisplayFormat::HumanBytes),
                TableColumn::with_format("used", DisplayFormat::HumanBytes),
                TableColumn::with_format("avail", DisplayFormat::HumanBytes),
                TableColumn::with_format("use%", DisplayFormat::BarPercentage),
                TableColumn::new("mounted"),
            ]
        } else {
            vec![
                TableColumn::new("filesystem"),
                TableColumn::new("1K-blocks"),
                TableColumn::new("used"),
                TableColumn::new("available"),
                TableColumn::with_format("use%", DisplayFormat::BarPercentage),
                TableColumn::new("mounted"),
            ]
        };

        let rows = get_filesystem_info(&opts, &paths, &ctx.state.cwd)?;

        Ok(Value::Table { columns, rows })
    }
}

#[cfg(target_os = "macos")]
fn get_filesystem_info(
    opts: &DfOptions,
    paths: &[PathBuf],
    cwd: &PathBuf,
) -> anyhow::Result<Vec<Vec<Value>>> {
    use std::ffi::CString;
    use std::mem;

    let mut rows = Vec::new();

    // If specific paths are given, show info for those filesystems
    // Otherwise, show all mounted filesystems
    if paths.is_empty() {
        // Get all mounted filesystems using getmntinfo
        unsafe {
            let mut mntbuf: *mut libc::statfs = std::ptr::null_mut();
            let count = libc::getmntinfo(&mut mntbuf, libc::MNT_NOWAIT);

            if count > 0 && !mntbuf.is_null() {
                for i in 0..count {
                    let stat = &*mntbuf.offset(i as isize);
                    if let Some(row) = statfs_to_row(stat, opts) {
                        rows.push(row);
                    }
                }
            }
        }
    } else {
        // Get info for specific paths
        for path in paths {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };

            let path_cstr = CString::new(resolved.to_string_lossy().as_bytes())?;
            let mut stat: libc::statfs = unsafe { mem::zeroed() };

            let ret = unsafe { libc::statfs(path_cstr.as_ptr(), &mut stat) };

            if ret == 0 {
                if let Some(row) = statfs_to_row(&stat, opts) {
                    rows.push(row);
                }
            }
        }
    }

    Ok(rows)
}

#[cfg(target_os = "macos")]
fn statfs_to_row(stat: &libc::statfs, opts: &DfOptions) -> Option<Vec<Value>> {
    use std::ffi::CStr;

    // Get filesystem name
    let fs_name = unsafe {
        CStr::from_ptr(stat.f_mntfromname.as_ptr())
            .to_string_lossy()
            .to_string()
    };

    // Skip pseudo-filesystems unless -a is specified
    if !opts.all {
        if fs_name.starts_with("devfs")
            || fs_name.starts_with("map ")
            || fs_name == "none"
        {
            return None;
        }
    }

    // Get mount point
    let mount_point = unsafe {
        CStr::from_ptr(stat.f_mntonname.as_ptr())
            .to_string_lossy()
            .to_string()
    };

    let block_size = stat.f_bsize as u64;
    let total_blocks = stat.f_blocks;
    let free_blocks = stat.f_bfree;
    let avail_blocks = stat.f_bavail; // Available to non-superuser

    let total_bytes = total_blocks * block_size;
    let free_bytes = free_blocks * block_size;
    let avail_bytes = avail_blocks * block_size;
    let used_bytes = total_bytes.saturating_sub(free_bytes);

    let use_percent = if total_bytes > 0 {
        ((used_bytes as f64 / total_bytes as f64) * 100.0) as i64
    } else {
        0
    };

    if opts.human_readable {
        Some(vec![
            Value::String(fs_name),
            Value::Int(total_bytes as i64),
            Value::Int(used_bytes as i64),
            Value::Int(avail_bytes as i64),
            Value::Int(use_percent),
            Value::String(mount_point),
        ])
    } else {
        // Convert to 1K blocks
        let total_1k = total_bytes / 1024;
        let used_1k = used_bytes / 1024;
        let avail_1k = avail_bytes / 1024;

        Some(vec![
            Value::String(fs_name),
            Value::Int(total_1k as i64),
            Value::Int(used_1k as i64),
            Value::Int(avail_1k as i64),
            Value::Int(use_percent),
            Value::String(mount_point),
        ])
    }
}

#[cfg(target_os = "linux")]
fn get_filesystem_info(
    opts: &DfOptions,
    paths: &[PathBuf],
    cwd: &PathBuf,
) -> anyhow::Result<Vec<Vec<Value>>> {
    use std::ffi::CString;
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use std::mem;

    let mut rows = Vec::new();

    if paths.is_empty() {
        // Read /proc/mounts to get all mounted filesystems
        let file = File::open("/proc/mounts")?;
        let reader = BufReader::new(file);

        for line in reader.lines() {
            let line = line?;
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                let fs_name = parts[0];
                let mount_point = parts[1];

                // Skip pseudo-filesystems unless -a
                if !opts.all {
                    let fs_type = parts.get(2).unwrap_or(&"");
                    if matches!(
                        *fs_type,
                        "sysfs" | "proc" | "devtmpfs" | "devpts" | "tmpfs" | "cgroup" | "cgroup2"
                    ) {
                        continue;
                    }
                }

                let path_cstr = CString::new(mount_point)?;
                let mut stat: libc::statfs = unsafe { mem::zeroed() };

                let ret = unsafe { libc::statfs(path_cstr.as_ptr(), &mut stat) };

                if ret == 0 {
                    let row = linux_statfs_to_row(fs_name, mount_point, &stat, opts);
                    rows.push(row);
                }
            }
        }
    } else {
        for path in paths {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };

            let path_cstr = CString::new(resolved.to_string_lossy().as_bytes())?;
            let mut stat: libc::statfs = unsafe { mem::zeroed() };

            let ret = unsafe { libc::statfs(path_cstr.as_ptr(), &mut stat) };

            if ret == 0 {
                let row = linux_statfs_to_row(
                    &resolved.to_string_lossy(),
                    &resolved.to_string_lossy(),
                    &stat,
                    opts,
                );
                rows.push(row);
            }
        }
    }

    Ok(rows)
}

#[cfg(target_os = "linux")]
fn linux_statfs_to_row(
    fs_name: &str,
    mount_point: &str,
    stat: &libc::statfs,
    opts: &DfOptions,
) -> Vec<Value> {
    let block_size = stat.f_bsize as u64;
    let total_blocks = stat.f_blocks;
    let free_blocks = stat.f_bfree;
    let avail_blocks = stat.f_bavail;

    let total_bytes = total_blocks * block_size;
    let free_bytes = free_blocks * block_size;
    let avail_bytes = avail_blocks * block_size;
    let used_bytes = total_bytes.saturating_sub(free_bytes);

    let use_percent = if total_bytes > 0 {
        ((used_bytes as f64 / total_bytes as f64) * 100.0) as i64
    } else {
        0
    };

    if opts.human_readable {
        vec![
            Value::String(fs_name.to_string()),
            Value::Int(total_bytes as i64),
            Value::Int(used_bytes as i64),
            Value::Int(avail_bytes as i64),
            Value::Int(use_percent),
            Value::String(mount_point.to_string()),
        ]
    } else {
        let total_1k = total_bytes / 1024;
        let used_1k = used_bytes / 1024;
        let avail_1k = avail_bytes / 1024;

        vec![
            Value::String(fs_name.to_string()),
            Value::Int(total_1k as i64),
            Value::Int(used_1k as i64),
            Value::Int(avail_1k as i64),
            Value::Int(use_percent),
            Value::String(mount_point.to_string()),
        ]
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn get_filesystem_info(
    _opts: &DfOptions,
    _paths: &[PathBuf],
    _cwd: &PathBuf,
) -> anyhow::Result<Vec<Vec<Value>>> {
    // Fallback for other platforms
    Ok(vec![vec![
        Value::String("unknown".to_string()),
        Value::Int(0),
        Value::Int(0),
        Value::Int(0),
        Value::Int(0),
        Value::String("/".to_string()),
    ]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_df_basic() {
        let mut test_ctx = TestContext::new_default();

        let cmd = DfCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Table { columns, rows } => {
                let col_names: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
                assert!(col_names.contains(&"filesystem"));
                assert!(col_names.contains(&"mounted"));
                // Should have at least one filesystem
                assert!(!rows.is_empty());
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn test_df_human_readable() {
        let mut test_ctx = TestContext::new_default();

        let cmd = DfCommand;
        let result = cmd.execute(&["-h".to_string()], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Table { columns, .. } => {
                // Check that size columns have HumanBytes format
                let size_col = columns.iter().find(|c| c.name == "size").unwrap();
                assert_eq!(size_col.format, Some(DisplayFormat::HumanBytes));

                let used_col = columns.iter().find(|c| c.name == "used").unwrap();
                assert_eq!(used_col.format, Some(DisplayFormat::HumanBytes));

                let avail_col = columns.iter().find(|c| c.name == "avail").unwrap();
                assert_eq!(avail_col.format, Some(DisplayFormat::HumanBytes));
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn test_df_specific_path() {
        let mut test_ctx = TestContext::new_default();

        let cmd = DfCommand;
        let result = cmd.execute(&["/".to_string()], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Table { rows, .. } => {
                // Should have info for the root filesystem
                assert!(!rows.is_empty());
            }
            _ => panic!("Expected Table"),
        }
    }
}
