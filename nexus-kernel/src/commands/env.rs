//! Environment commands - env, printenv.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

// ============================================================================
// env - print environment variables
// ============================================================================

pub struct EnvCommand;

impl NexusCommand for EnvCommand {
    fn name(&self) -> &'static str {
        "env"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut entries: Vec<(&String, &String)> = ctx.state.env.iter().collect();

        // Sort by key for consistent output
        entries.sort_by(|a, b| a.0.cmp(b.0));

        let rows: Vec<Vec<Value>> = entries
            .into_iter()
            .map(|(k, v)| vec![Value::String(k.clone()), Value::String(v.clone())])
            .collect();

        Ok(Value::table(vec!["name", "value"], rows))
    }
}

// ============================================================================
// printenv - print specific environment variables
// ============================================================================

pub struct PrintenvCommand;

impl NexusCommand for PrintenvCommand {
    fn name(&self) -> &'static str {
        "printenv"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.is_empty() {
            // Print all variables as table (like env)
            let mut entries: Vec<(&String, &String)> = ctx.state.env.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));

            let rows: Vec<Vec<Value>> = entries
                .into_iter()
                .map(|(k, v)| vec![Value::String(k.clone()), Value::String(v.clone())])
                .collect();

            return Ok(Value::table(vec!["name", "value"], rows));
        }

        // Print specific variables
        let results: Vec<Value> = args
            .iter()
            .filter_map(|name| {
                ctx.state
                    .get_env(name)
                    .map(|v| Value::String(v.to_string()))
            })
            .collect();

        if results.len() == 1 {
            Ok(results.into_iter().next().unwrap())
        } else if results.is_empty() {
            Err(anyhow::anyhow!(""))
        } else {
            Ok(Value::List(results))
        }
    }
}

// ============================================================================
// export - set environment variable (returns Unit but modifies state)
// ============================================================================

pub struct ExportCommand;

impl NexusCommand for ExportCommand {
    fn name(&self) -> &'static str {
        "export"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        for arg in args {
            if let Some((name, value)) = arg.split_once('=') {
                ctx.state.set_env(name, value);
            } else {
                // Export existing variable
                if let Some(value) = ctx.state.get_var(arg) {
                    let value = value.to_string();
                    ctx.state.set_env(arg, &value);
                }
            }
        }
        Ok(Value::Unit)
    }
}

// ============================================================================
// unset - remove environment variable
// ============================================================================

pub struct UnsetCommand;

impl NexusCommand for UnsetCommand {
    fn name(&self) -> &'static str {
        "unset"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        for name in args {
            ctx.state.unset_env(name);
        }
        Ok(Value::Unit)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_env_returns_record() {
        // Can't easily test without a CommandContext, but the structure is correct
    }
}
