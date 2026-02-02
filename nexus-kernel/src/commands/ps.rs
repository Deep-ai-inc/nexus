//! Process commands - ps.
//!
//! POSIX-compliant `ps` command that returns typed `ProcessInfo` values.
//!
//! ## Typed Output Benefits
//! - Rich GUI rendering with CPU sparklines, memory bars, kill buttons
//! - Type-safe filtering: `ps | where cpu > 80`
//! - Direct piping to kill: `ps | where command == "node" | kill`
//! - Field extraction: `ps | select pid,cpu,command`
//!
//! ## POSIX Options
//! - `ps` - processes for current terminal
//! - `ps -e` or `ps -A` - all processes
//! - `ps -f` - full format
//! - `ps -l` - long format
//! - `ps -u user` - processes for user
//! - `ps -p pid` - process by PID
//! - `ps -t tty` - processes for terminal
//! - `ps -o format` - custom output columns
//! - `ps --sort key` - sort by field
//!
//! ## BSD-style Options
//! - `ps aux` - all users, user-oriented format, include processes without tty
//! - `ps axjf` - show process tree

use super::{CommandContext, NexusCommand};
use nexus_api::{ProcessInfo, ProcessStatus, TableColumn, Value};
use std::collections::HashSet;

#[cfg(unix)]
use nix::libc;

pub struct PsCommand;

#[derive(Debug, Default)]
struct PsOptions {
    // Selection options
    all_processes: bool,      // -e, -A, a
    all_users: bool,          // -a (with tty), a (BSD)
    no_tty_required: bool,    // x (BSD)
    select_pids: Vec<u32>,    // -p pid,pid,...
    select_users: Vec<String>,// -u user,user,...
    select_ttys: Vec<String>, // -t tty,tty,...
    select_groups: Vec<String>,// -G group,...

    // Format options
    full_format: bool,        // -f
    long_format: bool,        // -l
    user_format: bool,        // u (BSD)
    jobs_format: bool,        // j (BSD)
    custom_format: Vec<String>,// -o col,col,...
    wide_output: bool,        // -w, w
    show_threads: bool,       // -L, -T
    forest: bool,             // --forest, f (BSD)
    no_header: bool,          // --no-header

    // Sort options
    sort_keys: Vec<(String, bool)>, // (field, descending)
}

impl PsOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = PsOptions::default();
        let mut i = 0;

        // Check for BSD-style combined options without dash (e.g., "aux")
        if let Some(first) = args.first() {
            if !first.starts_with('-') && first.chars().all(|c| c.is_alphabetic()) {
                for c in first.chars() {
                    match c {
                        'a' => opts.all_users = true,
                        'u' => opts.user_format = true,
                        'x' => opts.no_tty_required = true,
                        'e' => opts.all_processes = true,
                        'f' => opts.forest = true,
                        'j' => opts.jobs_format = true,
                        'l' => opts.long_format = true,
                        'w' => opts.wide_output = true,
                        _ => {}
                    }
                }
                i = 1;
            }
        }

        while i < args.len() {
            let arg = &args[i];

            // Handle combined short options like -ef, -aux
            if arg.starts_with('-') && !arg.starts_with("--") && arg.len() > 2 {
                for c in arg[1..].chars() {
                    match c {
                        'e' | 'A' => opts.all_processes = true,
                        'a' => opts.all_users = true,
                        'x' => opts.no_tty_required = true,
                        'f' => opts.full_format = true,
                        'l' => opts.long_format = true,
                        'u' => opts.user_format = true,
                        'j' => opts.jobs_format = true,
                        'w' => opts.wide_output = true,
                        'L' | 'T' => opts.show_threads = true,
                        _ => {}
                    }
                }
                i += 1;
                continue;
            }

            if arg == "-e" || arg == "-A" || arg == "--all" {
                opts.all_processes = true;
            } else if arg == "-a" {
                opts.all_users = true;
            } else if arg == "-x" {
                opts.no_tty_required = true;
            } else if arg == "-f" || arg == "--full" {
                opts.full_format = true;
            } else if arg == "-l" || arg == "--long" {
                opts.long_format = true;
            } else if arg == "-u" || arg == "-U" || arg == "--user" {
                if i + 1 < args.len() {
                    opts.select_users.extend(args[i + 1].split(',').map(String::from));
                    i += 1;
                }
            } else if arg.starts_with("-u") {
                opts.select_users.extend(arg[2..].split(',').map(String::from));
            } else if arg == "-p" || arg == "--pid" {
                if i + 1 < args.len() {
                    opts.select_pids.extend(
                        args[i + 1].split(',').filter_map(|s| s.parse::<u32>().ok())
                    );
                    i += 1;
                }
            } else if arg.starts_with("-p") {
                opts.select_pids.extend(
                    arg[2..].split(',').filter_map(|s| s.parse::<u32>().ok())
                );
            } else if arg == "-t" || arg == "--tty" {
                if i + 1 < args.len() {
                    opts.select_ttys.extend(args[i + 1].split(',').map(String::from));
                    i += 1;
                }
            } else if arg.starts_with("-t") {
                opts.select_ttys.extend(arg[2..].split(',').map(String::from));
            } else if arg == "-G" || arg == "--group" {
                if i + 1 < args.len() {
                    opts.select_groups.extend(args[i + 1].split(',').map(String::from));
                    i += 1;
                }
            } else if arg == "-o" || arg == "--format" {
                if i + 1 < args.len() {
                    opts.custom_format.extend(args[i + 1].split(',').map(String::from));
                    i += 1;
                }
            } else if arg.starts_with("-o") {
                opts.custom_format.extend(arg[2..].split(',').map(String::from));
            } else if arg == "-w" || arg == "-ww" {
                opts.wide_output = true;
            } else if arg == "-L" || arg == "-T" || arg == "--threads" {
                opts.show_threads = true;
            } else if arg == "--forest" {
                opts.forest = true;
            } else if arg == "--no-header" || arg == "--no-headers" {
                opts.no_header = true;
            } else if arg == "--sort" {
                if i + 1 < args.len() {
                    for key in args[i + 1].split(',') {
                        let (field, desc) = if key.starts_with('-') {
                            (key[1..].to_string(), true)
                        } else if key.starts_with('+') {
                            (key[1..].to_string(), false)
                        } else {
                            (key.to_string(), false)
                        };
                        opts.sort_keys.push((field, desc));
                    }
                    i += 1;
                }
            } else if arg.starts_with("--sort=") {
                for key in arg[7..].split(',') {
                    let (field, desc) = if key.starts_with('-') {
                        (key[1..].to_string(), true)
                    } else if key.starts_with('+') {
                        (key[1..].to_string(), false)
                    } else {
                        (key.to_string(), false)
                    };
                    opts.sort_keys.push((field, desc));
                }
            }

            i += 1;
        }

        // BSD "aux" style means all processes from all users
        if opts.all_users && opts.no_tty_required {
            opts.all_processes = true;
        }

        opts
    }

    fn should_include(&self, proc: &ProcessInfo, current_user: &str, current_tty: Option<&str>) -> bool {
        // Filter by PID - if specified, only include matching PIDs
        if !self.select_pids.is_empty() {
            return self.select_pids.contains(&proc.pid);
        }

        // Filter by user - if specified, only include matching users
        if !self.select_users.is_empty() {
            return self.select_users.contains(&proc.user);
        }

        // Filter by TTY - if specified, only include matching TTYs
        if !self.select_ttys.is_empty() {
            if let Some(ref tty) = proc.tty {
                return self.select_ttys.iter().any(|t| tty.contains(t));
            }
            return false;
        }

        // -e/-A: all processes
        if self.all_processes {
            return true;
        }

        // BSD "aux" style: all processes from all users
        if self.all_users && self.no_tty_required {
            return true;
        }

        // x (BSD): include processes without controlling terminal for current user
        if self.no_tty_required {
            return proc.user == current_user;
        }

        // -a: all processes with a tty (any user)
        if self.all_users {
            return proc.tty.is_some();
        }

        // Default: current user's processes
        // When no TTY (e.g., in tests or scripts), show current user's processes
        if current_tty.is_none() {
            return proc.user == current_user;
        }

        // With a TTY: show processes on same terminal
        if let (Some(proc_tty), Some(cur_tty)) = (&proc.tty, current_tty) {
            proc_tty == cur_tty && proc.user == current_user
        } else {
            false
        }
    }

    fn wants_table_output(&self) -> bool {
        self.full_format || self.long_format || self.user_format || self.jobs_format || !self.custom_format.is_empty()
    }
}

impl NexusCommand for PsCommand {
    fn name(&self) -> &'static str {
        "ps"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = PsOptions::parse(args);
        let mut processes = list_processes()?;

        // Get current user and tty for filtering
        let current_user = get_current_user();
        let current_tty = get_current_tty();

        // Filter processes
        processes.retain(|p| opts.should_include(p, &current_user, current_tty.as_deref()));

        // Sort if requested
        if !opts.sort_keys.is_empty() {
            processes.sort_by(|a, b| {
                for (key, desc) in &opts.sort_keys {
                    let ord = compare_by_field(a, b, key);
                    if ord != std::cmp::Ordering::Equal {
                        return if *desc { ord.reverse() } else { ord };
                    }
                }
                std::cmp::Ordering::Equal
            });
        }

        // Build forest (process tree) if requested
        if opts.forest {
            processes = build_process_tree(processes);
        }

        if opts.wants_table_output() {
            // Return as table for formatted output
            let (columns, rows) = build_table(&processes, &opts);
            Ok(Value::Table { columns, rows })
        } else {
            // Return as list of typed Process values
            let values: Vec<Value> = processes
                .into_iter()
                .map(|p| Value::Process(Box::new(p)))
                .collect();
            Ok(Value::List(values))
        }
    }
}

fn compare_by_field(a: &ProcessInfo, b: &ProcessInfo, field: &str) -> std::cmp::Ordering {
    match field.to_lowercase().as_str() {
        "pid" => a.pid.cmp(&b.pid),
        "ppid" => a.ppid.cmp(&b.ppid),
        "user" | "euser" => a.user.cmp(&b.user),
        "cpu" | "%cpu" | "pcpu" => a.cpu_percent.partial_cmp(&b.cpu_percent).unwrap_or(std::cmp::Ordering::Equal),
        "mem" | "%mem" | "pmem" => a.mem_percent.partial_cmp(&b.mem_percent).unwrap_or(std::cmp::Ordering::Equal),
        "vsz" | "vsize" => a.virtual_size.cmp(&b.virtual_size),
        "rss" | "rssize" | "rsz" => a.mem_bytes.cmp(&b.mem_bytes),
        "time" | "cputime" => a.cpu_time.cmp(&b.cpu_time),
        "start" | "stime" | "start_time" => a.started.cmp(&b.started),
        "comm" | "command" | "cmd" => a.command.cmp(&b.command),
        "tty" | "tt" => a.tty.cmp(&b.tty),
        "stat" | "state" | "s" => format!("{:?}", a.status).cmp(&format!("{:?}", b.status)),
        "ni" | "nice" => a.nice.cmp(&b.nice),
        "pri" | "priority" => a.priority.cmp(&b.priority),
        _ => std::cmp::Ordering::Equal,
    }
}

fn build_table(processes: &[ProcessInfo], opts: &PsOptions) -> (Vec<TableColumn>, Vec<Vec<Value>>) {
    let column_names: Vec<String> = if !opts.custom_format.is_empty() {
        opts.custom_format.iter().map(|s| normalize_column_name(s)).collect()
    } else if opts.long_format {
        vec!["F", "S", "UID", "PID", "PPID", "C", "PRI", "NI", "ADDR", "SZ", "WCHAN", "TTY", "TIME", "CMD"]
            .into_iter().map(String::from).collect()
    } else if opts.user_format {
        vec!["USER", "PID", "%CPU", "%MEM", "VSZ", "RSS", "TTY", "STAT", "START", "TIME", "COMMAND"]
            .into_iter().map(String::from).collect()
    } else if opts.jobs_format {
        vec!["PPID", "PID", "PGID", "SID", "TTY", "TPGID", "STAT", "UID", "TIME", "COMMAND"]
            .into_iter().map(String::from).collect()
    } else if opts.full_format {
        vec!["UID", "PID", "PPID", "C", "STIME", "TTY", "TIME", "CMD"]
            .into_iter().map(String::from).collect()
    } else {
        vec!["PID", "TTY", "TIME", "CMD"]
            .into_iter().map(String::from).collect()
    };

    let rows: Vec<Vec<Value>> = processes
        .iter()
        .map(|p| {
            column_names.iter().map(|col| format_column(p, col, opts)).collect()
        })
        .collect();

    let columns: Vec<TableColumn> = column_names.into_iter().map(TableColumn::from).collect();
    (columns, rows)
}

fn normalize_column_name(name: &str) -> String {
    // Handle column=header syntax
    if let Some(eq_pos) = name.find('=') {
        return name[eq_pos + 1..].to_string();
    }
    name.to_uppercase()
}

fn format_column(proc: &ProcessInfo, col: &str, opts: &PsOptions) -> Value {
    let col_lower = col.to_lowercase();
    // Extract actual column name (before = if present)
    let col_key = if let Some(eq_pos) = col_lower.find('=') {
        &col_lower[..eq_pos]
    } else {
        &col_lower
    };

    match col_key {
        "pid" => Value::Int(proc.pid as i64),
        "ppid" => Value::Int(proc.ppid as i64),
        "uid" | "euid" => Value::String(proc.user.clone()),
        "user" | "euser" | "uname" => Value::String(proc.user.clone()),
        "gid" | "egid" | "group" => Value::String(proc.group.clone().unwrap_or_default()),
        "%cpu" | "pcpu" | "c" => Value::Float((proc.cpu_percent * 10.0).round() / 10.0),
        "%mem" | "pmem" => Value::Float((proc.mem_percent * 10.0).round() / 10.0),
        "vsz" | "vsize" => Value::Int((proc.virtual_size / 1024) as i64),
        "rss" | "rssize" | "rsz" => Value::Int((proc.mem_bytes / 1024) as i64),
        "sz" => Value::Int((proc.virtual_size / 4096) as i64), // pages
        "tty" | "tt" => Value::String(proc.tty.clone().unwrap_or_else(|| "?".to_string())),
        "stat" | "state" | "s" => Value::String(format_state(proc)),
        "start" | "stime" | "start_time" | "lstart" => {
            if let Some(ts) = proc.started {
                Value::String(format_start_time(ts))
            } else {
                Value::String("?".to_string())
            }
        }
        "time" | "cputime" => Value::String(format_cpu_time(proc.cpu_time)),
        "etime" => Value::String(format_elapsed_time(proc.started)),
        "cmd" | "comm" | "command" | "args" => {
            if opts.wide_output || col_key == "args" || col_key == "command" {
                if proc.args.is_empty() {
                    Value::String(proc.command.clone())
                } else {
                    Value::String(format!("{} {}", proc.command, proc.args.join(" ")))
                }
            } else {
                Value::String(proc.command.clone())
            }
        }
        "pri" | "priority" => Value::Int(proc.priority as i64),
        "ni" | "nice" => Value::Int(proc.nice.unwrap_or(0) as i64),
        "pgid" | "pgrp" => Value::Int(proc.pgid.unwrap_or(0) as i64),
        "sid" | "sess" => Value::Int(proc.sid.unwrap_or(0) as i64),
        "tpgid" => Value::Int(proc.tpgid.unwrap_or(-1) as i64),
        "nlwp" | "thcount" => Value::Int(proc.threads.unwrap_or(1) as i64),
        "wchan" => Value::String(proc.wchan.clone().unwrap_or_else(|| "-".to_string())),
        "f" | "flag" | "flags" => Value::Int(proc.flags.unwrap_or(0) as i64),
        "addr" => Value::String("-".to_string()), // Kernel address, usually hidden
        _ => Value::String("-".to_string()),
    }
}

fn format_state(proc: &ProcessInfo) -> String {
    let state_char = match proc.status {
        ProcessStatus::Running => 'R',
        ProcessStatus::Sleeping => 'S',
        ProcessStatus::DiskSleep => 'D',
        ProcessStatus::Stopped => 'T',
        ProcessStatus::Zombie => 'Z',
        ProcessStatus::Idle => 'I',
        ProcessStatus::Dead => 'X',
        ProcessStatus::TracingStop => 't',
        ProcessStatus::Unknown => '?',
    };

    let mut state = String::from(state_char);

    // Add modifiers
    if proc.nice.unwrap_or(0) < 0 {
        state.push('<'); // High priority
    } else if proc.nice.unwrap_or(0) > 0 {
        state.push('N'); // Low priority
    }

    if proc.is_session_leader.unwrap_or(false) {
        state.push('s'); // Session leader
    }

    if proc.threads.map(|t| t > 1).unwrap_or(false) {
        state.push('l'); // Multi-threaded
    }

    if proc.has_foreground.unwrap_or(false) {
        state.push('+'); // Foreground process group
    }

    state
}

fn format_start_time(timestamp: u64) -> String {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    let start = UNIX_EPOCH + Duration::from_secs(timestamp);
    let now = SystemTime::now();

    let elapsed = now.duration_since(start).unwrap_or_default();
    let one_day = Duration::from_secs(24 * 60 * 60);

    if elapsed < one_day {
        // Show HH:MM for today
        let secs = timestamp % 86400;
        let hours = (secs / 3600) % 24;
        let mins = (secs % 3600) / 60;
        format!("{:02}:{:02}", hours, mins)
    } else {
        // Show Mon DD for older
        let days = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                    "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        // Simplified - in production use chrono
        let day_of_year = (timestamp / 86400) % 365;
        let month = (day_of_year / 30) as usize % 12;
        let day = (day_of_year % 30) + 1;
        format!("{}{:2}", days[month], day)
    }
}

fn format_cpu_time(secs: u64) -> String {
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;

    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{:02}:{:02}", mins, secs)
    }
}

fn format_elapsed_time(started: Option<u64>) -> String {
    let Some(start) = started else {
        return "-".to_string();
    };

    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let elapsed = now.saturating_sub(start);
    let days = elapsed / 86400;
    let hours = (elapsed % 86400) / 3600;
    let mins = (elapsed % 3600) / 60;
    let secs = elapsed % 60;

    if days > 0 {
        format!("{}-{:02}:{:02}:{:02}", days, hours, mins, secs)
    } else if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    } else {
        format!("{:02}:{:02}", mins, secs)
    }
}

fn build_process_tree(mut processes: Vec<ProcessInfo>) -> Vec<ProcessInfo> {
    // Sort by PID first
    processes.sort_by_key(|p| p.pid);

    // Build parent->children map
    let pids: HashSet<u32> = processes.iter().map(|p| p.pid).collect();
    let mut children: std::collections::HashMap<u32, Vec<usize>> = std::collections::HashMap::new();

    for (i, proc) in processes.iter().enumerate() {
        if pids.contains(&proc.ppid) {
            children.entry(proc.ppid).or_default().push(i);
        }
    }

    // Find roots (processes whose parent is not in our list)
    let roots: Vec<usize> = processes.iter().enumerate()
        .filter(|(_, p)| !pids.contains(&p.ppid))
        .map(|(i, _)| i)
        .collect();

    // DFS to build tree order with indentation
    let mut result = Vec::new();
    let mut visited = vec![false; processes.len()];

    fn visit(
        idx: usize,
        depth: usize,
        processes: &[ProcessInfo],
        children: &std::collections::HashMap<u32, Vec<usize>>,
        visited: &mut [bool],
        result: &mut Vec<ProcessInfo>,
    ) {
        if visited[idx] {
            return;
        }
        visited[idx] = true;

        let mut proc = processes[idx].clone();
        // Add tree prefix to command
        if depth > 0 {
            let prefix = "  ".repeat(depth - 1) + "\\_ ";
            proc.command = format!("{}{}", prefix, proc.command);
        }
        result.push(proc);

        if let Some(child_indices) = children.get(&processes[idx].pid) {
            for &child_idx in child_indices {
                visit(child_idx, depth + 1, processes, children, visited, result);
            }
        }
    }

    for root in roots {
        visit(root, 0, &processes, &children, &mut visited, &mut result);
    }

    // Add any unvisited processes (shouldn't happen, but just in case)
    for (i, proc) in processes.into_iter().enumerate() {
        if !visited[i] {
            result.push(proc);
        }
    }

    result
}

fn get_current_user() -> String {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        unsafe {
            let uid = libc::getuid();
            let pwd = libc::getpwuid(uid);
            if !pwd.is_null() {
                CStr::from_ptr((*pwd).pw_name)
                    .to_string_lossy()
                    .to_string()
            } else {
                uid.to_string()
            }
        }
    }
    #[cfg(not(unix))]
    {
        std::env::var("USER").unwrap_or_else(|_| "unknown".to_string())
    }
}

fn get_current_tty() -> Option<String> {
    #[cfg(unix)]
    {
        use std::ffi::CStr;
        unsafe {
            let tty = libc::ttyname(0); // stdin
            if !tty.is_null() {
                Some(CStr::from_ptr(tty).to_string_lossy().to_string())
            } else {
                None
            }
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

// ============================================================================
// Platform-specific process listing
// ============================================================================

#[cfg(target_os = "macos")]
fn list_processes() -> anyhow::Result<Vec<ProcessInfo>> {
    use std::process::Command;

    // Use ps with simpler format for reliable parsing
    // Avoid lstart which spans multiple whitespace-separated fields
    let output = Command::new("ps")
        .args(["-axo", "pid,ppid,pgid,sess,uid,user,%cpu,%mem,vsz,rss,tty,state,etime,time,comm"])
        .output()?;

    if !output.status.success() {
        anyhow::bail!("ps command failed");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut processes = Vec::new();

    for line in stdout.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();

        if parts.len() < 15 {
            continue;
        }

        let pid: u32 = parts[0].parse().unwrap_or(0);
        let ppid: u32 = parts[1].parse().unwrap_or(0);
        let pgid: u32 = parts[2].parse().unwrap_or(0);
        let sid: u32 = parts[3].parse().unwrap_or(0);
        let _uid: u32 = parts[4].parse().unwrap_or(0);
        let user = parts[5].to_string();
        let cpu_percent: f64 = parts[6].parse().unwrap_or(0.0);
        let mem_percent: f64 = parts[7].parse().unwrap_or(0.0);
        let vsz: u64 = parts[8].parse::<u64>().unwrap_or(0) * 1024;
        let rss: u64 = parts[9].parse::<u64>().unwrap_or(0) * 1024;
        let tty_str = parts[10];
        let stat = parts[11];
        let etime = parts[12]; // elapsed time
        let time_str = parts[13];
        let command = parts[14].to_string();

        // Parse elapsed time to approximate start time
        let started = parse_etime_to_start(etime);
        let cpu_time = parse_time(time_str);

        let tty = if tty_str == "??" || tty_str == "-" {
            None
        } else {
            Some(format!("/dev/{}", tty_str))
        };

        let (status, nice, has_foreground) = parse_stat(stat);

        processes.push(ProcessInfo {
            pid,
            ppid,
            user,
            group: None,
            command,
            args: vec![], // Would need separate call to get args
            cpu_percent,
            mem_bytes: rss,
            mem_percent,
            virtual_size: vsz,
            status,
            started,
            cpu_time,
            tty,
            nice: Some(nice),
            priority: 0,
            pgid: Some(pgid),
            sid: Some(sid),
            tpgid: None,
            threads: None,
            wchan: None,
            flags: None,
            is_session_leader: Some(pid == sid),
            has_foreground: Some(has_foreground),
        });
    }

    Ok(processes)
}

#[cfg(target_os = "macos")]
fn parse_etime_to_start(etime: &str) -> Option<u64> {
    // Parse elapsed time format: [[DD-]HH:]MM:SS
    // Examples: "1-02:03:04" (1 day, 2 hours, 3 mins, 4 secs)
    //           "02:03:04" (2 hours, 3 mins, 4 secs)
    //           "03:04" (3 mins, 4 secs)
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .ok()?;

    let elapsed_secs = parse_etime(etime);
    Some(now.saturating_sub(elapsed_secs))
}

#[cfg(target_os = "macos")]
fn parse_etime(etime: &str) -> u64 {
    // Parse [[DD-]HH:]MM:SS format
    let mut total_secs: u64 = 0;

    // Check for days
    let time_part = if let Some(dash_pos) = etime.find('-') {
        let days: u64 = etime[..dash_pos].parse().unwrap_or(0);
        total_secs += days * 86400;
        &etime[dash_pos + 1..]
    } else {
        etime
    };

    let time_parts: Vec<&str> = time_part.split(':').collect();
    match time_parts.len() {
        3 => {
            // HH:MM:SS
            let hours: u64 = time_parts[0].parse().unwrap_or(0);
            let mins: u64 = time_parts[1].parse().unwrap_or(0);
            let secs: u64 = time_parts[2].parse().unwrap_or(0);
            total_secs += hours * 3600 + mins * 60 + secs;
        }
        2 => {
            // MM:SS
            let mins: u64 = time_parts[0].parse().unwrap_or(0);
            let secs: u64 = time_parts[1].parse().unwrap_or(0);
            total_secs += mins * 60 + secs;
        }
        1 => {
            // Just seconds
            let secs: u64 = time_parts[0].parse().unwrap_or(0);
            total_secs += secs;
        }
        _ => {}
    }

    total_secs
}

#[cfg(target_os = "macos")]
fn parse_time(s: &str) -> u64 {
    let parts: Vec<&str> = s.split(':').collect();
    match parts.len() {
        2 => {
            let mins: u64 = parts[0].parse().unwrap_or(0);
            let secs: u64 = parts[1].split('.').next().and_then(|s| s.parse().ok()).unwrap_or(0);
            mins * 60 + secs
        }
        3 => {
            let hours: u64 = parts[0].parse().unwrap_or(0);
            let mins: u64 = parts[1].parse().unwrap_or(0);
            let secs: u64 = parts[2].split('.').next().and_then(|s| s.parse().ok()).unwrap_or(0);
            hours * 3600 + mins * 60 + secs
        }
        _ => 0,
    }
}

#[cfg(target_os = "macos")]
fn parse_stat(stat: &str) -> (ProcessStatus, i8, bool) {
    let mut chars = stat.chars();
    let state = chars.next().unwrap_or('?');
    let rest: String = chars.collect();

    let status = match state {
        'R' => ProcessStatus::Running,
        'S' => ProcessStatus::Sleeping,
        'D' => ProcessStatus::DiskSleep,
        'T' => ProcessStatus::Stopped,
        'Z' => ProcessStatus::Zombie,
        'I' => ProcessStatus::Idle,
        'U' => ProcessStatus::DiskSleep, // Uninterruptible wait
        _ => ProcessStatus::Unknown,
    };

    let nice = if rest.contains('<') {
        -10 // High priority
    } else if rest.contains('N') {
        10 // Low priority
    } else {
        0
    };

    let has_foreground = rest.contains('+');

    (status, nice, has_foreground)
}

#[cfg(target_os = "linux")]
fn list_processes() -> anyhow::Result<Vec<ProcessInfo>> {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut processes = Vec::new();

    // Get system info for calculations
    let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    let total_mem = get_total_memory();
    let boot_time = get_boot_time();
    let uptime = get_uptime();

    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();

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

        // Parse stat (format: pid (comm) state ppid pgrp session tty_nr tpgid flags ...)
        let comm_start = stat.find('(').unwrap_or(0);
        let comm_end = stat.rfind(')').unwrap_or(stat.len());
        let command = stat[comm_start + 1..comm_end].to_string();
        let fields: Vec<&str> = stat[comm_end + 2..].split_whitespace().collect();

        if fields.len() < 40 {
            continue;
        }

        let state = fields[0];
        let ppid: u32 = fields[1].parse().unwrap_or(0);
        let pgid: u32 = fields[2].parse().unwrap_or(0);
        let sid: u32 = fields[3].parse().unwrap_or(0);
        let tty_nr: i32 = fields[4].parse().unwrap_or(0);
        let tpgid: i32 = fields[5].parse().unwrap_or(-1);
        let flags: u32 = fields[6].parse().unwrap_or(0);
        let utime: u64 = fields[11].parse().unwrap_or(0);
        let stime: u64 = fields[12].parse().unwrap_or(0);
        let nice: i8 = fields[16].parse().unwrap_or(0);
        let num_threads: u32 = fields[17].parse().unwrap_or(1);
        let starttime: u64 = fields[19].parse().unwrap_or(0);
        let vsize: u64 = fields[20].parse().unwrap_or(0);
        let rss: i64 = fields[21].parse().unwrap_or(0);
        let priority: i32 = fields[15].parse().unwrap_or(0);

        // Calculate CPU time in seconds
        let cpu_time = (utime + stime) / clk_tck as u64;

        // Calculate start time
        let started = if boot_time > 0 {
            Some(boot_time + (starttime / clk_tck as u64))
        } else {
            None
        };

        // Calculate CPU percentage (approximation)
        let total_time = utime + stime;
        let process_uptime = uptime.saturating_sub(starttime / clk_tck as u64);
        let cpu_percent = if process_uptime > 0 {
            (total_time as f64 / clk_tck) / process_uptime as f64 * 100.0
        } else {
            0.0
        };

        // Memory
        let mem_bytes = (rss.max(0) as u64) * page_size;
        let mem_percent = if total_mem > 0 {
            (mem_bytes as f64 / total_mem as f64) * 100.0
        } else {
            0.0
        };

        // Read status for user info
        let status_path = proc_path.join("status");
        let status_content = fs::read_to_string(&status_path).unwrap_or_default();
        let uid: u32 = status_content
            .lines()
            .find(|l| l.starts_with("Uid:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let gid: u32 = status_content
            .lines()
            .find(|l| l.starts_with("Gid:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let user = get_username(uid);
        let group = get_groupname(gid);

        // Read cmdline for args
        let cmdline_path = proc_path.join("cmdline");
        let cmdline = fs::read_to_string(&cmdline_path).unwrap_or_default();
        let args: Vec<String> = cmdline
            .split('\0')
            .filter(|s| !s.is_empty())
            .skip(1)
            .map(String::from)
            .collect();

        // TTY
        let tty = if tty_nr > 0 {
            get_tty_name(tty_nr)
        } else {
            None
        };

        // Wchan
        let wchan = fs::read_to_string(proc_path.join("wchan"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| s != "0" && !s.is_empty());

        let status = match state {
            "R" => ProcessStatus::Running,
            "S" => ProcessStatus::Sleeping,
            "D" => ProcessStatus::DiskSleep,
            "T" | "t" => ProcessStatus::Stopped,
            "Z" => ProcessStatus::Zombie,
            "I" => ProcessStatus::Idle,
            "X" => ProcessStatus::Dead,
            _ => ProcessStatus::Unknown,
        };

        processes.push(ProcessInfo {
            pid,
            ppid,
            user,
            group: Some(group),
            command,
            args,
            cpu_percent,
            mem_bytes,
            mem_percent,
            virtual_size: vsize,
            status,
            started,
            cpu_time,
            tty,
            nice: Some(nice),
            priority,
            pgid: Some(pgid),
            sid: Some(sid),
            tpgid: Some(tpgid),
            threads: Some(num_threads),
            wchan,
            flags: Some(flags),
            is_session_leader: Some(pid == sid),
            has_foreground: Some(tpgid > 0 && pgid == tpgid as u32),
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

#[cfg(target_os = "linux")]
fn get_groupname(gid: u32) -> String {
    use std::ffi::CStr;
    unsafe {
        let grp = libc::getgrgid(gid);
        if grp.is_null() {
            gid.to_string()
        } else {
            CStr::from_ptr((*grp).gr_name)
                .to_string_lossy()
                .to_string()
        }
    }
}

#[cfg(target_os = "linux")]
fn get_total_memory() -> u64 {
    use std::fs;
    fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|content| {
            content.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse::<u64>().ok())
                .map(|kb| kb * 1024)
        })
        .unwrap_or(16 * 1024 * 1024 * 1024) // Default 16GB
}

#[cfg(target_os = "linux")]
fn get_boot_time() -> u64 {
    use std::fs;
    fs::read_to_string("/proc/stat")
        .ok()
        .and_then(|content| {
            content.lines()
                .find(|l| l.starts_with("btime "))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn get_uptime() -> u64 {
    use std::fs;
    fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|content| {
            content.split_whitespace().next()
                .and_then(|s| s.parse::<f64>().ok())
                .map(|f| f as u64)
        })
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn get_tty_name(tty_nr: i32) -> Option<String> {
    // TTY number is encoded as major*256 + minor
    let major = tty_nr >> 8;
    let minor = tty_nr & 0xff;

    match major {
        4 => Some(format!("/dev/tty{}", minor)),
        136..=143 => Some(format!("/dev/pts/{}", minor + (major - 136) * 256)),
        _ => Some(format!("tty{}/{}", major, minor)),
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn list_processes() -> anyhow::Result<Vec<ProcessInfo>> {
    // Fallback for other platforms - try using system ps command
    use std::process::Command;

    let output = Command::new("ps")
        .args(["-eo", "pid,ppid,user,%cpu,%mem,stat,comm"])
        .output()?;

    if !output.status.success() {
        return Ok(vec![]);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut processes = Vec::new();

    for line in stdout.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 7 {
            continue;
        }

        processes.push(ProcessInfo {
            pid: parts[0].parse().unwrap_or(0),
            ppid: parts[1].parse().unwrap_or(0),
            user: parts[2].to_string(),
            group: None,
            command: parts[6].to_string(),
            args: vec![],
            cpu_percent: parts[3].parse().unwrap_or(0.0),
            mem_bytes: 0,
            mem_percent: parts[4].parse().unwrap_or(0.0),
            virtual_size: 0,
            status: ProcessStatus::Unknown,
            started: None,
            cpu_time: 0,
            tty: None,
            nice: None,
            priority: 0,
            pgid: None,
            sid: None,
            tpgid: None,
            threads: None,
            wchan: None,
            flags: None,
            is_session_leader: None,
            has_foreground: None,
        });
    }

    Ok(processes)
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
    fn test_ps_aux_returns_table() {
        let cmd = PsCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&["aux".to_string()], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Table { columns, rows } => {
                assert!(columns.iter().any(|c| c.name == "USER"));
                assert!(columns.iter().any(|c| c.name == "PID"));
                assert!(columns.iter().any(|c| c.name == "%CPU"));
                assert!(columns.iter().any(|c| c.name == "COMMAND"));
                assert!(!rows.is_empty());
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn test_ps_full_format() {
        let cmd = PsCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&["-ef".to_string()], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Table { columns, .. } => {
                assert!(columns.iter().any(|c| c.name == "UID"));
                assert!(columns.iter().any(|c| c.name == "PID"));
                assert!(columns.iter().any(|c| c.name == "PPID"));
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn test_ps_select_pid() {
        let cmd = PsCommand;
        let mut test_ctx = TestContext::new_default();

        let current_pid = std::process::id().to_string();
        let result = cmd.execute(&["-p".to_string(), current_pid.clone()], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::List(items) => {
                assert_eq!(items.len(), 1);
                if let Value::Process(p) = &items[0] {
                    assert_eq!(p.pid.to_string(), current_pid);
                }
            }
            _ => panic!("Expected List"),
        }
    }

    #[test]
    fn test_ps_sort() {
        let cmd = PsCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&["-e".to_string(), "--sort".to_string(), "-pid".to_string()], &mut test_ctx.ctx()).unwrap();

        if let Value::List(items) = result {
            // Check that PIDs are in descending order
            let mut prev_pid = u32::MAX;
            for item in items.iter().take(10) {
                if let Value::Process(p) = item {
                    assert!(p.pid <= prev_pid, "PIDs should be in descending order");
                    prev_pid = p.pid;
                }
            }
        }
    }

    #[test]
    fn test_process_get_field() {
        let proc = ProcessInfo {
            pid: 1234,
            ppid: 1,
            user: "root".to_string(),
            group: Some("wheel".to_string()),
            command: "test".to_string(),
            args: vec!["--arg".to_string()],
            cpu_percent: 50.0,
            mem_bytes: 1024 * 1024,
            mem_percent: 1.0,
            virtual_size: 100 * 1024 * 1024,
            status: ProcessStatus::Running,
            started: Some(1234567890),
            cpu_time: 3600,
            tty: Some("/dev/pts/0".to_string()),
            nice: Some(0),
            priority: 20,
            pgid: Some(1234),
            sid: Some(1234),
            tpgid: Some(1234),
            threads: Some(4),
            wchan: None,
            flags: Some(0),
            is_session_leader: Some(true),
            has_foreground: Some(true),
        };

        assert_eq!(proc.get_field("pid"), Some(Value::Int(1234)));
        assert_eq!(proc.get_field("user"), Some(Value::String("root".to_string())));
        assert_eq!(proc.get_field("cpu"), Some(Value::Float(50.0)));
        assert_eq!(proc.get_field("status"), Some(Value::String("Running".to_string())));
        assert_eq!(proc.get_field("tty"), Some(Value::String("/dev/pts/0".to_string())));
    }

    #[test]
    fn test_format_cpu_time() {
        assert_eq!(format_cpu_time(0), "00:00");
        assert_eq!(format_cpu_time(65), "01:05");
        assert_eq!(format_cpu_time(3665), "01:01:05");
    }

    #[test]
    fn test_option_parsing() {
        let opts = PsOptions::parse(&["aux".to_string()]);
        assert!(opts.all_users);
        assert!(opts.user_format);
        assert!(opts.no_tty_required);

        let opts = PsOptions::parse(&["-ef".to_string()]);
        assert!(opts.all_processes);
        assert!(opts.full_format);

        let opts = PsOptions::parse(&["-p".to_string(), "1,2,3".to_string()]);
        assert_eq!(opts.select_pids, vec![1, 2, 3]);

        let opts = PsOptions::parse(&["--sort".to_string(), "-cpu,+pid".to_string()]);
        assert_eq!(opts.sort_keys, vec![("cpu".to_string(), true), ("pid".to_string(), false)]);
    }
}

/// Public helper: get all processes as a table Value (for top command).
pub fn get_process_table() -> anyhow::Result<Value> {
    let processes = list_processes()?;
    let opts = PsOptions {
        all_processes: true,
        user_format: true,
        no_tty_required: true,
        ..PsOptions::default()
    };
    let (columns, rows) = build_table(&processes, &opts);
    Ok(Value::Table { columns, rows })
}
