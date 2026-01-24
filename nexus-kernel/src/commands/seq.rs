//! The `seq` command - print a sequence of numbers.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

pub struct SeqCommand;

struct SeqOptions {
    first: f64,
    increment: f64,
    last: f64,
    separator: String,
    format: Option<String>,
    equal_width: bool,
}

impl SeqOptions {
    fn parse(args: &[String]) -> Self {
        let mut opts = SeqOptions {
            first: 1.0,
            increment: 1.0,
            last: 1.0,
            separator: "\n".to_string(),
            format: None,
            equal_width: false,
        };

        let mut positional = Vec::new();
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];

            if arg == "-s" || arg == "--separator" {
                if i + 1 < args.len() {
                    opts.separator = args[i + 1].clone();
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-s") {
                opts.separator = arg[2..].to_string();
            } else if arg == "-f" || arg == "--format" {
                if i + 1 < args.len() {
                    opts.format = Some(args[i + 1].clone());
                    i += 2;
                    continue;
                }
            } else if arg.starts_with("-f") {
                opts.format = Some(arg[2..].to_string());
            } else if arg == "-w" || arg == "--equal-width" {
                opts.equal_width = true;
            } else if !arg.starts_with('-') || arg.parse::<f64>().is_ok() {
                positional.push(arg.clone());
            }

            i += 1;
        }

        // Parse positional arguments
        match positional.len() {
            1 => {
                opts.last = positional[0].parse().unwrap_or(1.0);
            }
            2 => {
                opts.first = positional[0].parse().unwrap_or(1.0);
                opts.last = positional[1].parse().unwrap_or(1.0);
            }
            3 => {
                opts.first = positional[0].parse().unwrap_or(1.0);
                opts.increment = positional[1].parse().unwrap_or(1.0);
                opts.last = positional[2].parse().unwrap_or(1.0);
            }
            _ => {}
        }

        opts
    }
}

impl NexusCommand for SeqCommand {
    fn name(&self) -> &'static str {
        "seq"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let opts = SeqOptions::parse(args);

        let mut numbers: Vec<Value> = Vec::new();
        let mut current = opts.first;

        // Determine width for equal-width formatting
        let width = if opts.equal_width {
            let first_str = format!("{}", opts.first as i64);
            let last_str = format!("{}", opts.last as i64);
            first_str.len().max(last_str.len())
        } else {
            0
        };

        // Generate sequence
        if opts.increment > 0.0 {
            while current <= opts.last + f64::EPSILON {
                numbers.push(format_number(current, &opts, width));
                current += opts.increment;
            }
        } else if opts.increment < 0.0 {
            while current >= opts.last - f64::EPSILON {
                numbers.push(format_number(current, &opts, width));
                current += opts.increment;
            }
        }

        // If separator is newline, return as List
        if opts.separator == "\n" {
            Ok(Value::List(numbers))
        } else {
            // Join with separator and return as String
            let text = numbers
                .into_iter()
                .map(|v| v.to_text())
                .collect::<Vec<_>>()
                .join(&opts.separator);
            Ok(Value::String(text))
        }
    }
}

fn format_number(n: f64, opts: &SeqOptions, width: usize) -> Value {
    if let Some(fmt) = &opts.format {
        // Simple printf-style format support
        let formatted = if fmt.contains("%g") || fmt.contains("%f") {
            fmt.replace("%g", &format!("{}", n))
                .replace("%f", &format!("{}", n))
        } else if fmt.contains("%d") || fmt.contains("%i") {
            fmt.replace("%d", &format!("{}", n as i64))
                .replace("%i", &format!("{}", n as i64))
        } else {
            format!("{}", n)
        };
        Value::String(formatted)
    } else if opts.equal_width {
        Value::String(format!("{:0>width$}", n as i64, width = width))
    } else if n.fract() == 0.0 {
        Value::Int(n as i64)
    } else {
        Value::Float(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_seq_simple() {
        let opts = SeqOptions {
            first: 1.0,
            increment: 1.0,
            last: 5.0,
            separator: "\n".to_string(),
            format: None,
            equal_width: false,
        };

        let mut numbers = Vec::new();
        let mut current = opts.first;
        while current <= opts.last {
            numbers.push(current as i64);
            current += opts.increment;
        }

        assert_eq!(numbers, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_seq_with_increment() {
        let opts = SeqOptions::parse(&["1".to_string(), "2".to_string(), "10".to_string()]);
        assert_eq!(opts.first, 1.0);
        assert_eq!(opts.increment, 2.0);
        assert_eq!(opts.last, 10.0);
    }

    #[test]
    fn test_seq_equal_width() {
        let opts = SeqOptions {
            first: 1.0,
            increment: 1.0,
            last: 10.0,
            separator: "\n".to_string(),
            format: None,
            equal_width: true,
        };

        let result = format_number(1.0, &opts, 2);
        assert_eq!(result.to_text(), "01");
    }
}
