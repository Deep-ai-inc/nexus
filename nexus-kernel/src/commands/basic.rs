//! Basic commands - echo, pwd, true, false, whoami, hostname, etc.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

// ============================================================================
// echo
// ============================================================================

pub struct EchoCommand;

impl NexusCommand for EchoCommand {
    fn name(&self) -> &'static str {
        "echo"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut no_newline = false;
        let mut enable_escapes = false;
        let mut start_idx = 0;

        // Parse options
        for (i, arg) in args.iter().enumerate() {
            match arg.as_str() {
                "-n" => no_newline = true,
                "-e" => enable_escapes = true,
                "-E" => enable_escapes = false,
                _ => {
                    start_idx = i;
                    break;
                }
            }
            start_idx = i + 1;
        }

        let mut output = args[start_idx..].join(" ");

        if enable_escapes {
            output = process_escapes(&output);
        }

        if !no_newline {
            output.push('\n');
        }

        Ok(Value::String(output.trim_end_matches('\n').to_string()))
    }
}

fn process_escapes(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('r') => result.push('\r'),
                Some('\\') => result.push('\\'),
                Some('0') => result.push('\0'),
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }

    result
}

// ============================================================================
// pwd
// ============================================================================

pub struct PwdCommand;

impl NexusCommand for PwdCommand {
    fn name(&self) -> &'static str {
        "pwd"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        Ok(Value::Path(ctx.state.cwd.clone()))
    }
}

// ============================================================================
// true
// ============================================================================

pub struct TrueCommand;

impl NexusCommand for TrueCommand {
    fn name(&self) -> &'static str {
        "true"
    }

    fn execute(&self, _args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        Ok(Value::Unit)
    }
}

// ============================================================================
// false
// ============================================================================

pub struct FalseCommand;

impl NexusCommand for FalseCommand {
    fn name(&self) -> &'static str {
        "false"
    }

    fn execute(&self, _args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        Err(anyhow::anyhow!(""))
    }
}

// ============================================================================
// whoami
// ============================================================================

pub struct WhoamiCommand;

impl NexusCommand for WhoamiCommand {
    fn name(&self) -> &'static str {
        "whoami"
    }

    fn execute(&self, _args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let user = ctx
            .state
            .get_env("USER")
            .or_else(|| ctx.state.get_env("LOGNAME"))
            .unwrap_or("unknown");
        Ok(Value::String(user.to_string()))
    }
}

// ============================================================================
// hostname
// ============================================================================

pub struct HostnameCommand;

impl NexusCommand for HostnameCommand {
    fn name(&self) -> &'static str {
        "hostname"
    }

    fn execute(&self, _args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let hostname = gethostname::gethostname();
        Ok(Value::String(hostname.to_string_lossy().to_string()))
    }
}

// ============================================================================
// yes
// ============================================================================

pub struct YesCommand;

impl NexusCommand for YesCommand {
    fn name(&self) -> &'static str {
        "yes"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // Just return one instance (can't do infinite in structured data)
        let text = if args.is_empty() {
            "y".to_string()
        } else {
            args.join(" ")
        };
        // Return a reasonable number for piping
        let lines: Vec<Value> = (0..1000).map(|_| Value::String(text.clone())).collect();
        Ok(Value::List(lines))
    }
}

// ============================================================================
// sleep
// ============================================================================

pub struct SleepCommand;

impl NexusCommand for SleepCommand {
    fn name(&self) -> &'static str {
        "sleep"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let seconds: f64 = args
            .first()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);

        std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
        Ok(Value::Unit)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_echo() {
        let result = process_escapes("hello\\nworld");
        assert_eq!(result, "hello\nworld");
    }

    #[test]
    fn test_echo_tabs() {
        let result = process_escapes("a\\tb");
        assert_eq!(result, "a\tb");
    }
}
