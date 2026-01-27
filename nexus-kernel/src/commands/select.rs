//! Selection commands - first, last, nth, skip, take, flatten, compact, reverse.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

// ============================================================================
// first - get first N items (default 1)
// ============================================================================

pub struct FirstCommand;

impl NexusCommand for FirstCommand {
    fn name(&self) -> &'static str {
        "first"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let count: usize = args
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(first_value(stdin_value, count));
        }

        Ok(Value::Unit)
    }
}

fn first_value(value: Value, count: usize) -> Value {
    match value {
        Value::List(items) => {
            let taken: Vec<Value> = items.into_iter().take(count).collect();
            if count == 1 {
                taken.into_iter().next().unwrap_or(Value::Unit)
            } else {
                Value::List(taken)
            }
        }
        Value::Table { columns, rows } => {
            let taken: Vec<Vec<Value>> = rows.into_iter().take(count).collect();
            Value::Table { columns, rows: taken }
        }
        Value::String(s) => {
            let lines: Vec<&str> = s.lines().take(count).collect();
            if count == 1 {
                Value::String(lines.into_iter().next().unwrap_or("").to_string())
            } else {
                Value::String(lines.join("\n"))
            }
        }
        other => other,
    }
}

// ============================================================================
// last - get last N items (default 1)
// ============================================================================

pub struct LastCommand;

impl NexusCommand for LastCommand {
    fn name(&self) -> &'static str {
        "last"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let count: usize = args
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(last_value(stdin_value, count));
        }

        Ok(Value::Unit)
    }
}

fn last_value(value: Value, count: usize) -> Value {
    match value {
        Value::List(items) => {
            let len = items.len();
            let skip = len.saturating_sub(count);
            let taken: Vec<Value> = items.into_iter().skip(skip).collect();
            if count == 1 {
                taken.into_iter().next().unwrap_or(Value::Unit)
            } else {
                Value::List(taken)
            }
        }
        Value::Table { columns, rows } => {
            let len = rows.len();
            let skip = len.saturating_sub(count);
            let taken: Vec<Vec<Value>> = rows.into_iter().skip(skip).collect();
            Value::Table { columns, rows: taken }
        }
        Value::String(s) => {
            let lines: Vec<&str> = s.lines().collect();
            let len = lines.len();
            let skip = len.saturating_sub(count);
            let taken: Vec<&str> = lines.into_iter().skip(skip).collect();
            if count == 1 {
                Value::String(taken.into_iter().next().unwrap_or("").to_string())
            } else {
                Value::String(taken.join("\n"))
            }
        }
        other => other,
    }
}

// ============================================================================
// nth - get item at index (0-based)
// ============================================================================

pub struct NthCommand;

impl NexusCommand for NthCommand {
    fn name(&self) -> &'static str {
        "nth"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let index: usize = args
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(nth_value(stdin_value, index));
        }

        Ok(Value::Unit)
    }
}

fn nth_value(value: Value, index: usize) -> Value {
    match value {
        Value::List(items) => items.into_iter().nth(index).unwrap_or(Value::Unit),
        Value::Table { columns, rows } => {
            rows.into_iter()
                .nth(index)
                .map(|row| Value::Record(columns.into_iter().map(|c| c.name).zip(row).collect()))
                .unwrap_or(Value::Unit)
        }
        Value::String(s) => s
            .lines()
            .nth(index)
            .map(|l| Value::String(l.to_string()))
            .unwrap_or(Value::Unit),
        other => {
            if index == 0 {
                other
            } else {
                Value::Unit
            }
        }
    }
}

// ============================================================================
// skip - skip first N items
// ============================================================================

pub struct SkipCommand;

impl NexusCommand for SkipCommand {
    fn name(&self) -> &'static str {
        "skip"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let count: usize = args
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(skip_value(stdin_value, count));
        }

        Ok(Value::Unit)
    }
}

fn skip_value(value: Value, count: usize) -> Value {
    match value {
        Value::List(items) => Value::List(items.into_iter().skip(count).collect()),
        Value::Table { columns, rows } => {
            Value::Table {
                columns,
                rows: rows.into_iter().skip(count).collect(),
            }
        }
        Value::String(s) => {
            let lines: Vec<&str> = s.lines().skip(count).collect();
            Value::String(lines.join("\n"))
        }
        other => other,
    }
}

// ============================================================================
// take - take first N items (alias for first with multiple output)
// ============================================================================

pub struct TakeCommand;

impl NexusCommand for TakeCommand {
    fn name(&self) -> &'static str {
        "take"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let count: usize = args
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(10);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(take_value(stdin_value, count));
        }

        Ok(Value::Unit)
    }
}

fn take_value(value: Value, count: usize) -> Value {
    match value {
        Value::List(items) => Value::List(items.into_iter().take(count).collect()),
        Value::Table { columns, rows } => {
            Value::Table {
                columns,
                rows: rows.into_iter().take(count).collect(),
            }
        }
        Value::String(s) => {
            let lines: Vec<&str> = s.lines().take(count).collect();
            Value::String(lines.join("\n"))
        }
        other => other,
    }
}

// ============================================================================
// flatten - flatten nested lists
// ============================================================================

pub struct FlattenCommand;

impl NexusCommand for FlattenCommand {
    fn name(&self) -> &'static str {
        "flatten"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let depth: usize = args
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(flatten_value(stdin_value, depth));
        }

        Ok(Value::Unit)
    }
}

fn flatten_value(value: Value, depth: usize) -> Value {
    if depth == 0 {
        return value;
    }

    match value {
        Value::List(items) => {
            let mut result = Vec::new();
            for item in items {
                match item {
                    Value::List(inner) => {
                        for inner_item in inner {
                            result.push(flatten_value(inner_item, depth - 1));
                        }
                    }
                    other => result.push(other),
                }
            }
            Value::List(result)
        }
        other => other,
    }
}

// ============================================================================
// compact - remove null/empty values
// ============================================================================

pub struct CompactCommand;

impl NexusCommand for CompactCommand {
    fn name(&self) -> &'static str {
        "compact"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(compact_value(stdin_value));
        }

        Ok(Value::Unit)
    }
}

fn compact_value(value: Value) -> Value {
    match value {
        Value::List(items) => {
            let filtered: Vec<Value> = items
                .into_iter()
                .filter(|item| !is_empty(item))
                .collect();
            Value::List(filtered)
        }
        other => other,
    }
}

fn is_empty(value: &Value) -> bool {
    match value {
        Value::Unit => true,
        Value::String(s) => s.is_empty(),
        Value::List(items) => items.is_empty(),
        _ => false,
    }
}

// ============================================================================
// reverse - reverse list order
// ============================================================================

pub struct ReverseCommand;

impl NexusCommand for ReverseCommand {
    fn name(&self) -> &'static str {
        "reverse"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(reverse_value(stdin_value));
        }

        Ok(Value::Unit)
    }
}

fn reverse_value(value: Value) -> Value {
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
            let lines: Vec<&str> = s.lines().rev().collect();
            Value::String(lines.join("\n"))
        }
        other => other,
    }
}

// ============================================================================
// enumerate - add index to each item
// ============================================================================

pub struct EnumerateCommand;

impl NexusCommand for EnumerateCommand {
    fn name(&self) -> &'static str {
        "enumerate"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(enumerate_value(stdin_value));
        }

        Ok(Value::Unit)
    }
}

fn enumerate_value(value: Value) -> Value {
    match value {
        Value::List(items) => {
            let enumerated: Vec<Value> = items
                .into_iter()
                .enumerate()
                .map(|(i, item)| {
                    Value::Record(vec![
                        ("index".to_string(), Value::Int(i as i64)),
                        ("value".to_string(), item),
                    ])
                })
                .collect();
            Value::List(enumerated)
        }
        Value::Table { columns, rows } => {
            let mut new_columns = vec![nexus_api::TableColumn::new("#")];
            new_columns.extend(columns);

            let new_rows: Vec<Vec<Value>> = rows
                .into_iter()
                .enumerate()
                .map(|(i, row)| {
                    let mut new_row = vec![Value::Int(i as i64)];
                    new_row.extend(row);
                    new_row
                })
                .collect();

            Value::Table {
                columns: new_columns,
                rows: new_rows,
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first() {
        let list = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let result = first_value(list, 1);
        assert_eq!(result, Value::Int(1));
    }

    #[test]
    fn test_last() {
        let list = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let result = last_value(list, 1);
        assert_eq!(result, Value::Int(3));
    }

    #[test]
    fn test_nth() {
        let list = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let result = nth_value(list, 1);
        assert_eq!(result, Value::Int(2));
    }

    #[test]
    fn test_skip() {
        let list = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let result = skip_value(list, 1);
        if let Value::List(items) = result {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], Value::Int(2));
        }
    }

    #[test]
    fn test_flatten() {
        let nested = Value::List(vec![
            Value::List(vec![Value::Int(1), Value::Int(2)]),
            Value::List(vec![Value::Int(3), Value::Int(4)]),
        ]);
        let result = flatten_value(nested, 1);
        if let Value::List(items) = result {
            assert_eq!(items.len(), 4);
        }
    }

    #[test]
    fn test_reverse() {
        let list = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let result = reverse_value(list);
        if let Value::List(items) = result {
            assert_eq!(items[0], Value::Int(3));
            assert_eq!(items[2], Value::Int(1));
        }
    }
}
