//! times - Display process times.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

pub struct TimesCommand;

impl NexusCommand for TimesCommand {
    fn name(&self) -> &'static str {
        "times"
    }

    fn execute(&self, _args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        #[cfg(unix)]
        {
            use nix::libc;

            let mut tms: libc::tms = unsafe { std::mem::zeroed() };
            let result = unsafe { libc::times(&mut tms) };

            // On error, times() returns (clock_t)-1
            if result as i64 == -1 {
                anyhow::bail!("times: failed to get process times");
            }

            // Get clock ticks per second
            let ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;

            let user_time = tms.tms_utime as f64 / ticks_per_sec;
            let sys_time = tms.tms_stime as f64 / ticks_per_sec;
            let child_user_time = tms.tms_cutime as f64 / ticks_per_sec;
            let child_sys_time = tms.tms_cstime as f64 / ticks_per_sec;

            Ok(Value::Record(vec![
                ("user".to_string(), Value::Float(user_time)),
                ("system".to_string(), Value::Float(sys_time)),
                ("child_user".to_string(), Value::Float(child_user_time)),
                ("child_system".to_string(), Value::Float(child_sys_time)),
            ]))
        }

        #[cfg(not(unix))]
        {
            Ok(Value::Record(vec![
                ("user".to_string(), Value::Float(0.0)),
                ("system".to_string(), Value::Float(0.0)),
                ("child_user".to_string(), Value::Float(0.0)),
                ("child_system".to_string(), Value::Float(0.0)),
            ]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_times_command_name() {
        let cmd = TimesCommand;
        assert_eq!(cmd.name(), "times");
    }

    #[test]
    fn test_times_returns_record() {
        let cmd = TimesCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Record(fields) => {
                // Should have all four time fields
                let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
                assert!(keys.contains(&"user"));
                assert!(keys.contains(&"system"));
                assert!(keys.contains(&"child_user"));
                assert!(keys.contains(&"child_system"));
            }
            _ => panic!("Expected Record"),
        }
    }

    #[test]
    fn test_times_values_are_floats() {
        let cmd = TimesCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Record(fields) => {
                for (name, value) in fields {
                    match value {
                        Value::Float(f) => {
                            // Times should be non-negative
                            assert!(f >= 0.0, "{} should be non-negative, got {}", name, f);
                        }
                        _ => panic!("Expected Float for {}", name),
                    }
                }
            }
            _ => panic!("Expected Record"),
        }
    }

    #[test]
    fn test_times_ignores_args() {
        let cmd = TimesCommand;
        let mut test_ctx = TestContext::new_default();

        // times doesn't take arguments, but shouldn't fail if given any
        let result = cmd
            .execute(
                &["ignored".to_string(), "args".to_string()],
                &mut test_ctx.ctx(),
            )
            .unwrap();

        match result {
            Value::Record(_) => {} // Success
            _ => panic!("Expected Record"),
        }
    }
}
