//! Process commands - ps.
//!
//! Returns typed `ProcessInfo` values that enable:
//! - Rich GUI rendering with CPU sparklines, memory bars, kill buttons
//! - Type-safe filtering: `ps | where cpu > 80`
//! - Direct piping to kill: `ps | where command == "node" | kill`

use super::{CommandContext, NexusCommand};
use nexus_api::{ProcessInfo, ProcessStatus, Value};

// ============================================================================
// ps - List processes (returns typed ProcessInfo)
// ============================================================================

pub struct PsCommand;

impl NexusCommand for PsCommand {
    fn name(&self) -> &'static str {
        "ps"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let show_all = args.iter().any(|a| a == "-a" || a == "-A" || a == "-e" || a == "aux");
        let show_full = args.iter().any(|a| a == "-f" || a == "-l" || a == "aux");

        let processes = list_processes(show_all)?;

        if show_full {
            // Return as table for full listing (backwards compatible)
            let columns = vec![
                "pid".to_string(),
                "ppid".to_string(),
                "user".to_string(),
                "cpu".to_string(),
                "mem".to_string(),
                "status".to_string(),
                "command".to_string(),
            ];

            let rows: Vec<Vec<Value>> = processes
                .iter()
                .map(|p| {
                    vec![
                        Value::Int(p.pid as i64),
                        Value::Int(p.ppid as i64),
                        Value::String(p.user.clone()),
                        Value::Float(p.cpu_percent),
                        Value::Float(p.mem_percent),
                        Value::String(format!("{:?}", p.status)),
                        Value::String(if p.args.is_empty() {
                            p.command.clone()
                        } else {
                            format!("{} {}", p.command, p.args.join(" "))
                        }),
                    ]
                })
                .collect();

            Ok(Value::Table { columns, rows })
        } else {
            // Return as list of typed Process values (the new way!)
            let values: Vec<Value> = processes
                .into_iter()
                .map(|p| Value::Process(Box::new(p)))
                .collect();
            Ok(Value::List(values))
        }
    }
}

#[cfg(target_os = "macos")]
fn list_processes(all: bool) -> anyhow::Result<Vec<ProcessInfo>> {
    use std::process::Command;

    // Use ps command with specific format for reliable parsing
    // Format: pid, ppid, user, %cpu, %mem, stat, comm, args
    let output = Command::new("ps")
        .args(if all {
            vec!["-axo", "pid,ppid,user,%cpu,%mem,state,comm"]
        } else {
            vec!["-o", "pid,ppid,user,%cpu,%mem,state,comm"]
        })
        .output()?;

    if !output.status.success() {
        anyhow::bail!("ps command failed");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut processes = Vec::new();

    for line in stdout.lines().skip(1) {
        // Skip header
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 7 {
            continue;
        }

        let pid: u32 = parts[0].parse().unwrap_or(0);
        let ppid: u32 = parts[1].parse().unwrap_or(0);
        let user = parts[2].to_string();
        let cpu_percent: f64 = parts[3].parse().unwrap_or(0.0);
        let mem_percent: f64 = parts[4].parse().unwrap_or(0.0);
        let state = parts[5];
        let command = parts[6].to_string();

        // Approximate memory bytes from percentage (assume 16GB system)
        // In a real implementation, we'd query actual memory
        let total_mem: u64 = 16 * 1024 * 1024 * 1024;
        let mem_bytes = ((mem_percent / 100.0) * total_mem as f64) as u64;

        let status = match state.chars().next() {
            Some('R') => ProcessStatus::Running,
            Some('S') => ProcessStatus::Sleeping,
            Some('I') => ProcessStatus::Idle,
            Some('T') => ProcessStatus::Stopped,
            Some('Z') => ProcessStatus::Zombie,
            _ => ProcessStatus::Unknown,
        };

        processes.push(ProcessInfo {
            pid,
            ppid,
            user,
            command,
            args: vec![], // Would need -o args to get full args
            cpu_percent,
            mem_bytes,
            mem_percent,
            status,
            started: None, // Would need -o lstart to get this
        });
    }

    Ok(processes)
}

#[cfg(target_os = "linux")]
fn list_processes(all: bool) -> anyhow::Result<Vec<ProcessInfo>> {
    use std::fs;
    use std::path::Path;

    let mut processes = Vec::new();
    let current_uid = unsafe { libc::getuid() };

    // Read /proc for each process
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();

        // Only look at numeric directories (PIDs)
        let pid: u32 = match name.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        let proc_path = entry.path();

        // Read stat file
        let stat_path = proc_path.join("stat");
        let stat = match fs::read_to_string(&stat_path) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Parse stat (format: pid (comm) state ppid ...)
        let comm_start = stat.find('(').unwrap_or(0);
        let comm_end = stat.rfind(')').unwrap_or(stat.len());
        let command = stat[comm_start + 1..comm_end].to_string();
        let after_comm: Vec<&str> = stat[comm_end + 2..].split_whitespace().collect();

        if after_comm.is_empty() {
            continue;
        }

        let state = after_comm[0];
        let ppid: u32 = after_comm.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);

        // Read status for user info
        let status_path = proc_path.join("status");
        let status_content = fs::read_to_string(&status_path).unwrap_or_default();
        let uid: u32 = status_content
            .lines()
            .find(|l| l.starts_with("Uid:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // Filter by user if not showing all
        if !all && uid != current_uid {
            continue;
        }

        // Get username
        let user = get_username(uid);

        // Read statm for memory
        let statm_path = proc_path.join("statm");
        let mem_pages: u64 = fs::read_to_string(&statm_path)
            .ok()
            .and_then(|s| s.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
        let mem_bytes = mem_pages * page_size;

        // CPU usage would require sampling over time - just report 0 for now
        let cpu_percent = 0.0;

        let status = match state {
            "R" => ProcessStatus::Running,
            "S" | "D" => ProcessStatus::Sleeping,
            "T" | "t" => ProcessStatus::Stopped,
            "Z" => ProcessStatus::Zombie,
            "I" => ProcessStatus::Idle,
            _ => ProcessStatus::Unknown,
        };

        // Read cmdline for args
        let cmdline_path = proc_path.join("cmdline");
        let cmdline = fs::read_to_string(&cmdline_path).unwrap_or_default();
        let args: Vec<String> = cmdline
            .split('\0')
            .filter(|s| !s.is_empty())
            .skip(1)
            .map(String::from)
            .collect();

        processes.push(ProcessInfo {
            pid,
            ppid,
            user,
            command,
            args,
            cpu_percent,
            mem_bytes,
            mem_percent: 0.0, // Would need to read /proc/meminfo
            status,
            started: None,
        });
    }

    Ok(processes)
}

#[cfg(target_os = "linux")]
fn get_username(uid: u32) -> String {
    use std::ffi::CStr;

    unsafe {
        let pwd = libc::getpwuid(uid);
        if pwd.is_null() {
            uid.to_string()
        } else {
            CStr::from_ptr((*pwd).pw_name)
                .to_string_lossy()
                .to_string()
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn list_processes(_all: bool) -> anyhow::Result<Vec<ProcessInfo>> {
    // Fallback for other platforms
    Ok(vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_ps_returns_list() {
        let cmd = PsCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::List(items) => {
                // Should have at least one process (ourselves)
                assert!(!items.is_empty());

                // First item should be a Process
                match &items[0] {
                    Value::Process(p) => {
                        assert!(p.pid > 0);
                        assert!(!p.command.is_empty());
                    }
                    _ => panic!("Expected Process variant"),
                }
            }
            _ => panic!("Expected List"),
        }
    }

    #[test]
    fn test_ps_full_returns_table() {
        let cmd = PsCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&["-f".to_string()], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Table { columns, rows } => {
                assert!(columns.contains(&"pid".to_string()));
                assert!(columns.contains(&"command".to_string()));
                assert!(!rows.is_empty());
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn test_process_get_field() {
        let proc = ProcessInfo {
            pid: 1234,
            ppid: 1,
            user: "root".to_string(),
            command: "test".to_string(),
            args: vec!["--arg".to_string()],
            cpu_percent: 50.0,
            mem_bytes: 1024 * 1024,
            mem_percent: 1.0,
            status: ProcessStatus::Running,
            started: Some(1234567890),
        };

        assert_eq!(proc.get_field("pid"), Some(Value::Int(1234)));
        assert_eq!(proc.get_field("user"), Some(Value::String("root".to_string())));
        assert_eq!(proc.get_field("cpu"), Some(Value::Float(50.0)));
        assert_eq!(proc.get_field("status"), Some(Value::String("Running".to_string())));
    }
}
