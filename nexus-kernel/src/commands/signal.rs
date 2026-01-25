//! Signal-related commands - kill.

use super::{CommandContext, NexusCommand};
use nexus_api::Value;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

// ============================================================================
// kill - Send signals to processes
// ============================================================================

pub struct KillCommand;

impl NexusCommand for KillCommand {
    fn name(&self) -> &'static str {
        "kill"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.is_empty() {
            anyhow::bail!("usage: kill [-s sigspec | -n signum | -sigspec] pid | jobspec ...");
        }

        // Handle -l (list signals)
        if args.len() == 1 && args[0] == "-l" {
            return list_signals();
        }

        // Handle -l with exit status
        if args.len() == 2 && args[0] == "-l" {
            let exit_status: i32 = args[1].parse().unwrap_or(0);
            let sig = if exit_status > 128 {
                exit_status - 128
            } else {
                exit_status
            };
            return Ok(Value::String(signal_name(sig).to_string()));
        }

        // Parse signal and targets
        let mut signal = Signal::SIGTERM; // Default
        let mut targets_start = 0;

        if !args.is_empty() {
            let first = &args[0];

            if first == "-s" {
                // -s SIGNAL
                if args.len() < 2 {
                    anyhow::bail!("kill: -s requires an argument");
                }
                signal = parse_signal(&args[1])?;
                targets_start = 2;
            } else if first == "-n" {
                // -n signum
                if args.len() < 2 {
                    anyhow::bail!("kill: -n requires an argument");
                }
                let signum: i32 = args[1].parse()?;
                signal = Signal::try_from(signum)?;
                targets_start = 2;
            } else if first.starts_with('-') && first.len() > 1 {
                // -SIGNAL or -signum
                let sig_spec = &first[1..];
                signal = parse_signal(sig_spec)?;
                targets_start = 1;
            }
        }

        let targets = &args[targets_start..];
        if targets.is_empty() {
            anyhow::bail!("kill: no process or job specified");
        }

        let mut errors = Vec::new();

        for target in targets {
            if let Some(job_spec) = target.strip_prefix('%') {
                // Job specification
                let job = find_job(job_spec, ctx)?;
                if let Err(e) = kill(job, signal) {
                    errors.push(format!("kill: {}: {}", target, e));
                }
            } else {
                // PID
                let pid: i32 = target
                    .parse()
                    .map_err(|_| anyhow::anyhow!("kill: {}: arguments must be process or job IDs", target))?;

                if pid == 0 {
                    // Kill all processes in the current process group
                    if let Err(e) = kill(Pid::from_raw(0), signal) {
                        errors.push(format!("kill: 0: {}", e));
                    }
                } else if pid < 0 {
                    // Kill process group
                    if let Err(e) = kill(Pid::from_raw(pid), signal) {
                        errors.push(format!("kill: {}: {}", pid, e));
                    }
                } else {
                    // Kill specific process
                    if let Err(e) = kill(Pid::from_raw(pid), signal) {
                        errors.push(format!("kill: ({}): {}", pid, e));
                    }
                }
            }
        }

        if !errors.is_empty() {
            for err in &errors {
                eprintln!("{}", err);
            }
            anyhow::bail!("kill: some processes could not be signaled");
        }

        Ok(Value::Unit)
    }
}

/// List all signal names.
fn list_signals() -> anyhow::Result<Value> {
    let signals = vec![
        Value::Record(vec![
            ("num".to_string(), Value::Int(1)),
            ("name".to_string(), Value::String("SIGHUP".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(2)),
            ("name".to_string(), Value::String("SIGINT".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(3)),
            ("name".to_string(), Value::String("SIGQUIT".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(4)),
            ("name".to_string(), Value::String("SIGILL".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(5)),
            ("name".to_string(), Value::String("SIGTRAP".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(6)),
            ("name".to_string(), Value::String("SIGABRT".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(9)),
            ("name".to_string(), Value::String("SIGKILL".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(10)),
            ("name".to_string(), Value::String("SIGUSR1".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(11)),
            ("name".to_string(), Value::String("SIGSEGV".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(12)),
            ("name".to_string(), Value::String("SIGUSR2".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(13)),
            ("name".to_string(), Value::String("SIGPIPE".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(14)),
            ("name".to_string(), Value::String("SIGALRM".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(15)),
            ("name".to_string(), Value::String("SIGTERM".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(17)),
            ("name".to_string(), Value::String("SIGCHLD".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(18)),
            ("name".to_string(), Value::String("SIGCONT".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(19)),
            ("name".to_string(), Value::String("SIGSTOP".to_string())),
        ]),
        Value::Record(vec![
            ("num".to_string(), Value::Int(20)),
            ("name".to_string(), Value::String("SIGTSTP".to_string())),
        ]),
    ];

    Ok(Value::List(signals))
}

/// Parse a signal specification (name or number).
fn parse_signal(s: &str) -> anyhow::Result<Signal> {
    // Try as number first
    if let Ok(n) = s.parse::<i32>() {
        return Signal::try_from(n).map_err(|_| anyhow::anyhow!("invalid signal number: {}", n));
    }

    // Try as name (with or without SIG prefix)
    let name = s.to_uppercase();
    let name = name.strip_prefix("SIG").unwrap_or(&name);

    match name {
        "HUP" => Ok(Signal::SIGHUP),
        "INT" => Ok(Signal::SIGINT),
        "QUIT" => Ok(Signal::SIGQUIT),
        "ILL" => Ok(Signal::SIGILL),
        "TRAP" => Ok(Signal::SIGTRAP),
        "ABRT" | "IOT" => Ok(Signal::SIGABRT),
        "BUS" => Ok(Signal::SIGBUS),
        "FPE" => Ok(Signal::SIGFPE),
        "KILL" => Ok(Signal::SIGKILL),
        "USR1" => Ok(Signal::SIGUSR1),
        "SEGV" => Ok(Signal::SIGSEGV),
        "USR2" => Ok(Signal::SIGUSR2),
        "PIPE" => Ok(Signal::SIGPIPE),
        "ALRM" => Ok(Signal::SIGALRM),
        "TERM" => Ok(Signal::SIGTERM),
        "CHLD" | "CLD" => Ok(Signal::SIGCHLD),
        "CONT" => Ok(Signal::SIGCONT),
        "STOP" => Ok(Signal::SIGSTOP),
        "TSTP" => Ok(Signal::SIGTSTP),
        "TTIN" => Ok(Signal::SIGTTIN),
        "TTOU" => Ok(Signal::SIGTTOU),
        "URG" => Ok(Signal::SIGURG),
        "XCPU" => Ok(Signal::SIGXCPU),
        "XFSZ" => Ok(Signal::SIGXFSZ),
        "VTALRM" => Ok(Signal::SIGVTALRM),
        "PROF" => Ok(Signal::SIGPROF),
        "WINCH" => Ok(Signal::SIGWINCH),
        "IO" | "POLL" => Ok(Signal::SIGIO),
        "SYS" => Ok(Signal::SIGSYS),
        _ => Err(anyhow::anyhow!("invalid signal specification: {}", s)),
    }
}

/// Find a job by specification and return its PGID.
fn find_job(spec: &str, ctx: &CommandContext) -> anyhow::Result<Pid> {
    match spec {
        "+" | "%" => {
            // Current job
            ctx.state
                .jobs
                .last()
                .map(|j| j.pgid)
                .ok_or_else(|| anyhow::anyhow!("no current job"))
        }
        "-" => {
            // Previous job
            if ctx.state.jobs.len() >= 2 {
                Ok(ctx.state.jobs[ctx.state.jobs.len() - 2].pgid)
            } else {
                ctx.state
                    .jobs
                    .last()
                    .map(|j| j.pgid)
                    .ok_or_else(|| anyhow::anyhow!("no previous job"))
            }
        }
        _ => {
            // Try as job number
            if let Ok(id) = spec.parse::<u32>() {
                ctx.state
                    .jobs
                    .iter()
                    .find(|j| j.id == id)
                    .map(|j| j.pgid)
                    .ok_or_else(|| anyhow::anyhow!("no such job: %{}", id))
            } else {
                // Try as command prefix
                ctx.state
                    .jobs
                    .iter()
                    .find(|j| j.command.starts_with(spec))
                    .map(|j| j.pgid)
                    .ok_or_else(|| anyhow::anyhow!("no such job: %{}", spec))
            }
        }
    }
}

/// Get signal name from number.
fn signal_name(sig: i32) -> &'static str {
    match sig {
        1 => "HUP",
        2 => "INT",
        3 => "QUIT",
        4 => "ILL",
        5 => "TRAP",
        6 => "ABRT",
        7 => "BUS",
        8 => "FPE",
        9 => "KILL",
        10 => "USR1",
        11 => "SEGV",
        12 => "USR2",
        13 => "PIPE",
        14 => "ALRM",
        15 => "TERM",
        17 => "CHLD",
        18 => "CONT",
        19 => "STOP",
        20 => "TSTP",
        _ => "UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::signal::Signal;

    #[test]
    fn test_parse_signal_by_number() {
        assert_eq!(parse_signal("9").unwrap(), Signal::SIGKILL);
        assert_eq!(parse_signal("15").unwrap(), Signal::SIGTERM);
        assert_eq!(parse_signal("2").unwrap(), Signal::SIGINT);
    }

    #[test]
    fn test_parse_signal_by_name() {
        assert_eq!(parse_signal("KILL").unwrap(), Signal::SIGKILL);
        assert_eq!(parse_signal("TERM").unwrap(), Signal::SIGTERM);
        assert_eq!(parse_signal("INT").unwrap(), Signal::SIGINT);
        assert_eq!(parse_signal("HUP").unwrap(), Signal::SIGHUP);
    }

    #[test]
    fn test_parse_signal_with_sig_prefix() {
        assert_eq!(parse_signal("SIGKILL").unwrap(), Signal::SIGKILL);
        assert_eq!(parse_signal("SIGTERM").unwrap(), Signal::SIGTERM);
    }

    #[test]
    fn test_parse_signal_case_insensitive() {
        assert_eq!(parse_signal("kill").unwrap(), Signal::SIGKILL);
        assert_eq!(parse_signal("Kill").unwrap(), Signal::SIGKILL);
    }

    #[test]
    fn test_parse_signal_invalid() {
        assert!(parse_signal("INVALID").is_err());
        assert!(parse_signal("999").is_err());
    }

    #[test]
    fn test_signal_name() {
        assert_eq!(signal_name(9), "KILL");
        assert_eq!(signal_name(15), "TERM");
        assert_eq!(signal_name(2), "INT");
        assert_eq!(signal_name(999), "UNKNOWN");
    }
}
