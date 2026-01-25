//! Link commands - ln, link, unlink.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::path::PathBuf;

// ============================================================================
// ln - Create links between files
// ============================================================================

pub struct LnCommand;

impl NexusCommand for LnCommand {
    fn name(&self) -> &'static str {
        "ln"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.is_empty() {
            anyhow::bail!("usage: ln [-s] [-f] target [link_name]");
        }

        let mut symbolic = false;
        let mut force = false;
        let mut no_deref = false;
        let mut arg_start = 0;

        // Parse options
        for (i, arg) in args.iter().enumerate() {
            match arg.as_str() {
                "-s" | "--symbolic" => symbolic = true,
                "-f" | "--force" => force = true,
                "-n" | "--no-dereference" => no_deref = true,
                "-sf" | "-fs" => {
                    symbolic = true;
                    force = true;
                }
                arg if arg.starts_with('-') => {
                    anyhow::bail!("ln: unrecognized option: {}", arg);
                }
                _ => {
                    arg_start = i;
                    break;
                }
            }
        }

        let positional = &args[arg_start..];

        if positional.is_empty() {
            anyhow::bail!("ln: missing file operand");
        }

        let (target, link_name) = if positional.len() == 1 {
            // ln -s target -> creates link with same name in current dir
            let target = &positional[0];
            let link_name = PathBuf::from(target)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .ok_or_else(|| anyhow::anyhow!("ln: cannot determine link name"))?;
            (target.clone(), ctx.state.cwd.join(link_name))
        } else {
            // ln target link_name
            let target = positional[0].clone();
            let link_name = resolve_path(&positional[1], ctx);

            // If link_name is a directory, create link inside it
            if link_name.is_dir() && !no_deref {
                let target_name = PathBuf::from(&target)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .ok_or_else(|| anyhow::anyhow!("ln: cannot determine link name"))?;
                (target, link_name.join(target_name))
            } else {
                (target, link_name)
            }
        };

        // Handle force option
        if force && link_name.exists() {
            std::fs::remove_file(&link_name)?;
        }

        // Create the link
        if symbolic {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &link_name)?;
            #[cfg(not(unix))]
            anyhow::bail!("symbolic links not supported on this platform");
        } else {
            // Hard link - target must be resolved to absolute path
            let target_path = resolve_path(&target, ctx);
            std::fs::hard_link(&target_path, &link_name)?;
        }

        Ok(Value::Path(link_name))
    }
}

// ============================================================================
// link - Create a hard link (POSIX)
// ============================================================================

pub struct LinkCommand;

impl NexusCommand for LinkCommand {
    fn name(&self) -> &'static str {
        "link"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.len() != 2 {
            anyhow::bail!("usage: link file1 file2");
        }

        let source = resolve_path(&args[0], ctx);
        let dest = resolve_path(&args[1], ctx);

        std::fs::hard_link(&source, &dest)?;

        Ok(Value::Path(dest))
    }
}

// ============================================================================
// unlink - Remove a single file
// ============================================================================

pub struct UnlinkCommand;

impl NexusCommand for UnlinkCommand {
    fn name(&self) -> &'static str {
        "unlink"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.len() != 1 {
            anyhow::bail!("usage: unlink file");
        }

        let path = resolve_path(&args[0], ctx);

        // unlink should only work on files, not directories
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            anyhow::bail!("unlink: cannot unlink '{}': Is a directory", args[0]);
        }

        std::fs::remove_file(&path)?;

        Ok(Value::Unit)
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
    use crate::commands::test_utils::test_helpers::{create_test_file, TestContext};
    use tempfile::TempDir;

    #[test]
    fn test_ln_command_name() {
        let cmd = LnCommand;
        assert_eq!(cmd.name(), "ln");
    }

    #[test]
    fn test_link_command_name() {
        let cmd = LinkCommand;
        assert_eq!(cmd.name(), "link");
    }

    #[test]
    fn test_unlink_command_name() {
        let cmd = UnlinkCommand;
        assert_eq!(cmd.name(), "unlink");
    }

    #[test]
    fn test_ln_missing_operand() {
        let cmd = LnCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());
    }

    #[test]
    fn test_ln_hard_link() {
        let dir = TempDir::new().unwrap();
        let source = create_test_file(&dir, "source.txt", b"test content");

        let cmd = LnCommand;
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let result = cmd
            .execute(
                &[
                    source.to_string_lossy().to_string(),
                    dir.path().join("hardlink.txt").to_string_lossy().to_string(),
                ],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result {
            Value::Path(p) => {
                assert!(p.exists());
                assert_eq!(std::fs::read_to_string(&p).unwrap(), "test content");
            }
            _ => panic!("Expected Path"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_ln_symbolic_link() {
        let dir = TempDir::new().unwrap();
        let source = create_test_file(&dir, "source.txt", b"test content");

        let cmd = LnCommand;
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let link_path = dir.path().join("symlink.txt");
        let result = cmd
            .execute(
                &[
                    "-s".to_string(),
                    source.to_string_lossy().to_string(),
                    link_path.to_string_lossy().to_string(),
                ],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result {
            Value::Path(p) => {
                assert!(p.is_symlink());
            }
            _ => panic!("Expected Path"),
        }
    }

    #[test]
    fn test_link_creates_hard_link() {
        let dir = TempDir::new().unwrap();
        let source = create_test_file(&dir, "source.txt", b"link test");

        let cmd = LinkCommand;
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let dest_path = dir.path().join("dest.txt");
        let result = cmd
            .execute(
                &[
                    source.to_string_lossy().to_string(),
                    dest_path.to_string_lossy().to_string(),
                ],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result {
            Value::Path(p) => {
                assert!(p.exists());
                assert_eq!(std::fs::read_to_string(&p).unwrap(), "link test");
            }
            _ => panic!("Expected Path"),
        }
    }

    #[test]
    fn test_link_wrong_args() {
        let cmd = LinkCommand;
        let mut test_ctx = TestContext::new_default();

        // Too few arguments
        let result = cmd.execute(&["file1".to_string()], &mut test_ctx.ctx());
        assert!(result.is_err());

        // Too many arguments
        let result = cmd.execute(
            &[
                "file1".to_string(),
                "file2".to_string(),
                "file3".to_string(),
            ],
            &mut test_ctx.ctx(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_unlink_file() {
        let dir = TempDir::new().unwrap();
        let file = create_test_file(&dir, "to_delete.txt", b"delete me");
        assert!(file.exists());

        let cmd = UnlinkCommand;
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let result = cmd
            .execute(&[file.to_string_lossy().to_string()], &mut test_ctx.ctx())
            .unwrap();
        assert_eq!(result, Value::Unit);
        assert!(!file.exists());
    }

    #[test]
    fn test_unlink_directory_fails() {
        let dir = TempDir::new().unwrap();
        let subdir = dir.path().join("subdir");
        std::fs::create_dir(&subdir).unwrap();

        let cmd = UnlinkCommand;
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let result = cmd.execute(&[subdir.to_string_lossy().to_string()], &mut test_ctx.ctx());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Is a directory"));
    }

    #[test]
    fn test_unlink_wrong_args() {
        let cmd = UnlinkCommand;
        let mut test_ctx = TestContext::new_default();

        // No arguments
        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());

        // Too many arguments
        let result = cmd.execute(
            &["file1".to_string(), "file2".to_string()],
            &mut test_ctx.ctx(),
        );
        assert!(result.is_err());
    }
}
