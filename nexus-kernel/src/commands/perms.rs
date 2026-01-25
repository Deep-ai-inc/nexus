//! Permission commands - chmod, chown, chgrp.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
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
                arg if arg.starts_with('-') => {
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

        let mode = parse_mode(mode_str)?;

        for file in files {
            let path = resolve_path(file, ctx);
            chmod_file(&path, mode, recursive)?;
        }

        Ok(Value::Unit)
    }
}

fn chmod_file(path: &PathBuf, mode: u32, recursive: bool) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(path)?;

    // Set permissions
    let mut perms = metadata.permissions();
    perms.set_mode(mode);
    std::fs::set_permissions(path, perms)?;

    if recursive && metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            chmod_file(&entry.path(), mode, true)?;
        }
    }

    Ok(())
}

/// Parse an octal or symbolic mode string.
fn parse_mode(s: &str) -> anyhow::Result<u32> {
    // Try octal first
    if let Ok(mode) = u32::from_str_radix(s, 8) {
        return Ok(mode);
    }

    // Symbolic mode (simplified: only handles +x, -x, etc.)
    // Full symbolic mode parsing is complex; implement basic support
    anyhow::bail!(
        "chmod: invalid mode: '{}' (only octal modes supported currently)",
        s
    )
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

        for file in files {
            let path = resolve_path(file, ctx);
            chown_file(&path, uid, gid, recursive)?;
        }

        Ok(Value::Unit)
    }
}

fn chown_file(path: &PathBuf, uid: Option<u32>, gid: Option<u32>, recursive: bool) -> anyhow::Result<()> {
    chown(path, uid, gid)?;

    if recursive {
        let metadata = std::fs::symlink_metadata(path)?;
        if metadata.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                chown_file(&entry.path(), uid, gid, true)?;
            }
        }
    }

    Ok(())
}

/// Parse owner[:group] specification.
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
            // Try as numeric UID first
            o.parse::<u32>().map_err(|_| {
                anyhow::anyhow!("chown: invalid user: '{}' (only numeric UIDs supported)", o)
            })
        })
        .transpose()?;

    let gid = group
        .map(|g| {
            // Try as numeric GID first
            g.parse::<u32>().map_err(|_| {
                anyhow::anyhow!("chown: invalid group: '{}' (only numeric GIDs supported)", g)
            })
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

        for file in files {
            let path = resolve_path(file, ctx);
            chown_file(&path, None, Some(gid), recursive)?;
        }

        Ok(Value::Unit)
    }
}

/// Parse a group specification (name or GID).
fn parse_group(s: &str) -> anyhow::Result<u32> {
    // Try as numeric GID first (only numeric GIDs supported for simplicity)
    s.parse::<u32>().map_err(|_| {
        anyhow::anyhow!("chgrp: invalid group: '{}' (only numeric GIDs supported)", s)
    })
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
        assert_eq!(parse_mode("755").unwrap(), 0o755);
        assert_eq!(parse_mode("644").unwrap(), 0o644);
        assert_eq!(parse_mode("777").unwrap(), 0o777);
        assert_eq!(parse_mode("000").unwrap(), 0o000);
    }

    #[test]
    fn test_parse_mode_invalid() {
        // Symbolic modes not supported yet
        assert!(parse_mode("+x").is_err());
        assert!(parse_mode("u+x").is_err());
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
    fn test_parse_group_invalid() {
        assert!(parse_group("invalid").is_err());
    }
}
