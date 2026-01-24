//! The `tail` command - output the last part of files or piped input.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

pub struct TailCommand;

struct TailOptions {
    count: usize,
    files: Vec<PathBuf>,
}

impl TailOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = TailOptions {
            count: 10,
            files: Vec::new(),
        };

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];

            if arg == "-n" {
                if i + 1 < args.len() {
                    if let Ok(n) = args[i + 1].parse::<usize>() {
                        opts.count = n;
                    }
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-n") {
                if let Ok(n) = arg[2..].parse::<usize>() {
                    opts.count = n;
                }
            } else if arg.starts_with('-') && arg.len() > 1 {
                if let Ok(n) = arg[1..].parse::<usize>() {
                    opts.count = n;
                }
            } else if !arg.starts_with('-') {
                opts.files.push(PathBuf::from(arg));
            }

            i += 1;
        }

        opts
    }
}

impl NexusCommand for TailCommand {
    fn name(&self) -> &'static str {
        "tail"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = TailOptions::parse(args);

        if let Some(stdin_value) = ctx.stdin.take() {
            return Ok(tail_value(stdin_value, opts.count));
        }

        if !opts.files.is_empty() {
            let mut all_lines = Vec::new();

            for path in &opts.files {
                let resolved = if path.is_absolute() {
                    path.clone()
                } else {
                    ctx.state.cwd.join(path)
                };

                match tail_file(&resolved, opts.count) {
                    Ok(lines) => all_lines.extend(lines),
                    Err(e) => {
                        return Err(anyhow::anyhow!("{}: {}", path.display(), e));
                    }
                }
            }

            return Ok(Value::List(
                all_lines.into_iter().map(Value::String).collect(),
            ));
        }

        Ok(Value::Unit)
    }
}

fn tail_value(value: Value, count: usize) -> Value {
    match value {
        Value::List(items) => {
            let len = items.len();
            let skip = len.saturating_sub(count);
            Value::List(items.into_iter().skip(skip).collect())
        }
        Value::Table { columns, rows } => {
            let len = rows.len();
            let skip = len.saturating_sub(count);
            Value::Table {
                columns,
                rows: rows.into_iter().skip(skip).collect(),
            }
        }
        Value::String(s) => {
            let lines: Vec<&str> = s.lines().collect();
            let len = lines.len();
            let skip = len.saturating_sub(count);
            Value::String(lines.into_iter().skip(skip).collect::<Vec<_>>().join("\n"))
        }
        Value::Bytes(bytes) => {
            let s = String::from_utf8_lossy(&bytes);
            let lines: Vec<&str> = s.lines().collect();
            let len = lines.len();
            let skip = len.saturating_sub(count);
            Value::String(lines.into_iter().skip(skip).collect::<Vec<_>>().join("\n"))
        }
        other => other,
    }
}

fn tail_file(path: &PathBuf, count: usize) -> anyhow::Result<Vec<String>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let lines: Vec<String> = reader.lines().collect::<Result<Vec<_>, _>>()?;
    let len = lines.len();
    let skip = len.saturating_sub(count);
    Ok(lines.into_iter().skip(skip).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tail_list() {
        let list = Value::List(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
            Value::Int(4),
            Value::Int(5),
        ]);
        let result = tail_value(list, 3);
        if let Value::List(items) = result {
            assert_eq!(items.len(), 3);
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_parse_options() {
        let opts = TailOptions::parse(&["-5".to_string()]);
        assert_eq!(opts.count, 5);

        let opts = TailOptions::parse(&["-n".to_string(), "3".to_string()]);
        assert_eq!(opts.count, 3);
    }
}
