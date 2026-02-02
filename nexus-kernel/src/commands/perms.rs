//! Permission commands - chmod, chown, chgrp.

use super::{CommandContext, NexusCommand};
use nexus_api::{FileOpInfo, FileOpKind, FileOpPhase, Value};
use std::os::unix::fs::{chown, PermissionsExt};
use std::path::PathBuf;

// ============================================================================
// chmod - Change file mode bits
// ============================================================================

pub struct ChmodCommand;

impl NexusCommand for ChmodCommand {
    fn name(&self) -> &'static str {
        "chmod"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.len() < 2 {
            anyhow::bail!("usage: chmod [-R] mode file...");
        }

        let mut recursive = false;
        let mut arg_start = 0;

        // Parse options
        for (i, arg) in args.iter().enumerate() {
            match arg.as_str() {
                "-R" | "--recursive" => recursive = true,
                arg if arg.starts_with('-') && !arg.chars().nth(1).map(|c| c.is_ascii_digit()).unwrap_or(false) => {
                    anyhow::bail!("chmod: unrecognized option: {}", arg);
                }
                _ => {
                    arg_start = i;
                    break;
                }
            }
        }

        let mode_str = &args[arg_start];
        let files = &args[arg_start + 1..];

        if files.is_empty() {
            anyhow::bail!("chmod: missing operand after '{}'", mode_str);
        }

        let mode_spec = parse_mode(mode_str)?;
        let sources: Vec<PathBuf> = files.iter().map(|f| PathBuf::from(f)).collect();
        let start_time_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut files_processed = 0usize;
        let mut errors = Vec::new();

        for file in files {
            let path = resolve_path(file, ctx);
            match chmod_file(&path, &mode_spec, recursive) {
                Ok(count) => files_processed += count,
                Err(e) => errors.push(nexus_api::FileOpError {
                    path: path.clone(),
                    message: e.to_string(),
                }),
            }
        }

        let phase = if errors.is_empty() {
            FileOpPhase::Completed
        } else {
            FileOpPhase::Failed
        };

        Ok(Value::file_op(FileOpInfo {
            op_type: FileOpKind::Chmod,
            phase,
            sources,
            dest: None,
            total_bytes: None,
            bytes_processed: 0,
            files_total: None,
            files_processed,
            current_file: None,
            start_time_ms,
            errors,
        }))
    }
}

fn chmod_file(path: &PathBuf, mode_spec: &ModeSpec, recursive: bool) -> anyhow::Result<usize> {
    let metadata = std::fs::metadata(path)
        .map_err(|e| anyhow::anyhow!("{}: {}", path.display(), e))?;
    let current_mode = metadata.permissions().mode();

    let new_mode = mode_spec.apply(current_mode, metadata.is_dir());

    let mut perms = metadata.permissions();
    perms.set_mode(new_mode);
    std::fs::set_permissions(path, perms)
        .map_err(|e| anyhow::anyhow!("{}: {}", path.display(), e))?;

    let mut count = 1;

    if recursive && metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            count += chmod_file(&entry.path(), mode_spec, true)?;
        }
    }

    Ok(count)
}

// ============================================================================
// POSIX symbolic mode parsing
// ============================================================================

/// Parsed mode specification: either absolute octal or symbolic clauses.
enum ModeSpec {
    Absolute(u32),
    Symbolic(Vec<SymbolicClause>),
}

impl ModeSpec {
    fn apply(&self, current: u32, is_dir: bool) -> u32 {
        match self {
            ModeSpec::Absolute(m) => (current & !0o7777) | (*m & 0o7777),
            ModeSpec::Symbolic(clauses) => {
                let mut mode = current & 0o7777; // Keep only permission bits
                for clause in clauses {
                    mode = clause.apply(mode, is_dir);
                }
                // Preserve file type bits from current
                (current & !0o7777) | mode
            }
        }
    }
}

#[derive(Debug)]
struct SymbolicClause {
    who: u32,          // Bitmask: 0o100 = user, 0o010 = group, 0o001 = other
    op: ModeOp,
    perms: u32,        // Permission bits (rwxst mapped to their positions)
    conditional_x: bool, // X flag
}

#[derive(Debug, Clone, Copy)]
enum ModeOp {
    Add,
    Remove,
    Set,
}

impl SymbolicClause {
    fn apply(&self, current: u32, is_dir: bool) -> u32 {
        let mut bits = self.perms;

        // Handle conditional X: set execute only if directory or already has any execute bit
        if self.conditional_x {
            if is_dir || (current & 0o111 != 0) {
                // Add x bits for the targeted who
                if self.who & 0o100 != 0 { bits |= 0o100; }
                if self.who & 0o010 != 0 { bits |= 0o010; }
                if self.who & 0o001 != 0 { bits |= 0o001; }
            }
        }

        match self.op {
            ModeOp::Add => current | bits,
            ModeOp::Remove => current & !bits,
            ModeOp::Set => {
                // Clear all bits for targeted users, then set new ones
                let mut mask = 0u32;
                if self.who & 0o100 != 0 { mask |= 0o4700; } // user: rwx + setuid
                if self.who & 0o010 != 0 { mask |= 0o2070; } // group: rwx + setgid
                if self.who & 0o001 != 0 { mask |= 0o1007; } // other: rwx + sticky
                (current & !mask) | bits
            }
        }
    }
}

/// Parse an octal or symbolic mode string.
fn parse_mode(s: &str) -> anyhow::Result<ModeSpec> {
    // Try octal first
    if s.chars().all(|c| c.is_ascii_digit()) {
        if let Ok(mode) = u32::from_str_radix(s, 8) {
            if mode <= 0o7777 {
                return Ok(ModeSpec::Absolute(mode));
            }
        }
    }

    // Symbolic mode: clause[,clause]*
    let mut clauses = Vec::new();
    for part in s.split(',') {
        clauses.push(parse_symbolic_clause(part)?);
    }
    if clauses.is_empty() {
        anyhow::bail!("chmod: invalid mode: '{}'", s);
    }
    Ok(ModeSpec::Symbolic(clauses))
}

fn parse_symbolic_clause(s: &str) -> anyhow::Result<SymbolicClause> {
    let bytes = s.as_bytes();
    let mut i = 0;

    // Parse who: [ugoa]*
    let mut who = 0u32;
    while i < bytes.len() {
        match bytes[i] {
            b'u' => who |= 0o100,
            b'g' => who |= 0o010,
            b'o' => who |= 0o001,
            b'a' => who |= 0o111,
            _ => break,
        }
        i += 1;
    }

    // If no who specified, default to 'a'
    if who == 0 {
        who = 0o111;
    }

    // Parse op: [+-=]
    if i >= bytes.len() {
        anyhow::bail!("chmod: invalid mode: '{}'", s);
    }
    let op = match bytes[i] {
        b'+' => ModeOp::Add,
        b'-' => ModeOp::Remove,
        b'=' => ModeOp::Set,
        _ => anyhow::bail!("chmod: invalid mode: '{}'", s),
    };
    i += 1;

    // Parse perms: [rwxXst]*  or copy source [ugo]
    let mut perms = 0u32;
    let mut conditional_x = false;

    while i < bytes.len() {
        match bytes[i] {
            b'r' => {
                if who & 0o100 != 0 { perms |= 0o400; }
                if who & 0o010 != 0 { perms |= 0o040; }
                if who & 0o001 != 0 { perms |= 0o004; }
            }
            b'w' => {
                if who & 0o100 != 0 { perms |= 0o200; }
                if who & 0o010 != 0 { perms |= 0o020; }
                if who & 0o001 != 0 { perms |= 0o002; }
            }
            b'x' => {
                if who & 0o100 != 0 { perms |= 0o100; }
                if who & 0o010 != 0 { perms |= 0o010; }
                if who & 0o001 != 0 { perms |= 0o001; }
            }
            b'X' => {
                conditional_x = true;
            }
            b's' => {
                if who & 0o100 != 0 { perms |= 0o4000; } // setuid
                if who & 0o010 != 0 { perms |= 0o2000; } // setgid
            }
            b't' => {
                perms |= 0o1000; // sticky
            }
            _ => break,
        }
        i += 1;
    }

    if i != bytes.len() {
        anyhow::bail!("chmod: invalid mode: '{}'", s);
    }

    Ok(SymbolicClause { who, op, perms, conditional_x })
}

// ============================================================================
// chown - Change file owner and group
// ============================================================================

pub struct ChownCommand;

impl NexusCommand for ChownCommand {
    fn name(&self) -> &'static str {
        "chown"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.len() < 2 {
            anyhow::bail!("usage: chown [-R] [owner][:group] file...");
        }

        let mut recursive = false;
        let mut arg_start = 0;

        // Parse options
        for (i, arg) in args.iter().enumerate() {
            match arg.as_str() {
                "-R" | "--recursive" => recursive = true,
                arg if arg.starts_with('-') => {
                    anyhow::bail!("chown: unrecognized option: {}", arg);
                }
                _ => {
                    arg_start = i;
                    break;
                }
            }
        }

        let owner_spec = &args[arg_start];
        let files = &args[arg_start + 1..];

        if files.is_empty() {
            anyhow::bail!("chown: missing operand after '{}'", owner_spec);
        }

        let (uid, gid) = parse_owner_spec(owner_spec)?;
        let sources: Vec<PathBuf> = files.iter().map(|f| PathBuf::from(f)).collect();
        let start_time_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut files_processed = 0usize;
        let mut errors = Vec::new();

        for file in files {
            let path = resolve_path(file, ctx);
            match chown_file(&path, uid, gid, recursive) {
                Ok(count) => files_processed += count,
                Err(e) => errors.push(nexus_api::FileOpError {
                    path: path.clone(),
                    message: e.to_string(),
                }),
            }
        }

        let phase = if errors.is_empty() {
            FileOpPhase::Completed
        } else {
            FileOpPhase::Failed
        };

        Ok(Value::file_op(FileOpInfo {
            op_type: FileOpKind::Chown,
            phase,
            sources,
            dest: None,
            total_bytes: None,
            bytes_processed: 0,
            files_total: None,
            files_processed,
            current_file: None,
            start_time_ms,
            errors,
        }))
    }
}

fn chown_file(path: &PathBuf, uid: Option<u32>, gid: Option<u32>, recursive: bool) -> anyhow::Result<usize> {
    chown(path, uid, gid)
        .map_err(|e| anyhow::anyhow!("{}: {}", path.display(), e))?;

    let mut count = 1;

    if recursive {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                count += chown_file(&entry.path(), uid, gid, true)?;
            }
        }
    }

    Ok(count)
}

/// Parse owner[:group] specification. Supports name or numeric uid/gid.
fn parse_owner_spec(s: &str) -> anyhow::Result<(Option<u32>, Option<u32>)> {
    let (owner, group) = if let Some((o, g)) = s.split_once(':') {
        (
            if o.is_empty() { None } else { Some(o) },
            if g.is_empty() { None } else { Some(g) },
        )
    } else {
        (Some(s), None)
    };

    let uid = owner
        .map(|o| {
            o.parse::<u32>()
                .or_else(|_| resolve_uid_by_name(o))
                .map_err(|_| anyhow::anyhow!("chown: invalid user: '{}'", o))
        })
        .transpose()?;

    let gid = group
        .map(|g| {
            g.parse::<u32>()
                .or_else(|_| resolve_gid_by_name(g))
                .map_err(|_| anyhow::anyhow!("chown: invalid group: '{}'", g))
        })
        .transpose()?;

    Ok((uid, gid))
}

// ============================================================================
// chgrp - Change group ownership
// ============================================================================

pub struct ChgrpCommand;

impl NexusCommand for ChgrpCommand {
    fn name(&self) -> &'static str {
        "chgrp"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.len() < 2 {
            anyhow::bail!("usage: chgrp [-R] group file...");
        }

        let mut recursive = false;
        let mut arg_start = 0;

        // Parse options
        for (i, arg) in args.iter().enumerate() {
            match arg.as_str() {
                "-R" | "--recursive" => recursive = true,
                arg if arg.starts_with('-') => {
                    anyhow::bail!("chgrp: unrecognized option: {}", arg);
                }
                _ => {
                    arg_start = i;
                    break;
                }
            }
        }

        let group_spec = &args[arg_start];
        let files = &args[arg_start + 1..];

        if files.is_empty() {
            anyhow::bail!("chgrp: missing operand after '{}'", group_spec);
        }

        let gid = parse_group(group_spec)?;
        let sources: Vec<PathBuf> = files.iter().map(|f| PathBuf::from(f)).collect();
        let start_time_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut files_processed = 0usize;
        let mut errors = Vec::new();

        for file in files {
            let path = resolve_path(file, ctx);
            match chown_file(&path, None, Some(gid), recursive) {
                Ok(count) => files_processed += count,
                Err(e) => errors.push(nexus_api::FileOpError {
                    path: path.clone(),
                    message: e.to_string(),
                }),
            }
        }

        let phase = if errors.is_empty() {
            FileOpPhase::Completed
        } else {
            FileOpPhase::Failed
        };

        Ok(Value::file_op(FileOpInfo {
            op_type: FileOpKind::Chown,
            phase,
            sources,
            dest: None,
            total_bytes: None,
            bytes_processed: 0,
            files_total: None,
            files_processed,
            current_file: None,
            start_time_ms,
            errors,
        }))
    }
}

/// Parse a group specification (name or GID).
fn parse_group(s: &str) -> anyhow::Result<u32> {
    s.parse::<u32>()
        .or_else(|_| resolve_gid_by_name(s))
        .map_err(|_| anyhow::anyhow!("chgrp: invalid group: '{}'", s))
}

/// Resolve a username to uid using libc::getpwnam_r.
fn resolve_uid_by_name(name: &str) -> Result<u32, ()> {
    use std::ffi::CString;
    let c_name = CString::new(name).map_err(|_| ())?;
    let mut buf = vec![0u8; 1024];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();

    let ret = unsafe {
        libc::getpwnam_r(
            c_name.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };

    if ret == 0 && !result.is_null() {
        Ok(pwd.pw_uid)
    } else {
        Err(())
    }
}

/// Resolve a group name to gid using libc::getgrnam_r.
fn resolve_gid_by_name(name: &str) -> Result<u32, ()> {
    use std::ffi::CString;
    let c_name = CString::new(name).map_err(|_| ())?;
    let mut buf = vec![0u8; 1024];
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::group = std::ptr::null_mut();

    let ret = unsafe {
        libc::getgrnam_r(
            c_name.as_ptr(),
            &mut grp,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };

    if ret == 0 && !result.is_null() {
        Ok(grp.gr_gid)
    } else {
        Err(())
    }
}

/// Resolve a file path relative to the current working directory.
fn resolve_path(file: &str, ctx: &CommandContext) -> PathBuf {
    let path = PathBuf::from(file);
    if path.is_absolute() {
        path
    } else {
        ctx.state.cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mode_octal() {
        let spec = parse_mode("755").unwrap();
        assert_eq!(spec.apply(0o100644, false), 0o100755);
    }

    #[test]
    fn test_parse_mode_octal_644() {
        let spec = parse_mode("644").unwrap();
        assert_eq!(spec.apply(0o100755, false), 0o100644);
    }

    #[test]
    fn test_symbolic_add_execute() {
        let spec = parse_mode("+x").unwrap();
        assert_eq!(spec.apply(0o100644, false) & 0o7777, 0o755);
    }

    #[test]
    fn test_symbolic_user_add_execute() {
        let spec = parse_mode("u+x").unwrap();
        assert_eq!(spec.apply(0o100644, false) & 0o7777, 0o744);
    }

    #[test]
    fn test_symbolic_remove_write() {
        let spec = parse_mode("go-w").unwrap();
        assert_eq!(spec.apply(0o100666, false) & 0o7777, 0o644);
    }

    #[test]
    fn test_symbolic_set_equals() {
        let spec = parse_mode("o=r").unwrap();
        assert_eq!(spec.apply(0o100777, false) & 0o7777, 0o774);
    }

    #[test]
    fn test_symbolic_comma_separated() {
        let spec = parse_mode("u+rwx,go+rx").unwrap();
        assert_eq!(spec.apply(0o100000, false) & 0o7777, 0o755);
    }

    #[test]
    fn test_symbolic_conditional_x_on_dir() {
        let spec = parse_mode("+X").unwrap();
        // Directory: should add execute
        assert_eq!(spec.apply(0o040644, true) & 0o7777, 0o755);
    }

    #[test]
    fn test_symbolic_conditional_x_on_file_no_exec() {
        let spec = parse_mode("+X").unwrap();
        // File without execute: should NOT add execute
        assert_eq!(spec.apply(0o100644, false) & 0o7777, 0o644);
    }

    #[test]
    fn test_symbolic_conditional_x_on_file_with_exec() {
        let spec = parse_mode("+X").unwrap();
        // File with some execute bit: should add all execute
        assert_eq!(spec.apply(0o100744, false) & 0o7777, 0o755);
    }

    #[test]
    fn test_symbolic_setuid() {
        let spec = parse_mode("u+s").unwrap();
        assert_eq!(spec.apply(0o100755, false) & 0o7777, 0o4755);
    }

    #[test]
    fn test_symbolic_sticky() {
        let spec = parse_mode("+t").unwrap();
        assert_eq!(spec.apply(0o040755, true) & 0o7777, 0o1755);
    }

    #[test]
    fn test_symbolic_clear_other() {
        let spec = parse_mode("o=").unwrap();
        assert_eq!(spec.apply(0o100777, false) & 0o7777, 0o770);
    }

    #[test]
    fn test_parse_owner_spec_numeric_uid() {
        let (uid, gid) = parse_owner_spec("1000").unwrap();
        assert_eq!(uid, Some(1000));
        assert_eq!(gid, None);
    }

    #[test]
    fn test_parse_owner_spec_uid_and_gid() {
        let (uid, gid) = parse_owner_spec("1000:1000").unwrap();
        assert_eq!(uid, Some(1000));
        assert_eq!(gid, Some(1000));
    }

    #[test]
    fn test_parse_owner_spec_gid_only() {
        let (uid, gid) = parse_owner_spec(":1000").unwrap();
        assert_eq!(uid, None);
        assert_eq!(gid, Some(1000));
    }

    #[test]
    fn test_parse_group_numeric() {
        assert_eq!(parse_group("1000").unwrap(), 1000);
        assert_eq!(parse_group("0").unwrap(), 0);
    }

    #[test]
    fn test_parse_owner_spec_by_name() {
        // "root" should resolve on any Unix system
        let (uid, _gid) = parse_owner_spec("root").unwrap();
        assert_eq!(uid, Some(0));
    }

    #[test]
    fn test_parse_group_by_name() {
        // "wheel" or "root" should exist; try wheel first (macOS), then root (Linux)
        let result = parse_group("wheel").or_else(|_| parse_group("root"));
        assert!(result.is_ok());
    }
}
