//! The `rev` and `tac` commands - reverse lines/characters.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::fs;
use std::path::PathBuf;

// ============================================================================
// rev - reverse characters in each line
// ============================================================================

pub struct RevCommand;

impl NexusCommand for RevCommand {
    fn name(&self) -> &'static str {
        "rev"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(rev_value(stdin_value));
        }

        // Read from files
        let files: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
        if files.is_empty() {
            return Ok(Value::Unit);
        }

        let mut all_lines = Vec::new();
        for path in &files {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                ctx.state.cwd.join(path)
            };

            match fs::read_to_string(&resolved) {
                Ok(content) => {
                    for line in content.lines() {
                        all_lines.push(line.chars().rev().collect::<String>());
                    }
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("{}: {}", path.display(), e));
                }
            }
        }

        Ok(Value::List(all_lines.into_iter().map(Value::String).collect()))
    }
}

fn rev_value(value: Value) -> Value {
    match value {
        Value::List(items) => {
            Value::List(
                items
                    .into_iter()
                    .map(|item| {
                        let text = item.to_text();
                        Value::String(text.chars().rev().collect())
                    })
                    .collect(),
            )
        }
        Value::String(s) => {
            let reversed: String = s
                .lines()
                .map(|line| line.chars().rev().collect::<String>())
                .collect::<Vec<_>>()
                .join("\n");
            Value::String(reversed)
        }
        other => {
            let text = other.to_text();
            Value::String(text.chars().rev().collect())
        }
    }
}

// ============================================================================
// tac - reverse line order (cat backwards)
// ============================================================================

pub struct TacCommand;

impl NexusCommand for TacCommand {
    fn name(&self) -> &'static str {
        "tac"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(tac_value(stdin_value));
        }

        let files: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
        if files.is_empty() {
            return Ok(Value::Unit);
        }

        let mut all_lines = Vec::new();
        for path in &files {
            let resolved = if path.is_absolute() {
                path.clone()
            } else {
                ctx.state.cwd.join(path)
            };

            match fs::read_to_string(&resolved) {
                Ok(content) => {
                    let mut lines: Vec<String> = content.lines().map(String::from).collect();
                    lines.reverse();
                    all_lines.extend(lines);
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("{}: {}", path.display(), e));
                }
            }
        }

        Ok(Value::List(all_lines.into_iter().map(Value::String).collect()))
    }
}

fn tac_value(value: Value) -> Value {
    match value {
        Value::List(mut items) => {
            items.reverse();
            Value::List(items)
        }
        Value::Table { columns, mut rows } => {
            rows.reverse();
            Value::Table { columns, rows }
        }
        Value::String(s) => {
            let mut lines: Vec<&str> = s.lines().collect();
            lines.reverse();
            Value::String(lines.join("\n"))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rev_string() {
        let result = rev_value(Value::String("hello".to_string()));
        assert_eq!(result, Value::String("olleh".to_string()));
    }

    #[test]
    fn test_tac_list() {
        let list = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let result = tac_value(list);
        if let Value::List(items) = result {
            assert_eq!(items[0], Value::Int(3));
            assert_eq!(items[2], Value::Int(1));
        }
    }
}
