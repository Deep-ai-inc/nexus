//! `ln` — create hard or symbolic links.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::path::PathBuf;

pub struct LnCommand;

impl NexusCommand for LnCommand {
    fn name(&self) -> &'static str {
        "ln"
    }

    fn description(&self) -> &'static str {
        "Create hard or symbolic links"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut symbolic = false;
        let mut force = false;
        let mut paths = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-s" | "--symbolic" => symbolic = true,
                "-f" | "--force" => force = true,
                "-sf" | "-fs" => {
                    symbolic = true;
                    force = true;
                }
                s if !s.starts_with('-') => paths.push(s.to_string()),
                _ => {}
            }
        }

        if paths.len() < 2 {
            anyhow::bail!(
                "ln: missing destination file operand after '{}'",
                paths.first().unwrap_or(&String::new())
            );
        }

        let link_name = paths.pop().unwrap();
        let link_path = if PathBuf::from(&link_name).is_absolute() {
            PathBuf::from(&link_name)
        } else {
            ctx.state.cwd.join(&link_name)
        };

        // If destination is a directory, create links inside it
        let dest_is_dir = link_path.is_dir();

        for target in &paths {
            let target_path = if PathBuf::from(target).is_absolute() {
                PathBuf::from(target)
            } else {
                ctx.state.cwd.join(target)
            };

            let actual_link = if dest_is_dir {
                link_path.join(
                    target_path
                        .file_name()
                        .ok_or_else(|| anyhow::anyhow!("ln: cannot determine filename"))?,
                )
            } else {
                link_path.clone()
            };

            if force && actual_link.exists() {
                std::fs::remove_file(&actual_link)?;
            }

            if symbolic {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&target_path, &actual_link)?;
                #[cfg(not(unix))]
                anyhow::bail!("ln -s: symbolic links not supported on this platform");
            } else {
                std::fs::hard_link(&target_path, &actual_link)?;
            }
        }

        Ok(Value::Unit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;
    use std::io::Write;
    use tempfile::TempDir;

    fn setup() -> TempDir {
        let dir = TempDir::new().unwrap();
        let mut f = std::fs::File::create(dir.path().join("target.txt")).unwrap();
        f.write_all(b"hello").unwrap();
        dir
    }

    #[test]
    fn test_ln_hard_link() {
        let dir = setup();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = LnCommand;
        let result = cmd
            .execute(
                &["target.txt".to_string(), "link.txt".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        assert!(matches!(result, Value::Unit));
        assert!(dir.path().join("link.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("link.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn test_ln_symbolic() {
        let dir = setup();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = LnCommand;
        let result = cmd
            .execute(
                &[
                    "-s".to_string(),
                    "target.txt".to_string(),
                    "symlink.txt".to_string(),
                ],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        assert!(matches!(result, Value::Unit));
        let link = dir.path().join("symlink.txt");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[test]
    fn test_ln_force() {
        let dir = setup();
        // Create existing file at link location
        std::fs::write(dir.path().join("link.txt"), "old").unwrap();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = LnCommand;
        let result = cmd
            .execute(
                &[
                    "-sf".to_string(),
                    "target.txt".to_string(),
                    "link.txt".to_string(),
                ],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        assert!(matches!(result, Value::Unit));
    }

    #[test]
    fn test_ln_missing_operand() {
        let mut test_ctx = TestContext::new_default();
        let cmd = LnCommand;
        assert!(cmd.execute(&[], &mut test_ctx.ctx()).is_err());
        assert!(cmd
            .execute(&["only_one".to_string()], &mut test_ctx.ctx())
            .is_err());
    }
}
