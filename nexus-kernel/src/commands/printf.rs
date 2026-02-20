//! `printf` — formatted output.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

pub struct PrintfCommand;

impl NexusCommand for PrintfCommand {
    fn name(&self) -> &'static str {
        "printf"
    }

    fn description(&self) -> &'static str {
        "Format and print data"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.is_empty() {
            anyhow::bail!("printf: usage: printf FORMAT [ARGUMENTS...]");
        }

        let format = &args[0];
        let arguments = &args[1..];
        let output = format_string(format, arguments)?;
        Ok(Value::String(output))
    }
}

fn format_string(format: &str, args: &[String]) -> anyhow::Result<String> {
    let mut result = String::new();
    let mut chars = format.chars().peekable();
    let mut arg_idx = 0;

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // Escape sequences
                match chars.next() {
                    Some('n') => result.push('\n'),
                    Some('t') => result.push('\t'),
                    Some('r') => result.push('\r'),
                    Some('\\') => result.push('\\'),
                    Some('0') => result.push('\0'),
                    Some('a') => result.push('\x07'),
                    Some('b') => result.push('\x08'),
                    Some('f') => result.push('\x0C'),
                    Some('v') => result.push('\x0B'),
                    Some('"') => result.push('"'),
                    Some('\'') => result.push('\''),
                    Some(other) => {
                        result.push('\\');
                        result.push(other);
                    }
                    None => result.push('\\'),
                }
            }
            '%' => {
                match chars.peek() {
                    Some('%') => {
                        chars.next();
                        result.push('%');
                    }
                    Some(_) => {
                        let arg = args.get(arg_idx).map(|s| s.as_str()).unwrap_or("");
                        arg_idx += 1;

                        // Parse optional width/precision
                        let mut spec = String::new();
                        // Flags
                        while let Some(&fc) = chars.peek() {
                            if matches!(fc, '-' | '+' | ' ' | '0' | '#') {
                                spec.push(fc);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        // Width
                        while let Some(&fc) = chars.peek() {
                            if fc.is_ascii_digit() {
                                spec.push(fc);
                                chars.next();
                            } else {
                                break;
                            }
                        }
                        // Precision
                        if chars.peek() == Some(&'.') {
                            spec.push('.');
                            chars.next();
                            while let Some(&fc) = chars.peek() {
                                if fc.is_ascii_digit() {
                                    spec.push(fc);
                                    chars.next();
                                } else {
                                    break;
                                }
                            }
                        }

                        let conversion = chars.next().unwrap_or('s');
                        match conversion {
                            's' => result.push_str(arg),
                            'd' | 'i' => {
                                let n: i64 = arg.parse().unwrap_or(0);
                                if spec.is_empty() {
                                    result.push_str(&n.to_string());
                                } else {
                                    result.push_str(&format_int(n, &spec));
                                }
                            }
                            'f' => {
                                let n: f64 = arg.parse().unwrap_or(0.0);
                                if spec.is_empty() {
                                    result.push_str(&format!("{:.6}", n));
                                } else {
                                    result.push_str(&format_float(n, &spec));
                                }
                            }
                            'x' => {
                                let n: i64 = arg.parse().unwrap_or(0);
                                result.push_str(&format!("{:x}", n));
                            }
                            'X' => {
                                let n: i64 = arg.parse().unwrap_or(0);
                                result.push_str(&format!("{:X}", n));
                            }
                            'o' => {
                                let n: i64 = arg.parse().unwrap_or(0);
                                result.push_str(&format!("{:o}", n));
                            }
                            'c' => {
                                if let Some(ch) = arg.chars().next() {
                                    result.push(ch);
                                }
                            }
                            other => {
                                result.push('%');
                                result.push_str(&spec);
                                result.push(other);
                            }
                        }
                    }
                    None => result.push('%'),
                }
            }
            other => result.push(other),
        }
    }

    Ok(result)
}

fn format_int(n: i64, spec: &str) -> String {
    // Parse width from spec (simplified)
    let width: usize = spec
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(0);
    let zero_pad = spec.starts_with('0');
    let left_align = spec.starts_with('-');

    if zero_pad && width > 0 {
        format!("{:0>width$}", n, width = width)
    } else if left_align && width > 0 {
        format!("{:<width$}", n, width = width)
    } else if width > 0 {
        format!("{:>width$}", n, width = width)
    } else {
        n.to_string()
    }
}

fn format_float(n: f64, spec: &str) -> String {
    // Extract precision from spec (after '.')
    if let Some(dot_pos) = spec.find('.') {
        let precision: usize = spec[dot_pos + 1..].parse().unwrap_or(6);
        format!("{:.prec$}", n, prec = precision)
    } else {
        format!("{:.6}", n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_printf_basic_string() {
        let mut test_ctx = TestContext::new_default();
        let cmd = PrintfCommand;
        let result = cmd
            .execute(
                &["hello %s".to_string(), "world".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();
        assert_eq!(result, Value::String("hello world".to_string()));
    }

    #[test]
    fn test_printf_integer() {
        let mut test_ctx = TestContext::new_default();
        let cmd = PrintfCommand;
        let result = cmd
            .execute(
                &["count: %d".to_string(), "42".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();
        assert_eq!(result, Value::String("count: 42".to_string()));
    }

    #[test]
    fn test_printf_float() {
        let mut test_ctx = TestContext::new_default();
        let cmd = PrintfCommand;
        let result = cmd
            .execute(
                &["pi: %.2f".to_string(), "3.14159".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();
        assert_eq!(result, Value::String("pi: 3.14".to_string()));
    }

    #[test]
    fn test_printf_hex() {
        let mut test_ctx = TestContext::new_default();
        let cmd = PrintfCommand;
        let result = cmd
            .execute(
                &["0x%x".to_string(), "255".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();
        assert_eq!(result, Value::String("0xff".to_string()));
    }

    #[test]
    fn test_printf_escapes() {
        let mut test_ctx = TestContext::new_default();
        let cmd = PrintfCommand;
        let result = cmd
            .execute(&["hello\\nworld".to_string()], &mut test_ctx.ctx())
            .unwrap();
        assert_eq!(result, Value::String("hello\nworld".to_string()));
    }

    #[test]
    fn test_printf_percent_literal() {
        let mut test_ctx = TestContext::new_default();
        let cmd = PrintfCommand;
        let result = cmd
            .execute(&["100%%".to_string()], &mut test_ctx.ctx())
            .unwrap();
        assert_eq!(result, Value::String("100%".to_string()));
    }

    #[test]
    fn test_printf_multiple_args() {
        let mut test_ctx = TestContext::new_default();
        let cmd = PrintfCommand;
        let result = cmd
            .execute(
                &[
                    "%s is %d".to_string(),
                    "age".to_string(),
                    "30".to_string(),
                ],
                &mut test_ctx.ctx(),
            )
            .unwrap();
        assert_eq!(result, Value::String("age is 30".to_string()));
    }

    #[test]
    fn test_printf_no_args() {
        let mut test_ctx = TestContext::new_default();
        let cmd = PrintfCommand;
        assert!(cmd.execute(&[], &mut test_ctx.ctx()).is_err());
    }
}
