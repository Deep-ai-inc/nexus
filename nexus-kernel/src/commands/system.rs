//! System information commands - tty, uname, umask.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::path::PathBuf;

// ============================================================================
// tty - Print terminal name
// ============================================================================

pub struct TtyCommand;

impl NexusCommand for TtyCommand {
    fn name(&self) -> &'static str {
        "tty"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let silent = args.iter().any(|a| a == "-s" || a == "--silent" || a == "--quiet");

        #[cfg(unix)]
        {
            use nix::unistd::isatty;
            use std::os::unix::io::AsRawFd;

            // Check if stdin is a tty
            let stdin_fd = std::io::stdin().as_raw_fd();

            if isatty(stdin_fd).unwrap_or(false) {
                // Get the tty name
                let tty_path = get_tty_name(stdin_fd);

                if silent {
                    Ok(Value::Bool(true))
                } else {
                    Ok(Value::Path(tty_path))
                }
            } else {
                if !silent {
                    eprintln!("not a tty");
                }
                if silent {
                    Ok(Value::Bool(false))
                } else {
                    anyhow::bail!("not a tty")
                }
            }
        }

        #[cfg(not(unix))]
        {
            if silent {
                Ok(Value::Bool(false))
            } else {
                anyhow::bail!("not a tty")
            }
        }
    }
}

#[cfg(unix)]
fn get_tty_name(fd: i32) -> PathBuf {
    use nix::libc;
    use std::ffi::CStr;

    unsafe {
        let name = libc::ttyname(fd);
        if name.is_null() {
            PathBuf::from("/dev/tty")
        } else {
            PathBuf::from(CStr::from_ptr(name).to_string_lossy().into_owned())
        }
    }
}

// ============================================================================
// uname - Print system information
// ============================================================================

pub struct UnameCommand;

impl NexusCommand for UnameCommand {
    fn name(&self) -> &'static str {
        "uname"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut show_sysname = false;
        let mut show_nodename = false;
        let mut show_release = false;
        let mut show_version = false;
        let mut show_machine = false;
        let mut show_all = false;

        // Parse options
        for arg in args {
            match arg.as_str() {
                "-a" | "--all" => show_all = true,
                "-s" | "--kernel-name" => show_sysname = true,
                "-n" | "--nodename" => show_nodename = true,
                "-r" | "--kernel-release" => show_release = true,
                "-v" | "--kernel-version" => show_version = true,
                "-m" | "--machine" => show_machine = true,
                _ if arg.starts_with('-') => {
                    // Allow combined short options like -snrvm
                    for c in arg[1..].chars() {
                        match c {
                            'a' => show_all = true,
                            's' => show_sysname = true,
                            'n' => show_nodename = true,
                            'r' => show_release = true,
                            'v' => show_version = true,
                            'm' => show_machine = true,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }

        // Default to showing sysname if no options
        if !show_all && !show_sysname && !show_nodename && !show_release && !show_version && !show_machine {
            show_sysname = true;
        }

        if show_all {
            show_sysname = true;
            show_nodename = true;
            show_release = true;
            show_version = true;
            show_machine = true;
        }

        let mut record = Vec::new();

        #[cfg(unix)]
        {
            use nix::libc;
            use std::ffi::CStr;

            let mut utsname: libc::utsname = unsafe { std::mem::zeroed() };
            let ret = unsafe { libc::uname(&mut utsname) };

            if ret == 0 {
                if show_sysname {
                    let s = unsafe { CStr::from_ptr(utsname.sysname.as_ptr()) };
                    record.push((
                        "sysname".to_string(),
                        Value::String(s.to_string_lossy().to_string()),
                    ));
                }
                if show_nodename {
                    let s = unsafe { CStr::from_ptr(utsname.nodename.as_ptr()) };
                    record.push((
                        "nodename".to_string(),
                        Value::String(s.to_string_lossy().to_string()),
                    ));
                }
                if show_release {
                    let s = unsafe { CStr::from_ptr(utsname.release.as_ptr()) };
                    record.push((
                        "release".to_string(),
                        Value::String(s.to_string_lossy().to_string()),
                    ));
                }
                if show_version {
                    let s = unsafe { CStr::from_ptr(utsname.version.as_ptr()) };
                    record.push((
                        "version".to_string(),
                        Value::String(s.to_string_lossy().to_string()),
                    ));
                }
                if show_machine {
                    let s = unsafe { CStr::from_ptr(utsname.machine.as_ptr()) };
                    record.push((
                        "machine".to_string(),
                        Value::String(s.to_string_lossy().to_string()),
                    ));
                }
            }
        }

        #[cfg(not(unix))]
        {
            if show_sysname {
                record.push(("sysname".to_string(), Value::String("Unknown".to_string())));
            }
            if show_nodename {
                record.push(("nodename".to_string(), Value::String("localhost".to_string())));
            }
            if show_release {
                record.push(("release".to_string(), Value::String("0.0.0".to_string())));
            }
            if show_version {
                record.push(("version".to_string(), Value::String("Unknown".to_string())));
            }
            if show_machine {
                record.push(("machine".to_string(), Value::String("unknown".to_string())));
            }
        }

        Ok(Value::Record(record))
    }
}

// ============================================================================
// umask - Display or set file mode creation mask
// ============================================================================

pub struct UmaskCommand;

impl NexusCommand for UmaskCommand {
    fn name(&self) -> &'static str {
        "umask"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let symbolic = args.iter().any(|a| a == "-S");
        let args: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();

        #[cfg(unix)]
        {
            use nix::sys::stat::{umask, Mode};

            if args.is_empty() {
                // Display current umask
                // We need to set and restore to get the current value
                let current = umask(Mode::empty());
                umask(current); // Restore

                let mask = current.bits() as u32;

                if symbolic {
                    // Symbolic format: u=rwx,g=rx,o=rx
                    let u = 0o777 ^ mask;
                    let ur = if u & 0o400 != 0 { "r" } else { "" };
                    let uw = if u & 0o200 != 0 { "w" } else { "" };
                    let ux = if u & 0o100 != 0 { "x" } else { "" };
                    let gr = if u & 0o040 != 0 { "r" } else { "" };
                    let gw = if u & 0o020 != 0 { "w" } else { "" };
                    let gx = if u & 0o010 != 0 { "x" } else { "" };
                    let or = if u & 0o004 != 0 { "r" } else { "" };
                    let ow = if u & 0o002 != 0 { "w" } else { "" };
                    let ox = if u & 0o001 != 0 { "x" } else { "" };

                    Ok(Value::String(format!(
                        "u={}{}{},g={}{}{},o={}{}{}",
                        ur, uw, ux, gr, gw, gx, or, ow, ox
                    )))
                } else {
                    Ok(Value::Int(mask as i64))
                }
            } else {
                // Set new umask
                let new_mask: u16 = u16::from_str_radix(args[0], 8)
                    .map_err(|_| anyhow::anyhow!("umask: '{}': invalid octal number", args[0]))?;

                let mode = Mode::from_bits_truncate(new_mask as nix::libc::mode_t);
                let old = umask(mode);

                Ok(Value::Int(old.bits() as i64))
            }
        }

        #[cfg(not(unix))]
        {
            // On non-Unix, just return a default
            Ok(Value::Int(0o022))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    /// umask is process-wide, so tests that read/write it must not run in parallel.
    static UMASK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_tty_command_name() {
        let cmd = TtyCommand;
        assert_eq!(cmd.name(), "tty");
    }

    #[test]
    fn test_uname_command_name() {
        let cmd = UnameCommand;
        assert_eq!(cmd.name(), "uname");
    }

    #[test]
    fn test_umask_command_name() {
        let cmd = UmaskCommand;
        assert_eq!(cmd.name(), "umask");
    }

    #[test]
    fn test_tty_silent_mode() {
        let cmd = TtyCommand;
        let mut test_ctx = TestContext::new_default();

        // In silent mode, should return Bool (true if tty, false if not)
        let result = cmd
            .execute(&["-s".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::Bool(_) => {} // Either true or false is valid
            _ => panic!("Expected Bool in silent mode"),
        }
    }

    #[test]
    fn test_uname_default_shows_sysname() {
        let cmd = UnameCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Record(fields) => {
                // Default should show sysname only
                assert_eq!(fields.len(), 1);
                assert!(fields.iter().any(|(k, _)| k == "sysname"));
            }
            _ => panic!("Expected Record"),
        }
    }

    #[test]
    fn test_uname_all() {
        let cmd = UnameCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd
            .execute(&["-a".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::Record(fields) => {
                // -a should show all fields
                let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
                assert!(keys.contains(&"sysname"));
                assert!(keys.contains(&"nodename"));
                assert!(keys.contains(&"release"));
                assert!(keys.contains(&"version"));
                assert!(keys.contains(&"machine"));
            }
            _ => panic!("Expected Record"),
        }
    }

    #[test]
    fn test_uname_combined_options() {
        let cmd = UnameCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd
            .execute(&["-sn".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::Record(fields) => {
                // -sn should show sysname and nodename
                let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
                assert!(keys.contains(&"sysname"));
                assert!(keys.contains(&"nodename"));
                assert!(!keys.contains(&"release"));
            }
            _ => panic!("Expected Record"),
        }
    }

    #[test]
    fn test_uname_single_option() {
        let cmd = UnameCommand;
        let mut test_ctx = TestContext::new_default();

        // Test -m (machine)
        let result = cmd
            .execute(&["-m".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::Record(fields) => {
                assert_eq!(fields.len(), 1);
                assert!(fields.iter().any(|(k, _)| k == "machine"));
            }
            _ => panic!("Expected Record"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_umask_get() {
        let _lock = UMASK_LOCK.lock().unwrap();
        let cmd = UmaskCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Int(mask) => {
                // Mask should be a valid octal value (typically 022, 077, etc.)
                assert!(mask >= 0 && mask <= 0o777);
            }
            _ => panic!("Expected Int"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_umask_symbolic() {
        let _lock = UMASK_LOCK.lock().unwrap();
        let cmd = UmaskCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd
            .execute(&["-S".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::String(s) => {
                // Should be in format u=rwx,g=rwx,o=rwx
                assert!(s.starts_with("u="));
                assert!(s.contains(",g="));
                assert!(s.contains(",o="));
            }
            _ => panic!("Expected String for symbolic mode"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_umask_set_and_restore() {
        let _lock = UMASK_LOCK.lock().unwrap();
        use nix::sys::stat::{umask, Mode};

        // Save original umask
        let original = umask(Mode::empty());
        umask(original);

        let cmd = UmaskCommand;
        let mut test_ctx = TestContext::new_default();

        // Set a new umask
        let result = cmd
            .execute(&["077".to_string()], &mut test_ctx.ctx())
            .unwrap();

        // Should return the old mask
        match result {
            Value::Int(old_mask) => {
                assert_eq!(old_mask, original.bits() as i64);
            }
            _ => panic!("Expected Int for old mask"),
        }

        // Verify new mask was set
        let current = umask(Mode::empty());
        umask(current);
        assert_eq!(current.bits(), 0o077);

        // Restore original
        umask(original);
    }

    #[test]
    fn test_umask_invalid_octal() {
        let cmd = UmaskCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&["999".to_string()], &mut test_ctx.ctx());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid octal"));
    }
}
