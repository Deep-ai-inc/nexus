//! _ (prev) command - outputs the last stored value.
//!
//! This enables pipeline continuation:
//!   ls -la
//!   _ | grep ".rs"     # pipes from previous output
//!
//! Also supports indexed access:
//!   _1   # most recent output (same as _)
//!   _2   # second most recent
//!   _3   # third most recent

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

/// The `_` command - outputs the last stored value.
pub struct PrevCommand;

impl NexusCommand for PrevCommand {
    fn name(&self) -> &'static str {
        "_"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        ctx.state
            .get_last_output()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no previous output"))
    }
}

/// The `_1` command - outputs the most recent stored value.
pub struct Prev1Command;

impl NexusCommand for Prev1Command {
    fn name(&self) -> &'static str {
        "_1"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        ctx.state
            .get_output_by_index(1)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no output at index 1"))
    }
}

/// The `_2` command - outputs the second most recent stored value.
pub struct Prev2Command;

impl NexusCommand for Prev2Command {
    fn name(&self) -> &'static str {
        "_2"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        ctx.state
            .get_output_by_index(2)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no output at index 2"))
    }
}

/// The `_3` command - outputs the third most recent stored value.
pub struct Prev3Command;

impl NexusCommand for Prev3Command {
    fn name(&self) -> &'static str {
        "_3"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        ctx.state
            .get_output_by_index(3)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no output at index 3"))
    }
}

/// The `outputs` command - list recent stored outputs.
pub struct OutputsCommand;

impl NexusCommand for OutputsCommand {
    fn name(&self) -> &'static str {
        "outputs"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let limit: usize = args
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);

        let rows: Vec<Vec<Value>> = ctx
            .state
            .block_outputs
            .iter()
            .take(limit)
            .enumerate()
            .map(|(i, output)| {
                vec![
                    Value::Int((i + 1) as i64),
                    Value::String(output.command.clone()),
                    Value::String(format!("{:?}", output.value).chars().take(50).collect()),
                ]
            })
            .collect();

        Ok(Value::Table {
            columns: vec![
                "index".to_string(),
                "command".to_string(),
                "preview".to_string(),
            ],
            rows,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_prev_no_output() {
        let mut test_ctx = TestContext::new_default();
        let cmd = PrevCommand;

        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());
    }

    #[test]
    fn test_prev_with_output() {
        let mut test_ctx = TestContext::new_default();

        // Store an output
        test_ctx
            .ctx()
            .state
            .store_output(
                nexus_api::BlockId(1),
                "test".to_string(),
                Value::String("hello".to_string()),
            );

        let cmd = PrevCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        assert_eq!(result, Value::String("hello".to_string()));
    }

    #[test]
    fn test_prev_indexed() {
        let mut test_ctx = TestContext::new_default();

        // Store multiple outputs
        {
            let ctx = test_ctx.ctx();
            ctx.state.store_output(
                nexus_api::BlockId(1),
                "first".to_string(),
                Value::Int(1),
            );
        }
        {
            let ctx = test_ctx.ctx();
            ctx.state.store_output(
                nexus_api::BlockId(2),
                "second".to_string(),
                Value::Int(2),
            );
        }
        {
            let ctx = test_ctx.ctx();
            ctx.state.store_output(
                nexus_api::BlockId(3),
                "third".to_string(),
                Value::Int(3),
            );
        }

        // _1 should be most recent (3)
        let cmd1 = Prev1Command;
        let result1 = cmd1.execute(&[], &mut test_ctx.ctx()).unwrap();
        assert_eq!(result1, Value::Int(3));

        // _2 should be second most recent (2)
        let cmd2 = Prev2Command;
        let result2 = cmd2.execute(&[], &mut test_ctx.ctx()).unwrap();
        assert_eq!(result2, Value::Int(2));

        // _3 should be third most recent (1)
        let cmd3 = Prev3Command;
        let result3 = cmd3.execute(&[], &mut test_ctx.ctx()).unwrap();
        assert_eq!(result3, Value::Int(1));
    }

    #[test]
    fn test_outputs_command() {
        let mut test_ctx = TestContext::new_default();

        // Store some outputs
        {
            let ctx = test_ctx.ctx();
            ctx.state.store_output(
                nexus_api::BlockId(1),
                "ls".to_string(),
                Value::String("files".to_string()),
            );
        }

        let cmd = OutputsCommand;
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Table { columns, rows } => {
                assert_eq!(columns, vec!["index", "command", "preview"]);
                assert!(!rows.is_empty());
            }
            _ => panic!("Expected Table"),
        }
    }
}
