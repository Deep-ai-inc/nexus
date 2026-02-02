//! The `man` command - display manual pages in an interactive pager.

use super::{CommandContext, NexusCommand};
use nexus_api::{InteractiveRequest, Value, ViewerKind};
use std::process::Command;

pub struct ManCommand;

impl NexusCommand for ManCommand {
    fn name(&self) -> &'static str {
        "man"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.is_empty() {
            return Err(anyhow::anyhow!("What manual page do you want?"));
        }

        // Filter flags and get the page name(s)
        let pages: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
        if pages.is_empty() {
            return Err(anyhow::anyhow!("man: missing page argument"));
        }

        let output = Command::new("man")
            .env("MANWIDTH", "100")
            .env("MANPAGER", "cat")
            .args(&pages)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("man: {}", stderr.trim()));
        }

        let text = String::from_utf8_lossy(&output.stdout).to_string();

        Ok(Value::interactive(InteractiveRequest {
            viewer: ViewerKind::ManPage,
            content: Value::String(text),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_man_ls() {
        let mut test_ctx = TestContext::new_default();
        let cmd = ManCommand;
        let result = cmd.execute(&["ls".to_string()], &mut test_ctx.ctx()).unwrap();

        let Some(nexus_api::DomainValue::Interactive(req)) = result.as_domain() else {
            panic!("Expected Interactive value");
        };
        assert!(matches!(req.viewer, ViewerKind::ManPage));
        match &req.content {
            Value::String(text) => {
                assert!(!text.is_empty());
                let lower = text.to_lowercase();
                assert!(lower.contains("list") || lower.contains("ls"));
            }
            _ => panic!("Expected String content"),
        }
    }

    #[test]
    fn test_man_no_args() {
        let mut test_ctx = TestContext::new_default();
        let cmd = ManCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());
    }

    #[test]
    fn test_man_nonexistent() {
        let mut test_ctx = TestContext::new_default();
        let cmd = ManCommand;
        let result = cmd
            .execute(&["zzznonexistentpage999".to_string()], &mut test_ctx.ctx());
        assert!(result.is_err());
    }
}
