//! The `uniq` command - report or omit repeated lines.

use super::{CommandContext, NexusCommand};
use nexus_api::{TableColumn, Value};

pub struct UniqCommand;

struct UniqOptions {
    count: bool,
    repeated: bool,
    unique_only: bool,
    ignore_case: bool,
}

impl UniqOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = UniqOptions {
            count: false,
            repeated: false,
            unique_only: false,
            ignore_case: false,
        };

        for arg in args {
            if arg.starts_with('-') && !arg.starts_with("--") {
                for c in arg[1..].chars() {
                    match c {
                        'c' => opts.count = true,
                        'd' => opts.repeated = true,
                        'u' => opts.unique_only = true,
                        'i' => opts.ignore_case = true,
                        _ => {}
                    }
                }
            } else {
                match arg.as_str() {
                    "--count" => opts.count = true,
                    "--repeated" => opts.repeated = true,
                    "--unique" => opts.unique_only = true,
                    "--ignore-case" => opts.ignore_case = true,
                    _ => {}
                }
            }
        }

        opts
    }
}

impl NexusCommand for UniqCommand {
    fn name(&self) -> &'static str {
        "uniq"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = UniqOptions::parse(args);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(uniq_value(stdin_value, &opts));
        }

        Ok(Value::Unit)
    }
}

fn uniq_value(value: Value, opts: &UniqOptions) -> Value {
    match value {
        Value::List(items) => {
            if opts.count {
                // Count occurrences - preserve typed values with count metadata
                let mut counts: Vec<(String, usize, Value)> = Vec::new();
                let mut prev_key: Option<String> = None;

                for item in items {
                    let text = item.to_text();
                    let key = if opts.ignore_case {
                        text.to_lowercase()
                    } else {
                        text.clone()
                    };

                    if Some(&key) == prev_key.as_ref() {
                        if let Some(last) = counts.last_mut() {
                            last.1 += 1;
                        }
                    } else {
                        counts.push((key.clone(), 1, item));
                        prev_key = Some(key);
                    }
                }

                // Filter and return as a proper Table
                let rows: Vec<Vec<Value>> = counts
                    .into_iter()
                    .filter(|(_, count, _)| {
                        if opts.repeated {
                            *count > 1
                        } else if opts.unique_only {
                            *count == 1
                        } else {
                            true
                        }
                    })
                    .map(|(_, count, item)| {
                        vec![Value::Int(count as i64), item]
                    })
                    .collect();

                Value::Table {
                    columns: vec![
                        TableColumn::new("count"),
                        TableColumn::new("value"),
                    ],
                    rows,
                }
            } else {
                // Just dedupe adjacent
                let mut result: Vec<Value> = Vec::new();
                let mut prev_key: Option<String> = None;
                let mut prev_count = 0;

                for item in items {
                    let text = item.to_text();
                    let key = if opts.ignore_case {
                        text.to_lowercase()
                    } else {
                        text.clone()
                    };

                    if Some(&key) == prev_key.as_ref() {
                        prev_count += 1;
                    } else {
                        if let Some(_pk) = prev_key.take() {
                            let should_include = if opts.repeated {
                                prev_count > 1
                            } else if opts.unique_only {
                                prev_count == 1
                            } else {
                                true
                            };
                            if !should_include && !result.is_empty() {
                                result.pop();
                            }
                        }
                        result.push(item);
                        prev_key = Some(key);
                        prev_count = 1;
                    }
                }

                // Handle last item
                if opts.repeated && prev_count <= 1 && !result.is_empty() {
                    result.pop();
                }
                if opts.unique_only && prev_count > 1 && !result.is_empty() {
                    result.pop();
                }

                Value::List(result)
            }
        }
        Value::String(s) => {
            let lines: Vec<&str> = s.lines().collect();
            let list = Value::List(lines.into_iter().map(|l| Value::String(l.to_string())).collect());
            let result = uniq_value(list, opts);
            if let Value::List(items) = result {
                Value::String(items.into_iter().map(|v| v.to_text()).collect::<Vec<_>>().join("\n"))
            } else {
                result
            }
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uniq_basic() {
        let list = Value::List(vec![
            Value::String("a".to_string()),
            Value::String("a".to_string()),
            Value::String("b".to_string()),
            Value::String("b".to_string()),
            Value::String("b".to_string()),
            Value::String("a".to_string()),
        ]);
        let opts = UniqOptions {
            count: false,
            repeated: false,
            unique_only: false,
            ignore_case: false,
        };
        let result = uniq_value(list, &opts);
        if let Value::List(items) = result {
            assert_eq!(items.len(), 3); // a, b, a
        }
    }

    #[test]
    fn test_uniq_count() {
        let list = Value::List(vec![
            Value::String("a".to_string()),
            Value::String("a".to_string()),
            Value::String("b".to_string()),
        ]);
        let opts = UniqOptions {
            count: true,
            repeated: false,
            unique_only: false,
            ignore_case: false,
        };
        let result = uniq_value(list, &opts);
        // Now returns Table with count and value columns
        if let Value::Table { columns, rows } = result {
            assert_eq!(columns.len(), 2);
            assert_eq!(columns[0].name, "count");
            assert_eq!(columns[1].name, "value");
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0][0], Value::Int(2));
            assert_eq!(rows[0][1], Value::String("a".to_string()));
            assert_eq!(rows[1][0], Value::Int(1));
            assert_eq!(rows[1][1], Value::String("b".to_string()));
        } else {
            panic!("Expected Table");
        }
    }
}
