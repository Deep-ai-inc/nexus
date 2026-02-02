//! The `less` command - page through text in an interactive viewer.

use super::{CommandContext, NexusCommand};
use nexus_api::{InteractiveRequest, Value, ViewerKind};
use std::fs;
use std::path::PathBuf;

pub struct LessCommand;

impl NexusCommand for LessCommand {
    fn name(&self) -> &'static str {
        "less"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // Determine content: piped stdin or file argument
        let content = if let Some(ref stdin) = ctx.stdin {
            stdin.clone()
        } else {
            // Read from file argument
            let file = args.iter().find(|a| !a.starts_with('-'));
            if let Some(file) = file {
                let path = if PathBuf::from(file).is_absolute() {
                    PathBuf::from(file)
                } else {
                    ctx.state.cwd.join(file)
                };
                let text = fs::read_to_string(&path)?;
                Value::String(text)
            } else {
                return Err(anyhow::anyhow!("less: missing filename or piped input"));
            }
        };

        Ok(Value::interactive(InteractiveRequest {
            viewer: ViewerKind::Pager,
            content,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_less_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        let mut f = std::fs::File::create(&file_path).unwrap();
        f.write_all(b"line1\nline2\nline3\n").unwrap();

        let mut test_ctx = TestContext::new(dir.path().to_path_buf());
        let cmd = LessCommand;
        let result = cmd
            .execute(&["test.txt".to_string()], &mut test_ctx.ctx())
            .unwrap();

        let Some(nexus_api::DomainValue::Interactive(req)) = result.as_domain() else {
            panic!("Expected Interactive value");
        };
        assert!(matches!(req.viewer, ViewerKind::Pager));
        match &req.content {
            Value::String(s) => assert!(s.contains("line1")),
            _ => panic!("Expected String content"),
        }
    }

    #[test]
    fn test_less_stdin() {
        let mut test_ctx = TestContext::new_default();
        let cmd = LessCommand;
        let stdin = Value::String("piped content".to_string());
        let result = cmd.execute(&[], &mut test_ctx.ctx_with_stdin(stdin)).unwrap();

        let Some(nexus_api::DomainValue::Interactive(req)) = result.as_domain() else {
            panic!("Expected Interactive value");
        };
        assert!(matches!(req.viewer, ViewerKind::Pager));
        match &req.content {
            Value::String(s) => assert_eq!(s, "piped content"),
            _ => panic!("Expected String content"),
        }
    }

    #[test]
    fn test_less_no_input() {
        let mut test_ctx = TestContext::new_default();
        let cmd = LessCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());
    }
}
