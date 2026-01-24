//! The `shuf` command - generate random permutations.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use rand::seq::SliceRandom;
use rand::thread_rng;

pub struct ShufCommand;

struct ShufOptions {
    count: Option<usize>,
    repeat: bool,
}

impl ShufOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = ShufOptions {
            count: None,
            repeat: false,
        };

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];

            if arg == "-n" || arg == "--head-count" {
                if i + 1 < args.len() {
                    opts.count = args[i + 1].parse().ok();
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-n") {
                opts.count = arg[2..].parse().ok();
            } else if arg == "-r" || arg == "--repeat" {
                opts.repeat = true;
            }

            i += 1;
        }

        opts
    }
}

impl NexusCommand for ShufCommand {
    fn name(&self) -> &'static str {
        "shuf"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = ShufOptions::parse(args);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(shuf_value(stdin_value, &opts));
        }

        Ok(Value::Unit)
    }
}

fn shuf_value(value: Value, opts: &ShufOptions) -> Value {
    match value {
        Value::List(mut items) => {
            let mut rng = thread_rng();

            if opts.repeat {
                // With repeat, sample with replacement
                let count = opts.count.unwrap_or(items.len());
                let result: Vec<Value> = (0..count)
                    .filter_map(|_| items.choose(&mut rng).cloned())
                    .collect();
                Value::List(result)
            } else {
                // Shuffle in place
                items.shuffle(&mut rng);

                // Optionally limit
                if let Some(n) = opts.count {
                    items.truncate(n);
                }

                Value::List(items)
            }
        }
        Value::Table { columns, mut rows } => {
            let mut rng = thread_rng();
            rows.shuffle(&mut rng);

            if let Some(n) = opts.count {
                rows.truncate(n);
            }

            Value::Table { columns, rows }
        }
        Value::String(s) => {
            let mut lines: Vec<&str> = s.lines().collect();
            let mut rng = thread_rng();
            lines.shuffle(&mut rng);

            if let Some(n) = opts.count {
                lines.truncate(n);
            }

            Value::String(lines.join("\n"))
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shuf_changes_order() {
        // Note: This test may occasionally fail due to randomness
        // but with enough items it's extremely unlikely
        let items: Vec<Value> = (0..100).map(|i| Value::Int(i)).collect();
        let original_order: Vec<i64> = items
            .iter()
            .map(|v| if let Value::Int(n) = v { *n } else { 0 })
            .collect();

        let opts = ShufOptions {
            count: None,
            repeat: false,
        };
        let result = shuf_value(Value::List(items), &opts);

        if let Value::List(shuffled) = result {
            let new_order: Vec<i64> = shuffled
                .iter()
                .map(|v| if let Value::Int(n) = v { *n } else { 0 })
                .collect();

            // Check that all elements are present
            let mut sorted = new_order.clone();
            sorted.sort();
            assert_eq!(sorted, original_order);

            // Check that order changed (very unlikely to stay the same with 100 items)
            assert_ne!(new_order, original_order);
        }
    }

    #[test]
    fn test_shuf_with_count() {
        let items: Vec<Value> = (0..10).map(|i| Value::Int(i)).collect();
        let opts = ShufOptions {
            count: Some(3),
            repeat: false,
        };
        let result = shuf_value(Value::List(items), &opts);

        if let Value::List(shuffled) = result {
            assert_eq!(shuffled.len(), 3);
        }
    }
}
