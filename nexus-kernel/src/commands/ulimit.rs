//! ulimit - Get and set user resource limits.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;

pub struct UlimitCommand;

impl NexusCommand for UlimitCommand {
    fn name(&self) -> &'static str {
        "ulimit"
    }

    fn execute(&self, args: &[String], _ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let mut show_all = false;
        let mut soft = true; // Default to soft limit
        let mut resource = None;
        let mut new_value = None;

        // Parse options
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            match arg.as_str() {
                "-a" => show_all = true,
                "-S" => soft = true,
                "-H" => soft = false,
                "-c" => resource = Some(Resource::CoreSize),
                "-d" => resource = Some(Resource::DataSize),
                "-f" => resource = Some(Resource::FileSize),
                "-l" => resource = Some(Resource::LockedMemory),
                "-m" => resource = Some(Resource::ResidentSetSize),
                "-n" => resource = Some(Resource::OpenFiles),
                "-s" => resource = Some(Resource::StackSize),
                "-t" => resource = Some(Resource::CpuTime),
                "-u" => resource = Some(Resource::MaxProcesses),
                "-v" => resource = Some(Resource::VirtualMemory),
                arg if !arg.starts_with('-') => {
                    new_value = Some(arg);
                }
                _ => {}
            }
            i += 1;
        }

        #[cfg(unix)]
        {
            use nix::libc;

            if show_all {
                return Ok(get_all_limits(soft));
            }

            let resource = resource.unwrap_or(Resource::FileSize);
            let libc_resource = resource.to_libc();

            if let Some(value_str) = new_value {
                // Set the limit
                let value = if value_str == "unlimited" {
                    libc::RLIM_INFINITY
                } else {
                    value_str.parse::<u64>()
                        .map_err(|_| anyhow::anyhow!("ulimit: invalid limit: {}", value_str))?
                        * resource.multiplier()
                };

                let mut rlim: libc::rlimit = unsafe { std::mem::zeroed() };
                let ret = unsafe { libc::getrlimit(libc_resource, &mut rlim) };
                if ret != 0 {
                    anyhow::bail!("ulimit: failed to get current limit");
                }

                if soft {
                    rlim.rlim_cur = value;
                } else {
                    rlim.rlim_max = value;
                }

                let ret = unsafe { libc::setrlimit(libc_resource, &rlim) };
                if ret != 0 {
                    anyhow::bail!("ulimit: failed to set limit");
                }

                Ok(Value::Unit)
            } else {
                // Get the limit
                let mut rlim: libc::rlimit = unsafe { std::mem::zeroed() };
                let ret = unsafe { libc::getrlimit(libc_resource, &mut rlim) };
                if ret != 0 {
                    anyhow::bail!("ulimit: failed to get limit");
                }

                let limit = if soft { rlim.rlim_cur } else { rlim.rlim_max };

                if limit == libc::RLIM_INFINITY {
                    Ok(Value::String("unlimited".to_string()))
                } else {
                    Ok(Value::Int((limit / resource.multiplier()) as i64))
                }
            }
        }

        #[cfg(not(unix))]
        {
            let _ = (show_all, soft, resource, new_value);
            Ok(Value::String("unlimited".to_string()))
        }
    }
}

#[derive(Clone, Copy)]
enum Resource {
    CoreSize,
    DataSize,
    FileSize,
    LockedMemory,
    ResidentSetSize,
    OpenFiles,
    StackSize,
    CpuTime,
    MaxProcesses,
    VirtualMemory,
}

impl Resource {
    #[cfg(unix)]
    fn to_libc(self) -> nix::libc::c_int {
        use nix::libc;
        match self {
            Resource::CoreSize => libc::RLIMIT_CORE as _,
            Resource::DataSize => libc::RLIMIT_DATA as _,
            Resource::FileSize => libc::RLIMIT_FSIZE as _,
            Resource::LockedMemory => libc::RLIMIT_MEMLOCK as _,
            Resource::ResidentSetSize => libc::RLIMIT_RSS as _,
            Resource::OpenFiles => libc::RLIMIT_NOFILE as _,
            Resource::StackSize => libc::RLIMIT_STACK as _,
            Resource::CpuTime => libc::RLIMIT_CPU as _,
            Resource::MaxProcesses => libc::RLIMIT_NPROC as _,
            Resource::VirtualMemory => libc::RLIMIT_AS as _,
        }
    }

    fn multiplier(self) -> u64 {
        match self {
            Resource::FileSize | Resource::CoreSize | Resource::DataSize
            | Resource::LockedMemory | Resource::ResidentSetSize
            | Resource::StackSize | Resource::VirtualMemory => 1024, // blocks to bytes
            Resource::OpenFiles | Resource::CpuTime | Resource::MaxProcesses => 1,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Resource::CoreSize => "core file size",
            Resource::DataSize => "data seg size",
            Resource::FileSize => "file size",
            Resource::LockedMemory => "max locked memory",
            Resource::ResidentSetSize => "max memory size",
            Resource::OpenFiles => "open files",
            Resource::StackSize => "stack size",
            Resource::CpuTime => "cpu time",
            Resource::MaxProcesses => "max user processes",
            Resource::VirtualMemory => "virtual memory",
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Resource::CoreSize | Resource::DataSize | Resource::FileSize
            | Resource::LockedMemory | Resource::ResidentSetSize
            | Resource::StackSize | Resource::VirtualMemory => "kbytes",
            Resource::CpuTime => "seconds",
            Resource::OpenFiles | Resource::MaxProcesses => "",
        }
    }
}

#[cfg(unix)]
fn get_all_limits(soft: bool) -> Value {
    use nix::libc;

    let resources = [
        Resource::CoreSize,
        Resource::DataSize,
        Resource::FileSize,
        Resource::LockedMemory,
        Resource::OpenFiles,
        Resource::StackSize,
        Resource::CpuTime,
        Resource::MaxProcesses,
        Resource::VirtualMemory,
    ];

    let mut record = Vec::new();

    for resource in resources {
        let mut rlim: libc::rlimit = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::getrlimit(resource.to_libc(), &mut rlim) };

        if ret == 0 {
            let limit = if soft { rlim.rlim_cur } else { rlim.rlim_max };
            let value = if limit == libc::RLIM_INFINITY {
                Value::String("unlimited".to_string())
            } else {
                Value::Int((limit / resource.multiplier()) as i64)
            };

            let name = format!("{} ({})", resource.name(), resource.unit());
            record.push((name, value));
        }
    }

    Value::Record(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;

    #[test]
    fn test_ulimit_command_name() {
        let cmd = UlimitCommand;
        assert_eq!(cmd.name(), "ulimit");
    }

    #[test]
    fn test_resource_name() {
        assert_eq!(Resource::OpenFiles.name(), "open files");
        assert_eq!(Resource::CpuTime.name(), "cpu time");
        assert_eq!(Resource::StackSize.name(), "stack size");
    }

    #[test]
    fn test_resource_unit() {
        assert_eq!(Resource::OpenFiles.unit(), "");
        assert_eq!(Resource::CpuTime.unit(), "seconds");
        assert_eq!(Resource::StackSize.unit(), "kbytes");
    }

    #[test]
    fn test_resource_multiplier() {
        assert_eq!(Resource::OpenFiles.multiplier(), 1);
        assert_eq!(Resource::CpuTime.multiplier(), 1);
        assert_eq!(Resource::StackSize.multiplier(), 1024);
    }

    #[cfg(unix)]
    #[test]
    fn test_ulimit_show_all() {
        let cmd = UlimitCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd
            .execute(&["-a".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::Record(fields) => {
                // Should have multiple resource limits
                assert!(!fields.is_empty());
                // Check that we have expected fields
                let has_open_files = fields.iter().any(|(k, _)| k.contains("open files"));
                assert!(has_open_files, "Should contain open files limit");
            }
            _ => panic!("Expected Record"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_ulimit_default_file_size() {
        let cmd = UlimitCommand;
        let mut test_ctx = TestContext::new_default();

        // Default (no resource flag) shows file size
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Int(_) | Value::String(_) => {} // Either numeric or "unlimited"
            _ => panic!("Expected Int or String"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_ulimit_open_files() {
        let cmd = UlimitCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd
            .execute(&["-n".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::Int(n) => {
                // Open files should be a positive number
                assert!(n > 0);
            }
            Value::String(s) => {
                assert_eq!(s, "unlimited");
            }
            _ => panic!("Expected Int or String"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_ulimit_soft_vs_hard() {
        let cmd = UlimitCommand;
        let mut test_ctx = TestContext::new_default();

        // Get soft limit
        let soft_result = cmd
            .execute(&["-S".to_string(), "-n".to_string()], &mut test_ctx.ctx())
            .unwrap();

        // Get hard limit - need a fresh context since state is borrowed
        let mut test_ctx2 = TestContext::new_default();
        let hard_result = cmd
            .execute(&["-H".to_string(), "-n".to_string()], &mut test_ctx2.ctx())
            .unwrap();

        // Both should be valid (soft <= hard usually)
        match (&soft_result, &hard_result) {
            (Value::Int(soft), Value::Int(hard)) => {
                assert!(*soft <= *hard);
            }
            (Value::String(s), _) | (_, Value::String(s)) if s == "unlimited" => {
                // unlimited is valid
            }
            _ => {} // Mixed types are also valid
        }
    }

    #[test]
    fn test_ulimit_invalid_value() {
        let cmd = UlimitCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(
            &["-n".to_string(), "notanumber".to_string()],
            &mut test_ctx.ctx(),
        );
        assert!(result.is_err());
    }
}
