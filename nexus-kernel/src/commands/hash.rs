//! Hash commands — `hash`, `md5sum`, `sha256sum`.

use super::{CommandContext, NexusCommand};
use md5::Md5;
use nexus_api::Value;
use sha2::{Digest, Sha256, Sha512};
use std::path::PathBuf;

// ============================================================================
// hash — generic hash command
// ============================================================================

pub struct HashCommand;

impl NexusCommand for HashCommand {
    fn name(&self) -> &'static str {
        "hash"
    }

    fn description(&self) -> &'static str {
        "Compute a cryptographic hash (md5, sha256, sha512)"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut algo = "sha256";
        let mut files = Vec::new();
        let mut iter = args.iter();

        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "-a" | "--algorithm" => {
                    if let Some(a) = iter.next() {
                        algo = match a.as_str() {
                            "md5" => "md5",
                            "sha256" => "sha256",
                            "sha512" => "sha512",
                            _ => anyhow::bail!("hash: unsupported algorithm '{}' (use md5, sha256, sha512)", a),
                        };
                    }
                }
                s if !s.starts_with('-') => files.push(s.to_string()),
                _ => {}
            }
        }

        if files.is_empty() {
            // Hash stdin
            let data = stdin_bytes(ctx)?;
            let hex = compute_hash(algo, &data);
            return Ok(Value::Record(vec![
                ("algorithm".to_string(), Value::String(algo.to_string())),
                ("hash".to_string(), Value::String(hex)),
            ]));
        }

        let results: Vec<Value> = files
            .iter()
            .map(|f| hash_file(f, algo, &ctx.state.cwd))
            .collect::<anyhow::Result<Vec<_>>>()?;

        if results.len() == 1 {
            Ok(results.into_iter().next().unwrap())
        } else {
            Ok(Value::List(results))
        }
    }
}

// ============================================================================
// md5sum
// ============================================================================

pub struct Md5sumCommand;

impl NexusCommand for Md5sumCommand {
    fn name(&self) -> &'static str {
        "md5sum"
    }

    fn description(&self) -> &'static str {
        "Compute MD5 hash"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let files: Vec<&str> = args.iter().map(|s| s.as_str()).filter(|s| !s.starts_with('-')).collect();

        if files.is_empty() {
            let data = stdin_bytes(ctx)?;
            let hex = compute_hash("md5", &data);
            return Ok(Value::String(format!("{}  -", hex)));
        }

        let mut lines = Vec::new();
        for f in files {
            let path = resolve(f, &ctx.state.cwd);
            let data = std::fs::read(&path)
                .map_err(|e| anyhow::anyhow!("md5sum: {}: {}", path.display(), e))?;
            let hex = compute_hash("md5", &data);
            lines.push(Value::String(format!("{}  {}", hex, f)));
        }

        if lines.len() == 1 {
            Ok(lines.into_iter().next().unwrap())
        } else {
            Ok(Value::List(lines))
        }
    }
}

// ============================================================================
// sha256sum
// ============================================================================

pub struct Sha256sumCommand;

impl NexusCommand for Sha256sumCommand {
    fn name(&self) -> &'static str {
        "sha256sum"
    }

    fn description(&self) -> &'static str {
        "Compute SHA-256 hash"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let files: Vec<&str> = args.iter().map(|s| s.as_str()).filter(|s| !s.starts_with('-')).collect();

        if files.is_empty() {
            let data = stdin_bytes(ctx)?;
            let hex = compute_hash("sha256", &data);
            return Ok(Value::String(format!("{}  -", hex)));
        }

        let mut lines = Vec::new();
        for f in files {
            let path = resolve(f, &ctx.state.cwd);
            let data = std::fs::read(&path)
                .map_err(|e| anyhow::anyhow!("sha256sum: {}: {}", path.display(), e))?;
            let hex = compute_hash("sha256", &data);
            lines.push(Value::String(format!("{}  {}", hex, f)));
        }

        if lines.len() == 1 {
            Ok(lines.into_iter().next().unwrap())
        } else {
            Ok(Value::List(lines))
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn compute_hash(algo: &str, data: &[u8]) -> String {
    match algo {
        "md5" => {
            let mut hasher = Md5::new();
            hasher.update(data);
            format!("{:x}", hasher.finalize())
        }
        "sha256" => {
            let mut hasher = Sha256::new();
            hasher.update(data);
            format!("{:x}", hasher.finalize())
        }
        "sha512" => {
            let mut hasher = Sha512::new();
            hasher.update(data);
            format!("{:x}", hasher.finalize())
        }
        _ => unreachable!(),
    }
}

fn stdin_bytes(ctx: &CommandContext) -> anyhow::Result<Vec<u8>> {
    match &ctx.stdin {
        Some(Value::String(s)) => Ok(s.as_bytes().to_vec()),
        Some(Value::Bytes(b)) => Ok(b.clone()),
        Some(other) => Ok(other.to_text().into_bytes()),
        None => anyhow::bail!("no input (pipe data or specify files)"),
    }
}

fn resolve(file: &str, cwd: &PathBuf) -> PathBuf {
    let p = PathBuf::from(file);
    if p.is_absolute() { p } else { cwd.join(file) }
}

fn hash_file(file: &str, algo: &str, cwd: &PathBuf) -> anyhow::Result<Value> {
    let path = resolve(file, cwd);
    let data = std::fs::read(&path)
        .map_err(|e| anyhow::anyhow!("{}: {}", path.display(), e))?;
    let hex = compute_hash(algo, &data);

    Ok(Value::Record(vec![
        ("file".to_string(), Value::String(file.to_string())),
        ("algorithm".to_string(), Value::String(algo.to_string())),
        ("hash".to_string(), Value::String(hex)),
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_hash_stdin_sha256() {
        let mut test_ctx = TestContext::new_default();
        let mut ctx = test_ctx.ctx_with_stdin(Value::String("hello".to_string()));

        let cmd = HashCommand;
        let result = cmd.execute(&[], &mut ctx).unwrap();

        match result {
            Value::Record(fields) => {
                let hash = fields.iter().find(|(k, _)| k == "hash").unwrap();
                // SHA-256 of "hello"
                assert_eq!(
                    hash.1,
                    Value::String(
                        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
                            .to_string()
                    )
                );
            }
            _ => panic!("Expected Record"),
        }
    }

    #[test]
    fn test_hash_stdin_md5() {
        let mut test_ctx = TestContext::new_default();
        let mut ctx = test_ctx.ctx_with_stdin(Value::String("hello".to_string()));

        let cmd = HashCommand;
        let result = cmd
            .execute(&["-a".to_string(), "md5".to_string()], &mut ctx)
            .unwrap();

        match result {
            Value::Record(fields) => {
                let hash = fields.iter().find(|(k, _)| k == "hash").unwrap();
                // MD5 of "hello"
                assert_eq!(
                    hash.1,
                    Value::String("5d41402abc4b2a76b9719d911017c592".to_string())
                );
            }
            _ => panic!("Expected Record"),
        }
    }

    #[test]
    fn test_md5sum_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();

        let mut test_ctx = TestContext::new(dir.path().to_path_buf());
        let cmd = Md5sumCommand;
        let result = cmd
            .execute(&["test.txt".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::String(s) => {
                assert!(s.starts_with("5d41402abc4b2a76b9719d911017c592"));
                assert!(s.contains("test.txt"));
            }
            _ => panic!("Expected String"),
        }
    }

    #[test]
    fn test_sha256sum_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();

        let mut test_ctx = TestContext::new(dir.path().to_path_buf());
        let cmd = Sha256sumCommand;
        let result = cmd
            .execute(&["test.txt".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::String(s) => {
                assert!(s.starts_with("2cf24dba5fb0a30e26e83b2ac5b9e29e"));
                assert!(s.contains("test.txt"));
            }
            _ => panic!("Expected String"),
        }
    }

    #[test]
    fn test_hash_no_input() {
        let mut test_ctx = TestContext::new_default();
        let cmd = HashCommand;
        assert!(cmd.execute(&[], &mut test_ctx.ctx()).is_err());
    }

    #[test]
    fn test_hash_unsupported_algo() {
        let mut test_ctx = TestContext::new_default();
        let mut ctx = test_ctx.ctx_with_stdin(Value::String("data".to_string()));

        let cmd = HashCommand;
        let result = cmd.execute(&["-a".to_string(), "sha3".to_string()], &mut ctx);
        assert!(result.is_err());
    }
}
