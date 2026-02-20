//! `open` — open files/URLs in the default application.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::path::PathBuf;
use std::process::Command;

pub struct OpenCommand;

impl NexusCommand for OpenCommand {
    fn name(&self) -> &'static str {
        "open"
    }

    fn description(&self) -> &'static str {
        "Open files or URLs in the default application"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut targets = Vec::new();
        let mut app: Option<&str> = None;
        let mut iter = args.iter();

        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "-a" => {
                    app = iter.next().map(|s| s.as_str());
                }
                s if !s.starts_with('-') => targets.push(s.to_string()),
                _ => {}
            }
        }

        if targets.is_empty() {
            anyhow::bail!("open: missing file operand");
        }

        for target in &targets {
            // Resolve relative paths (but leave URLs as-is)
            let is_url = target.contains("://");
            let resolved = if is_url {
                target.clone()
            } else {
                let p = PathBuf::from(target);
                if p.is_absolute() {
                    target.clone()
                } else {
                    ctx.state.cwd.join(target).to_string_lossy().to_string()
                }
            };

            let mut cmd = open_command();
            if let Some(a) = app {
                cmd.args(["-a", a]);
            }
            cmd.arg(&resolved);

            cmd.spawn()
                .map_err(|e| anyhow::anyhow!("open: failed to launch: {}", e))?;
        }

        Ok(Value::Unit)
    }
}

#[cfg(target_os = "macos")]
fn open_command() -> Command {
    Command::new("open")
}

#[cfg(target_os = "linux")]
fn open_command() -> Command {
    Command::new("xdg-open")
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn open_command() -> Command {
    Command::new("open")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_open_missing_operand() {
        let mut test_ctx = TestContext::new_default();
        let cmd = OpenCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());
    }
}
