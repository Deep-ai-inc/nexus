//! Math/aggregation commands - sum, avg, min, max, count.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

// ============================================================================
// sum
// ============================================================================

pub struct SumCommand;

impl NexusCommand for SumCommand {
    fn name(&self) -> &'static str {
        "sum"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(sum_value(stdin_value));
        }
        Ok(Value::Int(0))
    }
}

fn sum_value(value: Value) -> Value {
    match value {
        Value::List(items) => {
            let mut int_sum: i64 = 0;
            let mut float_sum: f64 = 0.0;
            let mut has_float = false;

            for item in items {
                match item {
                    Value::Int(n) => int_sum += n,
                    Value::Float(f) => {
                        has_float = true;
                        float_sum += f;
                    }
                    Value::String(s) => {
                        if let Ok(n) = s.trim().parse::<i64>() {
                            int_sum += n;
                        } else if let Ok(f) = s.trim().parse::<f64>() {
                            has_float = true;
                            float_sum += f;
                        }
                    }
                    Value::FileEntry(entry) => {
                        int_sum += entry.size as i64;
                    }
                    _ => {}
                }
            }

            if has_float {
                Value::Float(float_sum + int_sum as f64)
            } else {
                Value::Int(int_sum)
            }
        }
        Value::Int(n) => Value::Int(n),
        Value::Float(f) => Value::Float(f),
        _ => Value::Int(0),
    }
}

// ============================================================================
// avg (average/mean)
// ============================================================================

pub struct AvgCommand;

impl NexusCommand for AvgCommand {
    fn name(&self) -> &'static str {
        "avg"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(avg_value(stdin_value));
        }
        Ok(Value::Float(0.0))
    }
}

fn avg_value(value: Value) -> Value {
    match value {
        Value::List(items) => {
            let mut sum: f64 = 0.0;
            let mut count: usize = 0;

            for item in items {
                match item {
                    Value::Int(n) => {
                        sum += n as f64;
                        count += 1;
                    }
                    Value::Float(f) => {
                        sum += f;
                        count += 1;
                    }
                    Value::String(s) => {
                        if let Ok(n) = s.trim().parse::<f64>() {
                            sum += n;
                            count += 1;
                        }
                    }
                    Value::FileEntry(entry) => {
                        sum += entry.size as f64;
                        count += 1;
                    }
                    _ => {}
                }
            }

            if count > 0 {
                Value::Float(sum / count as f64)
            } else {
                Value::Float(0.0)
            }
        }
        Value::Int(n) => Value::Float(n as f64),
        Value::Float(f) => Value::Float(f),
        _ => Value::Float(0.0),
    }
}

// ============================================================================
// min
// ============================================================================

pub struct MinCommand;

impl NexusCommand for MinCommand {
    fn name(&self) -> &'static str {
        "min"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(min_value(stdin_value));
        }
        Ok(Value::Unit)
    }
}

fn min_value(value: Value) -> Value {
    match value {
        Value::List(items) => {
            let mut min_val: Option<f64> = None;
            let mut min_item: Option<Value> = None;

            for item in items {
                let num = match &item {
                    Value::Int(n) => Some(*n as f64),
                    Value::Float(f) => Some(*f),
                    Value::String(s) => s.trim().parse::<f64>().ok(),
                    Value::FileEntry(entry) => Some(entry.size as f64),
                    _ => None,
                };

                if let Some(n) = num {
                    if min_val.is_none() || n < min_val.unwrap() {
                        min_val = Some(n);
                        min_item = Some(item);
                    }
                }
            }

            min_item.unwrap_or(Value::Unit)
        }
        other => other,
    }
}

// ============================================================================
// max
// ============================================================================

pub struct MaxCommand;

impl NexusCommand for MaxCommand {
    fn name(&self) -> &'static str {
        "max"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(max_value(stdin_value));
        }
        Ok(Value::Unit)
    }
}

fn max_value(value: Value) -> Value {
    match value {
        Value::List(items) => {
            let mut max_val: Option<f64> = None;
            let mut max_item: Option<Value> = None;

            for item in items {
                let num = match &item {
                    Value::Int(n) => Some(*n as f64),
                    Value::Float(f) => Some(*f),
                    Value::String(s) => s.trim().parse::<f64>().ok(),
                    Value::FileEntry(entry) => Some(entry.size as f64),
                    _ => None,
                };

                if let Some(n) = num {
                    if max_val.is_none() || n > max_val.unwrap() {
                        max_val = Some(n);
                        max_item = Some(item);
                    }
                }
            }

            max_item.unwrap_or(Value::Unit)
        }
        other => other,
    }
}

// ============================================================================
// count (alias for wc -l on lists)
// ============================================================================

pub struct CountCommand;

impl NexusCommand for CountCommand {
    fn name(&self) -> &'static str {
        "count"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(count_value(stdin_value));
        }
        Ok(Value::Int(0))
    }
}

fn count_value(value: Value) -> Value {
    match value {
        Value::List(items) => Value::Int(items.len() as i64),
        Value::Table { rows, .. } => Value::Int(rows.len() as i64),
        Value::String(s) => Value::Int(s.lines().count() as i64),
        Value::Bytes(b) => Value::Int(b.len() as i64),
        _ => Value::Int(1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sum_integers() {
        let list = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let result = sum_value(list);
        assert_eq!(result, Value::Int(6));
    }

    #[test]
    fn test_avg() {
        let list = Value::List(vec![Value::Int(2), Value::Int(4), Value::Int(6)]);
        let result = avg_value(list);
        assert_eq!(result, Value::Float(4.0));
    }

    #[test]
    fn test_min() {
        let list = Value::List(vec![Value::Int(3), Value::Int(1), Value::Int(2)]);
        let result = min_value(list);
        assert_eq!(result, Value::Int(1));
    }

    #[test]
    fn test_max() {
        let list = Value::List(vec![Value::Int(1), Value::Int(3), Value::Int(2)]);
        let result = max_value(list);
        assert_eq!(result, Value::Int(3));
    }

    #[test]
    fn test_count() {
        let list = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let result = count_value(list);
        assert_eq!(result, Value::Int(3));
    }
}
