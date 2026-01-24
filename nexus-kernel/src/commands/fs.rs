//! Filesystem commands - touch, mkdir, rm, rmdir, cp, mv.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::fs;
use std::path::PathBuf;

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

        for target in targets {
            let path = if PathBuf::from(&target).is_absolute() {
                PathBuf::from(&target)
            } else {
                ctx.state.cwd.join(&target)
            };

            if !path.exists() {
                if !force {
                    return Err(anyhow::anyhow!("rm: cannot remove '{}': No such file or directory", target));
                }
                continue;
            }

            if path.is_dir() {
                if recursive {
                    fs::remove_dir_all(&path)?;
                } else {
                    return Err(anyhow::anyhow!("rm: cannot remove '{}': Is a directory", target));
                }
            } else {
                fs::remove_file(&path)?;
            }
        }

        Ok(Value::Unit)
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

        for src in paths {
            let src_path = if PathBuf::from(&src).is_absolute() {
                PathBuf::from(&src)
            } else {
                ctx.state.cwd.join(&src)
            };

            if !src_path.exists() {
                return Err(anyhow::anyhow!("cp: cannot stat '{}': No such file or directory", src));
            }

            let target = if dest_is_dir {
                dest_path.join(src_path.file_name().unwrap_or_default())
            } else {
                dest_path.clone()
            };

            if src_path.is_dir() {
                if !recursive {
                    return Err(anyhow::anyhow!("cp: -r not specified; omitting directory '{}'", src));
                }
                copy_dir_recursive(&src_path, &target)?;
            } else {
                fs::copy(&src_path, &target)?;
            }
        }

        Ok(Value::Unit)
    }
}

fn copy_dir_recursive(src: &PathBuf, dest: &PathBuf) -> anyhow::Result<()> {
    fs::create_dir_all(dest)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dest_path)?;
        } else {
            fs::copy(&src_path, &dest_path)?;
        }
    }

    Ok(())
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

        for src in paths {
            let src_path = if PathBuf::from(&src).is_absolute() {
                PathBuf::from(&src)
            } else {
                ctx.state.cwd.join(&src)
            };

            if !src_path.exists() {
                return Err(anyhow::anyhow!("mv: cannot stat '{}': No such file or directory", src));
            }

            let target = if dest_is_dir {
                dest_path.join(src_path.file_name().unwrap_or_default())
            } else {
                dest_path.clone()
            };

            if target.exists() && !force {
                // In interactive mode we'd ask; here we just proceed
            }

            fs::rename(&src_path, &target)?;
        }

        Ok(Value::Unit)
    }
}

#[cfg(test)]
mod tests {
    // Tests would require temp directories
}
