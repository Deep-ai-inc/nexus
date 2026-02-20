//! `chmod` — change file mode/permissions.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::path::PathBuf;

pub struct ChmodCommand;

impl NexusCommand for ChmodCommand {
    fn name(&self) -> &'static str {
        "chmod"
    }

    fn description(&self) -> &'static str {
        "Change file permissions"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut recursive = false;
        let mut positional = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-R" | "-r" | "--recursive" => recursive = true,
                s if !s.starts_with('-') || is_mode_spec(s) => positional.push(arg.clone()),
                _ => {}
            }
        }

        if positional.len() < 2 {
            anyhow::bail!("chmod: missing operand");
        }

        let mode_spec = positional.remove(0);
        let files = positional;

        let mut results = Vec::new();

        for file in &files {
            let path = if PathBuf::from(file).is_absolute() {
                PathBuf::from(file)
            } else {
                ctx.state.cwd.join(file)
            };

            if recursive && path.is_dir() {
                chmod_recursive(&path, &mode_spec, &mut results)?;
            } else {
                let result = apply_chmod(&path, &mode_spec)?;
                results.push(result);
            }
        }

        if results.len() == 1 {
            Ok(results.into_iter().next().unwrap())
        } else {
            Ok(Value::List(results))
        }
    }
}

fn chmod_recursive(path: &PathBuf, mode_spec: &str, results: &mut Vec<Value>) -> anyhow::Result<()> {
    let result = apply_chmod(path, mode_spec)?;
    results.push(result);

    if path.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            chmod_recursive(&entry.path(), mode_spec, results)?;
        }
    }

    Ok(())
}

#[cfg(unix)]
fn apply_chmod(path: &PathBuf, mode_spec: &str) -> anyhow::Result<Value> {
    use std::os::unix::fs::PermissionsExt;

    if !path.exists() {
        anyhow::bail!("chmod: cannot access '{}': No such file or directory",
            path.display());
    }

    let metadata = std::fs::metadata(path)?;
    let old_mode = metadata.permissions().mode() & 0o7777;

    let new_mode = parse_mode(mode_spec, old_mode)?;

    let perms = std::fs::Permissions::from_mode(new_mode);
    std::fs::set_permissions(path, perms)?;

    Ok(Value::Record(vec![
        ("path".to_string(), Value::Path(path.clone())),
        ("old_mode".to_string(), Value::String(format!("{:04o}", old_mode))),
        ("new_mode".to_string(), Value::String(format!("{:04o}", new_mode))),
    ]))
}

#[cfg(not(unix))]
fn apply_chmod(path: &PathBuf, _mode_spec: &str) -> anyhow::Result<Value> {
    anyhow::bail!("chmod: not supported on this platform")
}

/// Check if a string looks like a mode spec (starts with digit, or +/- or u/g/o/a).
fn is_mode_spec(s: &str) -> bool {
    let first = s.as_bytes().first().copied().unwrap_or(0);
    first.is_ascii_digit() || b"+-ugoa".contains(&first)
}

/// Parse a mode specification — octal or symbolic.
fn parse_mode(spec: &str, current: u32) -> anyhow::Result<u32> {
    // Octal: 755, 0644, etc.
    if spec.chars().all(|c| c.is_ascii_digit()) {
        let mode = u32::from_str_radix(spec, 8)
            .map_err(|_| anyhow::anyhow!("chmod: invalid mode: '{}'", spec))?;
        if mode > 0o7777 {
            anyhow::bail!("chmod: invalid mode: '{}'", spec);
        }
        return Ok(mode);
    }

    // Symbolic: u+x, go-rw, a+r, +x, etc.
    let mut result = current;

    for clause in spec.split(',') {
        let clause = clause.trim();
        if clause.is_empty() {
            continue;
        }

        let mut who_mask: u32 = 0;
        let mut chars = clause.chars().peekable();

        // Parse who: u, g, o, a
        while let Some(&c) = chars.peek() {
            match c {
                'u' => { who_mask |= 0o700; chars.next(); }
                'g' => { who_mask |= 0o070; chars.next(); }
                'o' => { who_mask |= 0o007; chars.next(); }
                'a' => { who_mask |= 0o777; chars.next(); }
                _ => break,
            }
        }

        // Default to 'a' if no who specified
        if who_mask == 0 {
            who_mask = 0o777;
        }

        // Parse operator: +, -, =
        let op = chars.next().ok_or_else(|| anyhow::anyhow!("chmod: invalid mode: '{}'", spec))?;
        if !matches!(op, '+' | '-' | '=') {
            anyhow::bail!("chmod: invalid mode: '{}'", spec);
        }

        // Parse permissions: r, w, x
        let mut perm_bits: u32 = 0;
        for c in chars {
            match c {
                'r' => perm_bits |= 0o444,
                'w' => perm_bits |= 0o222,
                'x' => perm_bits |= 0o111,
                _ => anyhow::bail!("chmod: invalid mode: '{}'", spec),
            }
        }

        let masked = perm_bits & who_mask;

        match op {
            '+' => result |= masked,
            '-' => result &= !masked,
            '=' => {
                result &= !who_mask;
                result |= masked;
            }
            _ => unreachable!(),
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_octal() {
        assert_eq!(parse_mode("755", 0).unwrap(), 0o755);
        assert_eq!(parse_mode("644", 0).unwrap(), 0o644);
        assert_eq!(parse_mode("0", 0o777).unwrap(), 0);
    }

    #[test]
    fn test_parse_symbolic_plus() {
        assert_eq!(parse_mode("+x", 0o644).unwrap(), 0o755);
        assert_eq!(parse_mode("u+x", 0o644).unwrap(), 0o744);
        assert_eq!(parse_mode("go+r", 0o700).unwrap(), 0o744);
    }

    #[test]
    fn test_parse_symbolic_minus() {
        assert_eq!(parse_mode("-x", 0o755).unwrap(), 0o644);
        assert_eq!(parse_mode("o-rwx", 0o777).unwrap(), 0o770);
    }

    #[test]
    fn test_parse_symbolic_equals() {
        assert_eq!(parse_mode("u=rwx", 0o000).unwrap(), 0o700);
        assert_eq!(parse_mode("a=r", 0o777).unwrap(), 0o444);
    }

    #[test]
    fn test_parse_symbolic_comma() {
        assert_eq!(parse_mode("u+x,g+r", 0o600).unwrap(), 0o740);
    }

    #[test]
    fn test_is_mode_spec() {
        assert!(is_mode_spec("755"));
        assert!(is_mode_spec("+x"));
        assert!(is_mode_spec("u+rw"));
        assert!(!is_mode_spec("file.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn test_chmod_file() {
        use crate::commands::test_utils::test_helpers::TestContext;
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();

        let mut test_ctx = TestContext::new(dir.path().to_path_buf());
        let cmd = ChmodCommand;
        let result = cmd
            .execute(
                &["755".to_string(), "test.txt".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match &result {
            Value::Record(fields) => {
                let new_mode = fields.iter().find(|(k, _)| k == "new_mode").unwrap();
                assert_eq!(new_mode.1, Value::String("0755".to_string()));
            }
            _ => panic!("Expected Record"),
        }

        let meta = std::fs::metadata(dir.path().join("test.txt")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o7777, 0o755);
    }
}
