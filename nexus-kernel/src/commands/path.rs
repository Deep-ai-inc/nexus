//! Path manipulation commands - basename, dirname, realpath, extname, stem.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::path::PathBuf;

// ============================================================================
// basename
// ============================================================================

pub struct BasenameCommand;

impl NexusCommand for BasenameCommand {
    fn name(&self) -> &'static str {
        "basename"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut suffix: Option<&str> = None;
        let mut paths = Vec::new();

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            if arg == "-s" || arg == "--suffix" {
                if i + 1 < args.len() {
                    suffix = Some(&args[i + 1]);
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-s") {
                suffix = Some(&arg[2..]);
            } else if !arg.starts_with('-') {
                paths.push(arg.clone());
            }
            i += 1;
        }

        // Handle piped input
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(basename_value(stdin_value, suffix));
        }

        if paths.is_empty() {
            return Ok(Value::Unit);
        }

        let results: Vec<Value> = paths
            .iter()
            .map(|p| {
                let path = PathBuf::from(p);
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();

                let name = if let Some(suf) = suffix {
                    name.strip_suffix(suf).unwrap_or(&name).to_string()
                } else {
                    name
                };

                Value::String(name)
            })
            .collect();

        if results.len() == 1 {
            Ok(results.into_iter().next().unwrap())
        } else {
            Ok(Value::List(results))
        }
    }
}

fn basename_value(value: Value, suffix: Option<&str>) -> Value {
    match value {
        Value::List(items) => Value::List(
            items
                .into_iter()
                .map(|item| basename_single(item, suffix))
                .collect(),
        ),
        Value::Path(p) => {
            let name = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let name = if let Some(suf) = suffix {
                name.strip_suffix(suf).unwrap_or(&name).to_string()
            } else {
                name
            };
            Value::String(name)
        }
        Value::FileEntry(entry) => {
            let name = if let Some(suf) = suffix {
                entry.name.strip_suffix(suf).unwrap_or(&entry.name).to_string()
            } else {
                entry.name.clone()
            };
            Value::String(name)
        }
        other => basename_single(other, suffix),
    }
}

fn basename_single(value: Value, suffix: Option<&str>) -> Value {
    let text = value.to_text();
    let path = PathBuf::from(&text);
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or(text);
    let name = if let Some(suf) = suffix {
        name.strip_suffix(suf).unwrap_or(&name).to_string()
    } else {
        name
    };
    Value::String(name)
}

// ============================================================================
// dirname
// ============================================================================

pub struct DirnameCommand;

impl NexusCommand for DirnameCommand {
    fn name(&self) -> &'static str {
        "dirname"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // Handle piped input
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(dirname_value(stdin_value));
        }

        let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();

        if paths.is_empty() {
            return Ok(Value::Unit);
        }

        let results: Vec<Value> = paths
            .iter()
            .map(|p| {
                let path = PathBuf::from(p);
                let parent = path
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| ".".to_string());
                Value::String(if parent.is_empty() { ".".to_string() } else { parent })
            })
            .collect();

        if results.len() == 1 {
            Ok(results.into_iter().next().unwrap())
        } else {
            Ok(Value::List(results))
        }
    }
}

fn dirname_value(value: Value) -> Value {
    match value {
        Value::List(items) => Value::List(items.into_iter().map(dirname_single).collect()),
        Value::Path(p) => {
            let parent = p
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            Value::Path(parent)
        }
        Value::FileEntry(entry) => {
            let parent = entry
                .path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            Value::Path(parent)
        }
        other => dirname_single(other),
    }
}

fn dirname_single(value: Value) -> Value {
    let text = value.to_text();
    let path = PathBuf::from(&text);
    let parent = path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| ".".to_string());
    Value::String(if parent.is_empty() {
        ".".to_string()
    } else {
        parent
    })
}

// ============================================================================
// realpath
// ============================================================================

pub struct RealpathCommand;

impl NexusCommand for RealpathCommand {
    fn name(&self) -> &'static str {
        "realpath"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // Handle piped input
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(realpath_value(stdin_value, &ctx.state.cwd));
        }

        let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();

        if paths.is_empty() {
            return Ok(Value::Unit);
        }

        let results: Vec<Value> = paths
            .iter()
            .filter_map(|p| {
                let path = if PathBuf::from(p).is_absolute() {
                    PathBuf::from(p)
                } else {
                    ctx.state.cwd.join(p)
                };
                path.canonicalize().ok().map(Value::Path)
            })
            .collect();

        if results.len() == 1 {
            Ok(results.into_iter().next().unwrap_or(Value::Unit))
        } else {
            Ok(Value::List(results))
        }
    }
}

fn realpath_value(value: Value, cwd: &PathBuf) -> Value {
    match value {
        Value::List(items) => Value::List(
            items
                .into_iter()
                .map(|item| realpath_single(item, cwd))
                .collect(),
        ),
        other => realpath_single(other, cwd),
    }
}

fn realpath_single(value: Value, cwd: &PathBuf) -> Value {
    let text = value.to_text();
    let path = if PathBuf::from(&text).is_absolute() {
        PathBuf::from(&text)
    } else {
        cwd.join(&text)
    };
    match path.canonicalize() {
        Ok(p) => Value::Path(p),
        Err(_) => Value::String(text),
    }
}

// ============================================================================
// extname - get file extension
// ============================================================================

pub struct ExtnameCommand;

impl NexusCommand for ExtnameCommand {
    fn name(&self) -> &'static str {
        "extname"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(extname_value(stdin_value));
        }

        let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();

        if paths.is_empty() {
            return Ok(Value::Unit);
        }

        let results: Vec<Value> = paths
            .iter()
            .map(|p| {
                let path = PathBuf::from(p);
                let ext = path
                    .extension()
                    .map(|s| format!(".{}", s.to_string_lossy()))
                    .unwrap_or_default();
                Value::String(ext)
            })
            .collect();

        if results.len() == 1 {
            Ok(results.into_iter().next().unwrap())
        } else {
            Ok(Value::List(results))
        }
    }
}

fn extname_value(value: Value) -> Value {
    match value {
        Value::List(items) => Value::List(items.into_iter().map(extname_single).collect()),
        Value::FileEntry(entry) => {
            let ext = PathBuf::from(&entry.name)
                .extension()
                .map(|s| format!(".{}", s.to_string_lossy()))
                .unwrap_or_default();
            Value::String(ext)
        }
        other => extname_single(other),
    }
}

fn extname_single(value: Value) -> Value {
    let text = value.to_text();
    let path = PathBuf::from(&text);
    let ext = path
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    Value::String(ext)
}

// ============================================================================
// stem - get filename without extension
// ============================================================================

pub struct StemCommand;

impl NexusCommand for StemCommand {
    fn name(&self) -> &'static str {
        "stem"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(stem_value(stdin_value));
        }

        let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();

        if paths.is_empty() {
            return Ok(Value::Unit);
        }

        let results: Vec<Value> = paths
            .iter()
            .map(|p| {
                let path = PathBuf::from(p);
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                Value::String(stem)
            })
            .collect();

        if results.len() == 1 {
            Ok(results.into_iter().next().unwrap())
        } else {
            Ok(Value::List(results))
        }
    }
}

fn stem_value(value: Value) -> Value {
    match value {
        Value::List(items) => Value::List(items.into_iter().map(stem_single).collect()),
        Value::FileEntry(entry) => {
            let stem = PathBuf::from(&entry.name)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            Value::String(stem)
        }
        other => stem_single(other),
    }
}

fn stem_single(value: Value) -> Value {
    let text = value.to_text();
    let path = PathBuf::from(&text);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or(text);
    Value::String(stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basename() {
        let result = basename_single(Value::String("/foo/bar/baz.txt".to_string()), None);
        assert_eq!(result, Value::String("baz.txt".to_string()));
    }

    #[test]
    fn test_basename_with_suffix() {
        let result = basename_single(Value::String("/foo/bar/baz.txt".to_string()), Some(".txt"));
        assert_eq!(result, Value::String("baz".to_string()));
    }

    #[test]
    fn test_dirname() {
        let result = dirname_single(Value::String("/foo/bar/baz.txt".to_string()));
        assert_eq!(result, Value::String("/foo/bar".to_string()));
    }

    #[test]
    fn test_extname() {
        let result = extname_single(Value::String("foo.txt".to_string()));
        assert_eq!(result, Value::String(".txt".to_string()));
    }

    #[test]
    fn test_stem() {
        let result = stem_single(Value::String("foo.txt".to_string()));
        assert_eq!(result, Value::String("foo".to_string()));
    }
}
