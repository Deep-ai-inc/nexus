//! Filesystem commands - touch, mkdir, rm, rmdir, cp, mv.

use super::{CommandContext, NexusCommand};
use nexus_api::{FileOpError, FileOpInfo, FileOpKind, FileOpPhase, ShellEvent, Value};
use std::fs;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// ============================================================================
// touch - create files or update timestamps
// ============================================================================

pub struct TouchCommand;

impl NexusCommand for TouchCommand {
    fn name(&self) -> &'static str {
        "touch"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut no_create = false;
        let mut files = Vec::new();

        for arg in args {
            if arg == "-c" || arg == "--no-create" {
                no_create = true;
            } else if !arg.starts_with('-') {
                files.push(arg.clone());
            }
        }

        if files.is_empty() {
            return Err(anyhow::anyhow!("touch: missing file operand"));
        }

        let mut created = Vec::new();

        for file in files {
            let path = if PathBuf::from(&file).is_absolute() {
                PathBuf::from(&file)
            } else {
                ctx.state.cwd.join(&file)
            };

            if path.exists() {
                // Update modification time
                let now = filetime::FileTime::now();
                filetime::set_file_mtime(&path, now)?;
            } else if !no_create {
                // Create the file
                fs::File::create(&path)?;
                created.push(Value::Path(path));
            }
        }

        if created.is_empty() {
            Ok(Value::Unit)
        } else if created.len() == 1 {
            Ok(created.into_iter().next().unwrap())
        } else {
            Ok(Value::List(created))
        }
    }
}

// ============================================================================
// mkdir - create directories
// ============================================================================

pub struct MkdirCommand;

impl NexusCommand for MkdirCommand {
    fn name(&self) -> &'static str {
        "mkdir"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut parents = false;
        let mut dirs = Vec::new();

        for arg in args {
            if arg == "-p" || arg == "--parents" {
                parents = true;
            } else if !arg.starts_with('-') {
                dirs.push(arg.clone());
            }
        }

        if dirs.is_empty() {
            return Err(anyhow::anyhow!("mkdir: missing operand"));
        }

        let mut created = Vec::new();

        for dir in dirs {
            let path = if PathBuf::from(&dir).is_absolute() {
                PathBuf::from(&dir)
            } else {
                ctx.state.cwd.join(&dir)
            };

            if parents {
                fs::create_dir_all(&path)?;
            } else {
                fs::create_dir(&path)?;
            }

            created.push(Value::Path(path));
        }

        if created.len() == 1 {
            Ok(created.into_iter().next().unwrap())
        } else {
            Ok(Value::List(created))
        }
    }
}

// ============================================================================
// rm - remove files
// ============================================================================

pub struct RmCommand;

impl NexusCommand for RmCommand {
    fn name(&self) -> &'static str {
        "rm"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut recursive = false;
        let mut force = false;
        let mut targets = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-r" | "-R" | "--recursive" => recursive = true,
                "-f" | "--force" => force = true,
                "-rf" | "-fr" => {
                    recursive = true;
                    force = true;
                }
                s if !s.starts_with('-') => targets.push(arg.clone()),
                _ => {}
            }
        }

        if targets.is_empty() {
            if !force {
                return Err(anyhow::anyhow!("rm: missing operand"));
            }
            return Ok(Value::Unit);
        }

        let start_time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let sources: Vec<PathBuf> = targets.iter().map(|t| {
            if PathBuf::from(t).is_absolute() { PathBuf::from(t) } else { ctx.state.cwd.join(t) }
        }).collect();

        let mut info = FileOpInfo {
            op_type: FileOpKind::Remove,
            phase: FileOpPhase::Executing,
            sources: sources.clone(),
            dest: None,
            total_bytes: None,
            bytes_processed: 0,
            files_total: Some(targets.len()),
            files_processed: 0,
            current_file: None,
            start_time_ms,
            errors: Vec::new(),
        };

        for target in &targets {
            let path = if PathBuf::from(target).is_absolute() {
                PathBuf::from(target)
            } else {
                ctx.state.cwd.join(target)
            };

            if !path.exists() {
                if !force {
                    info.errors.push(FileOpError {
                        path: path.clone(),
                        message: "No such file or directory".to_string(),
                    });
                }
                continue;
            }

            info.current_file = Some(path.clone());

            if path.is_dir() {
                if recursive {
                    match fs::remove_dir_all(&path) {
                        Ok(()) => info.files_processed += 1,
                        Err(e) => info.errors.push(FileOpError {
                            path: path.clone(),
                            message: e.to_string(),
                        }),
                    }
                } else {
                    info.errors.push(FileOpError {
                        path: path.clone(),
                        message: "Is a directory".to_string(),
                    });
                }
            } else {
                match fs::remove_file(&path) {
                    Ok(()) => info.files_processed += 1,
                    Err(e) => info.errors.push(FileOpError {
                        path: path.clone(),
                        message: e.to_string(),
                    }),
                }
            }
        }

        info.phase = if info.errors.is_empty() {
            FileOpPhase::Completed
        } else if info.files_processed > 0 {
            // Partial success
            FileOpPhase::Completed
        } else {
            FileOpPhase::Failed
        };
        info.current_file = None;

        // For backward compatibility: if no errors and simple rm, return Unit
        if info.errors.is_empty() && targets.len() <= 2 {
            return Ok(Value::Unit);
        }

        Ok(Value::file_op(info))
    }
}

// ============================================================================
// rmdir - remove empty directories
// ============================================================================

pub struct RmdirCommand;

impl NexusCommand for RmdirCommand {
    fn name(&self) -> &'static str {
        "rmdir"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut parents = false;
        let mut dirs = Vec::new();

        for arg in args {
            if arg == "-p" || arg == "--parents" {
                parents = true;
            } else if !arg.starts_with('-') {
                dirs.push(arg.clone());
            }
        }

        if dirs.is_empty() {
            return Err(anyhow::anyhow!("rmdir: missing operand"));
        }

        for dir in dirs {
            let path = if PathBuf::from(&dir).is_absolute() {
                PathBuf::from(&dir)
            } else {
                ctx.state.cwd.join(&dir)
            };

            if parents {
                // Remove directory and all empty parent directories
                let mut current = path;
                while current != ctx.state.cwd {
                    if current.exists() {
                        fs::remove_dir(&current)?;
                    }
                    current = match current.parent() {
                        Some(p) => p.to_path_buf(),
                        None => break,
                    };
                }
            } else {
                fs::remove_dir(&path)?;
            }
        }

        Ok(Value::Unit)
    }
}

// ============================================================================
// cp - copy files
// ============================================================================

pub struct CpCommand;

impl NexusCommand for CpCommand {
    fn name(&self) -> &'static str {
        "cp"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut recursive = false;
        let mut paths = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-r" | "-R" | "--recursive" => recursive = true,
                s if !s.starts_with('-') => paths.push(arg.clone()),
                _ => {}
            }
        }

        if paths.len() < 2 {
            return Err(anyhow::anyhow!("cp: missing destination file operand after '{}'", paths.first().unwrap_or(&String::new())));
        }

        let dest = paths.pop().unwrap();
        let dest_path = if PathBuf::from(&dest).is_absolute() {
            PathBuf::from(&dest)
        } else {
            ctx.state.cwd.join(&dest)
        };

        let dest_is_dir = dest_path.is_dir();
        let start_time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let sources: Vec<PathBuf> = paths.iter().map(|s| {
            if PathBuf::from(s).is_absolute() { PathBuf::from(s) } else { ctx.state.cwd.join(s) }
        }).collect();

        let mut info = FileOpInfo {
            op_type: FileOpKind::Copy,
            phase: FileOpPhase::Planning,
            sources: sources.clone(),
            dest: Some(dest_path.clone()),
            total_bytes: None,
            bytes_processed: 0,
            files_total: None,
            files_processed: 0,
            current_file: None,
            start_time_ms,
            errors: Vec::new(),
        };

        // Planning phase: scan sizes
        let mut seq_counter: u64 = 0;
        let mut total_bytes: u64 = 0;
        let mut total_files: usize = 0;
        let mut last_emit = Instant::now();

        for src_path in &sources {
            if !src_path.exists() {
                info.errors.push(FileOpError {
                    path: src_path.clone(),
                    message: "No such file or directory".to_string(),
                });
                continue;
            }
            count_recursive(src_path, &mut total_bytes, &mut total_files);
        }

        if !info.errors.is_empty() && sources.len() == info.errors.len() {
            info.phase = FileOpPhase::Failed;
            return Ok(Value::file_op(info));
        }

        info.total_bytes = Some(total_bytes);
        info.files_total = Some(total_files);
        info.phase = FileOpPhase::Executing;

        // Emit planning complete
        seq_counter += 1;
        let _ = ctx.events.send(ShellEvent::StreamingUpdate {
            block_id: ctx.block_id,
            seq: seq_counter,
            update: Value::file_op(info.clone()),
            coalesce: true,
        });

        // Execution phase
        for src in &paths {
            let src_path = if PathBuf::from(src).is_absolute() {
                PathBuf::from(src)
            } else {
                ctx.state.cwd.join(src)
            };

            if !src_path.exists() {
                continue; // Already recorded error
            }

            let target = if dest_is_dir {
                dest_path.join(src_path.file_name().unwrap_or_default())
            } else {
                dest_path.clone()
            };

            if src_path.is_dir() {
                if !recursive {
                    info.errors.push(FileOpError {
                        path: src_path.clone(),
                        message: "-r not specified; omitting directory".to_string(),
                    });
                    continue;
                }
                copy_dir_with_progress(
                    &src_path, &target, &mut info, ctx, &mut seq_counter, &mut last_emit,
                );
            } else {
                info.current_file = Some(src_path.clone());
                match fs::copy(&src_path, &target) {
                    Ok(bytes) => {
                        info.bytes_processed += bytes;
                        info.files_processed += 1;
                    }
                    Err(e) => {
                        info.errors.push(FileOpError {
                            path: src_path.clone(),
                            message: e.to_string(),
                        });
                    }
                }

                // Throttled emit
                if last_emit.elapsed().as_millis() >= 100 {
                    seq_counter += 1;
                    let _ = ctx.events.send(ShellEvent::StreamingUpdate {
                        block_id: ctx.block_id,
                        seq: seq_counter,
                        update: Value::file_op(info.clone()),
                        coalesce: true,
                    });
                    last_emit = Instant::now();
                }
            }
        }

        info.phase = if info.errors.is_empty() {
            FileOpPhase::Completed
        } else {
            FileOpPhase::Failed
        };
        info.current_file = None;

        Ok(Value::file_op(info))
    }
}

fn count_recursive(path: &PathBuf, total_bytes: &mut u64, total_files: &mut usize) {
    if path.is_file() {
        *total_bytes += fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        *total_files += 1;
    } else if path.is_dir() {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                count_recursive(&entry.path(), total_bytes, total_files);
            }
        }
    }
}

fn copy_dir_with_progress(
    src: &PathBuf,
    dest: &PathBuf,
    info: &mut FileOpInfo,
    ctx: &mut CommandContext,
    seq_counter: &mut u64,
    last_emit: &mut Instant,
) {
    if let Err(e) = fs::create_dir_all(dest) {
        info.errors.push(FileOpError {
            path: dest.clone(),
            message: e.to_string(),
        });
        return;
    }

    let entries = match fs::read_dir(src) {
        Ok(e) => e,
        Err(e) => {
            info.errors.push(FileOpError {
                path: src.clone(),
                message: e.to_string(),
            });
            return;
        }
    };

    for entry in entries.flatten() {
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_with_progress(&src_path, &dest_path, info, ctx, seq_counter, last_emit);
        } else {
            info.current_file = Some(src_path.clone());
            match fs::copy(&src_path, &dest_path) {
                Ok(bytes) => {
                    info.bytes_processed += bytes;
                    info.files_processed += 1;
                }
                Err(e) => {
                    info.errors.push(FileOpError {
                        path: src_path.clone(),
                        message: e.to_string(),
                    });
                }
            }

            // Throttled emit
            if last_emit.elapsed().as_millis() >= 100 {
                *seq_counter += 1;
                let _ = ctx.events.send(ShellEvent::StreamingUpdate {
                    block_id: ctx.block_id,
                    seq: *seq_counter,
                    update: Value::file_op(info.clone()),
                    coalesce: true,
                });
                *last_emit = Instant::now();
            }
        }
    }
}

// ============================================================================
// mv - move/rename files
// ============================================================================

pub struct MvCommand;

impl NexusCommand for MvCommand {
    fn name(&self) -> &'static str {
        "mv"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut force = false;
        let mut paths = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-f" | "--force" => force = true,
                s if !s.starts_with('-') => paths.push(arg.clone()),
                _ => {}
            }
        }

        if paths.len() < 2 {
            return Err(anyhow::anyhow!("mv: missing destination file operand after '{}'", paths.first().unwrap_or(&String::new())));
        }

        let dest = paths.pop().unwrap();
        let dest_path = if PathBuf::from(&dest).is_absolute() {
            PathBuf::from(&dest)
        } else {
            ctx.state.cwd.join(&dest)
        };

        let dest_is_dir = dest_path.is_dir();
        let start_time_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let sources: Vec<PathBuf> = paths.iter().map(|s| {
            if PathBuf::from(s).is_absolute() { PathBuf::from(s) } else { ctx.state.cwd.join(s) }
        }).collect();

        let mut info = FileOpInfo {
            op_type: FileOpKind::Move,
            phase: FileOpPhase::Executing,
            sources: sources.clone(),
            dest: Some(dest_path.clone()),
            total_bytes: None,
            bytes_processed: 0,
            files_total: Some(sources.len()),
            files_processed: 0,
            current_file: None,
            start_time_ms,
            errors: Vec::new(),
        };

        for src in &paths {
            let src_path = if PathBuf::from(src).is_absolute() {
                PathBuf::from(src)
            } else {
                ctx.state.cwd.join(src)
            };

            if !src_path.exists() {
                info.errors.push(FileOpError {
                    path: src_path.clone(),
                    message: "No such file or directory".to_string(),
                });
                continue;
            }

            let target = if dest_is_dir {
                dest_path.join(src_path.file_name().unwrap_or_default())
            } else {
                dest_path.clone()
            };

            if target.exists() && !force {
                // In interactive mode we'd ask; here we just proceed
            }

            info.current_file = Some(src_path.clone());

            // Try rename first (fast path, same filesystem)
            match fs::rename(&src_path, &target) {
                Ok(()) => {
                    info.files_processed += 1;
                }
                Err(rename_err) => {
                    // Only fallback to copy+delete on cross-device errors (EXDEV).
                    // Other errors (EACCES, ENOENT, etc.) should be reported directly.
                    // EXDEV = 18 on both macOS and Linux
                    let is_cross_device = rename_err.raw_os_error() == Some(18);
                    if !is_cross_device {
                        info.errors.push(FileOpError {
                            path: src_path.clone(),
                            message: rename_err.to_string(),
                        });
                        continue;
                    }
                    // Fallback: copy + delete (cross-filesystem move)
                    if src_path.is_dir() {
                        let mut seq: u64 = 0;
                        let mut last_emit = Instant::now();
                        copy_dir_with_progress(
                            &src_path, &target, &mut info, ctx, &mut seq, &mut last_emit,
                        );
                        if info.errors.is_empty() {
                            let _ = fs::remove_dir_all(&src_path);
                        }
                    } else {
                        match fs::copy(&src_path, &target) {
                            Ok(_) => {
                                let _ = fs::remove_file(&src_path);
                                info.files_processed += 1;
                            }
                            Err(e) => {
                                info.errors.push(FileOpError {
                                    path: src_path.clone(),
                                    message: e.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        info.phase = if info.errors.is_empty() {
            FileOpPhase::Completed
        } else {
            FileOpPhase::Failed
        };
        info.current_file = None;

        Ok(Value::file_op(info))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;
    use std::io::Write;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        let mut f = fs::File::create(dir.path().join("src.txt")).unwrap();
        f.write_all(b"hello world").unwrap();

        fs::create_dir(dir.path().join("subdir")).unwrap();
        let mut f2 = fs::File::create(dir.path().join("subdir/nested.txt")).unwrap();
        f2.write_all(b"nested content").unwrap();

        dir
    }

    #[test]
    fn test_cp_single_file() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = CpCommand;
        let result = cmd
            .execute(
                &["src.txt".to_string(), "dst.txt".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result.as_domain() {
            Some(nexus_api::DomainValue::FileOp(info)) => {
                assert!(matches!(info.op_type, FileOpKind::Copy));
                assert!(matches!(info.phase, FileOpPhase::Completed));
                assert!(info.errors.is_empty());
                assert_eq!(info.files_processed, 1);
            }
            _ => panic!("Expected FileOp"),
        }

        assert!(dir.path().join("dst.txt").exists());
        assert_eq!(fs::read_to_string(dir.path().join("dst.txt")).unwrap(), "hello world");
    }

    #[test]
    fn test_cp_recursive() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = CpCommand;
        let result = cmd
            .execute(
                &["-r".to_string(), "subdir".to_string(), "subdir_copy".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result.as_domain() {
            Some(nexus_api::DomainValue::FileOp(info)) => {
                assert!(matches!(info.phase, FileOpPhase::Completed));
                assert!(info.errors.is_empty());
            }
            _ => panic!("Expected FileOp"),
        }

        assert!(dir.path().join("subdir_copy/nested.txt").exists());
    }

    #[test]
    fn test_cp_dir_without_recursive() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = CpCommand;
        let result = cmd
            .execute(
                &["subdir".to_string(), "subdir_copy".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result.as_domain() {
            Some(nexus_api::DomainValue::FileOp(info)) => {
                // Should have error about -r not specified
                assert!(!info.errors.is_empty());
            }
            _ => panic!("Expected FileOp"),
        }
    }

    #[test]
    fn test_cp_nonexistent_source() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = CpCommand;
        let result = cmd
            .execute(
                &["nonexistent.txt".to_string(), "dst.txt".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result.as_domain() {
            Some(nexus_api::DomainValue::FileOp(info)) => {
                assert!(matches!(info.phase, FileOpPhase::Failed));
                assert!(!info.errors.is_empty());
            }
            _ => panic!("Expected FileOp"),
        }
    }

    #[test]
    fn test_mv_file() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = MvCommand;
        let result = cmd
            .execute(
                &["src.txt".to_string(), "moved.txt".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result.as_domain() {
            Some(nexus_api::DomainValue::FileOp(info)) => {
                assert!(matches!(info.op_type, FileOpKind::Move));
                assert!(matches!(info.phase, FileOpPhase::Completed));
                assert!(info.errors.is_empty());
                assert_eq!(info.files_processed, 1);
            }
            _ => panic!("Expected FileOp"),
        }

        assert!(!dir.path().join("src.txt").exists());
        assert!(dir.path().join("moved.txt").exists());
    }

    #[test]
    fn test_mv_nonexistent() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = MvCommand;
        let result = cmd
            .execute(
                &["nonexistent.txt".to_string(), "dst.txt".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result.as_domain() {
            Some(nexus_api::DomainValue::FileOp(info)) => {
                assert!(!info.errors.is_empty());
            }
            _ => panic!("Expected FileOp"),
        }
    }

    #[test]
    fn test_rm_file() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = RmCommand;
        let result = cmd
            .execute(&["src.txt".to_string()], &mut test_ctx.ctx())
            .unwrap();

        // Simple rm returns Unit for backward compatibility
        assert!(matches!(result, Value::Unit));
        assert!(!dir.path().join("src.txt").exists());
    }

    #[test]
    fn test_rm_recursive() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = RmCommand;
        let result = cmd
            .execute(
                &["-rf".to_string(), "subdir".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        assert!(matches!(result, Value::Unit));
        assert!(!dir.path().join("subdir").exists());
    }

    #[test]
    fn test_rm_dir_without_recursive() {
        let dir = setup_test_dir();
        let mut test_ctx = TestContext::new(dir.path().to_path_buf());

        let cmd = RmCommand;
        let result = cmd
            .execute(&["subdir".to_string()], &mut test_ctx.ctx())
            .unwrap();

        // Should have error about "Is a directory"
        match result.as_domain() {
            Some(nexus_api::DomainValue::FileOp(info)) => {
                assert!(!info.errors.is_empty());
                assert!(info.errors[0].message.contains("Is a directory"));
            }
            _ => panic!("Expected FileOp with error"),
        }
    }

    #[test]
    fn test_rm_missing_operand() {
        let mut test_ctx = TestContext::new_default();
        let cmd = RmCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());
    }
}
