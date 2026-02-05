//! The `which` command - locate a command.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::path::PathBuf;

pub struct WhichCommand;

impl NexusCommand for WhichCommand {
    fn name(&self) -> &'static str {
        "which"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut all = false;
        let mut commands = Vec::new();

        for arg in args {
            if arg == "-a" {
                all = true;
            } else if !arg.starts_with('-') {
                commands.push(arg.clone());
            }
        }

        if commands.is_empty() {
            return Ok(Value::Unit);
        }

        // Get PATH from environment
        let path_env = ctx.state.get_env("PATH").unwrap_or_default();
        let path_dirs: Vec<PathBuf> = path_env.split(':').map(PathBuf::from).collect();

        let mut results = Vec::new();

        for cmd in commands {
            // Check if it's a builtin/native command
            // This would require access to the command registry
            // For now, we'll skip this and just search PATH

            // Search in PATH directories
            let mut found = Vec::new();

            for dir in &path_dirs {
                let candidate = dir.join(&cmd);
                if candidate.exists() && is_executable(&candidate) {
                    found.push(Value::Path(candidate));
                    if !all {
                        break;
                    }
                }
            }

            if found.is_empty() {
                // Command not found - could return error or just skip
                // POSIX `which` returns exit code 1 for not found
            } else {
                results.extend(found);
            }
        }

        if results.is_empty() {
            Ok(Value::Unit)
        } else if results.len() == 1 {
            Ok(results.into_iter().next().unwrap())
        } else {
            Ok(Value::List(results))
        }
    }
}

#[cfg(unix)]
fn is_executable(path: &PathBuf) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(metadata) = path.metadata() {
        let mode = metadata.permissions().mode();
        mode & 0o111 != 0
    } else {
        false
    }
}

#[cfg(not(unix))]
fn is_executable(path: &PathBuf) -> bool {
    // On Windows, check for common executable extensions
    path.extension()
        .map(|ext| {
            let ext = ext.to_string_lossy().to_lowercase();
            matches!(ext.as_str(), "exe" | "cmd" | "bat" | "com")
        })
        .unwrap_or(false)
}

// ============================================================================
// type - describe a command (bash builtin style)
// ============================================================================

pub struct TypeCommand;

impl NexusCommand for TypeCommand {
    fn name(&self) -> &'static str {
        "type"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.is_empty() {
            return Ok(Value::Unit);
        }

        let path_env = ctx.state.get_env("PATH").unwrap_or_default();
        let path_dirs: Vec<PathBuf> = path_env.split(':').map(PathBuf::from).collect();

        let mut results = Vec::new();

        for cmd in args {
            if cmd.starts_with('-') {
                continue;
            }

            // Check builtins (hardcoded list for now)
            let builtins = [
                "cd", "exit", "export", "unset", "alias", "source", ".", "set",
            ];

            if builtins.contains(&cmd.as_str()) {
                results.push(Value::String(format!("{} is a shell builtin", cmd)));
                continue;
            }

            // Search in PATH
            let mut found = false;
            for dir in &path_dirs {
                let candidate = dir.join(cmd);
                if candidate.exists() && is_executable(&candidate) {
                    results.push(Value::String(format!(
                        "{} is {}",
                        cmd,
                        candidate.display()
                    )));
                    found = true;
                    break;
                }
            }

            if !found {
                results.push(Value::String(format!("{}: not found", cmd)));
            }
        }

        if results.len() == 1 {
            Ok(results.into_iter().next().unwrap())
        } else {
            Ok(Value::List(results))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::tempdir;

    // -------------------------------------------------------------------------
    // is_executable tests (Unix)
    // -------------------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn test_is_executable_with_exec_permission() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_exec");

        // Create file and set executable permission
        File::create(&file_path).unwrap();
        fs::set_permissions(&file_path, fs::Permissions::from_mode(0o755)).unwrap();

        assert!(is_executable(&file_path));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_executable_without_exec_permission() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_no_exec");

        // Create file with no execute permission
        File::create(&file_path).unwrap();
        fs::set_permissions(&file_path, fs::Permissions::from_mode(0o644)).unwrap();

        assert!(!is_executable(&file_path));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_executable_nonexistent() {
        let path = PathBuf::from("/nonexistent/path/to/file");
        assert!(!is_executable(&path));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_executable_group_exec() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_group_exec");

        // Create file with group execute permission
        File::create(&file_path).unwrap();
        fs::set_permissions(&file_path, fs::Permissions::from_mode(0o010)).unwrap();

        assert!(is_executable(&file_path));
    }

    #[cfg(unix)]
    #[test]
    fn test_is_executable_other_exec() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_other_exec");

        // Create file with other execute permission
        File::create(&file_path).unwrap();
        fs::set_permissions(&file_path, fs::Permissions::from_mode(0o001)).unwrap();

        assert!(is_executable(&file_path));
    }

    // -------------------------------------------------------------------------
    // TypeCommand builtin detection tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_type_command_name() {
        let cmd = TypeCommand;
        assert_eq!(cmd.name(), "type");
    }

    #[test]
    fn test_which_command_name() {
        let cmd = WhichCommand;
        assert_eq!(cmd.name(), "which");
    }
}
