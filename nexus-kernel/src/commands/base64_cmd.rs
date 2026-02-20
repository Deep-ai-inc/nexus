//! `base64` — encode and decode base64.

use super::{CommandContext, NexusCommand};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use nexus_api::Value;
use std::path::PathBuf;

pub struct Base64Command;

impl NexusCommand for Base64Command {
    fn name(&self) -> &'static str {
        "base64"
    }

    fn description(&self) -> &'static str {
        "Encode or decode base64 data"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut decode = false;
        let mut file: Option<String> = None;

        for arg in args {
            match arg.as_str() {
                "-d" | "--decode" | "-D" => decode = true,
                s if !s.starts_with('-') => file = Some(s.to_string()),
                _ => {}
            }
        }

        let input_data = if let Some(f) = file {
            let path = if PathBuf::from(&f).is_absolute() {
                PathBuf::from(&f)
            } else {
                ctx.state.cwd.join(&f)
            };
            std::fs::read(&path)
                .map_err(|e| anyhow::anyhow!("base64: {}: {}", path.display(), e))?
        } else if let Some(stdin) = &ctx.stdin {
            match stdin {
                Value::String(s) => s.as_bytes().to_vec(),
                Value::Bytes(b) => b.clone(),
                other => other.to_text().into_bytes(),
            }
        } else {
            anyhow::bail!("base64: no input (pipe data or specify a file)");
        };

        if decode {
            // Strip whitespace before decoding
            let text = String::from_utf8_lossy(&input_data);
            let cleaned: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            let decoded = STANDARD
                .decode(&cleaned)
                .map_err(|e| anyhow::anyhow!("base64: invalid input: {}", e))?;
            Ok(Value::Bytes(decoded))
        } else {
            let encoded = STANDARD.encode(&input_data);
            Ok(Value::String(encoded))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_base64_encode_stdin() {
        let mut test_ctx = TestContext::new_default();
        let mut ctx = test_ctx.ctx_with_stdin(Value::String("hello world".to_string()));

        let cmd = Base64Command;
        let result = cmd.execute(&[], &mut ctx).unwrap();
        assert_eq!(result, Value::String("aGVsbG8gd29ybGQ=".to_string()));
    }

    #[test]
    fn test_base64_decode_stdin() {
        let mut test_ctx = TestContext::new_default();
        let mut ctx =
            test_ctx.ctx_with_stdin(Value::String("aGVsbG8gd29ybGQ=".to_string()));

        let cmd = Base64Command;
        let result = cmd.execute(&["-d".to_string()], &mut ctx).unwrap();
        assert_eq!(result, Value::Bytes(b"hello world".to_vec()));
    }

    #[test]
    fn test_base64_encode_file() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.txt"), "hello").unwrap();

        let mut test_ctx = TestContext::new(dir.path().to_path_buf());
        let cmd = Base64Command;
        let result = cmd
            .execute(&["test.txt".to_string()], &mut test_ctx.ctx())
            .unwrap();
        assert_eq!(result, Value::String("aGVsbG8=".to_string()));
    }

    #[test]
    fn test_base64_roundtrip() {
        let mut test_ctx = TestContext::new_default();
        let original = "Nexus shell 🚀";

        let mut ctx = test_ctx.ctx_with_stdin(Value::String(original.to_string()));
        let cmd = Base64Command;
        let encoded = cmd.execute(&[], &mut ctx).unwrap();

        let encoded_str = match encoded {
            Value::String(s) => s,
            _ => panic!("Expected String"),
        };

        let mut test_ctx2 = TestContext::new_default();
        let mut ctx2 = test_ctx2.ctx_with_stdin(Value::String(encoded_str));
        let decoded = cmd.execute(&["-d".to_string()], &mut ctx2).unwrap();

        assert_eq!(decoded, Value::Bytes(original.as_bytes().to_vec()));
    }

    #[test]
    fn test_base64_no_input() {
        let mut test_ctx = TestContext::new_default();
        let cmd = Base64Command;
        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());
    }
}
