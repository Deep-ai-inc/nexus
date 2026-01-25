//! Claude command - opens the native Claude Code UI panel.

use anyhow::Result;
use nexus_api::{ShellEvent, Value};

use super::{CommandContext, NexusCommand};

/// Opens the native Claude Code UI panel.
///
/// Usage:
///   claude                  # Open Claude panel
///   claude "help me fix this bug"  # Open with initial prompt
pub struct ClaudeCommand;

impl NexusCommand for ClaudeCommand {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> Result<Value> {
        // Combine all arguments as the initial prompt (if any)
        let initial_prompt = if args.is_empty() {
            None
        } else {
            Some(args.join(" "))
        };

        // Emit event to open Claude panel
        let _ = ctx.events.send(ShellEvent::OpenClaudePanel {
            initial_prompt,
            cwd: ctx.state.cwd.clone(),
        });

        Ok(Value::Unit)
    }
}
