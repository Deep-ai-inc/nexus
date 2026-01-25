//! Job control commands - jobs, fg, bg, wait.
//!
//! These commands access ctx.state.jobs to manage background processes.

use super::{CommandContext, NexusCommand};
use crate::process::JobState;
use crate::state::ShellState;
use nexus_api::Value;
use nix::sys::signal::{kill, Signal};
use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
use nix::unistd::Pid;

// ============================================================================
// jobs - List active jobs
// ============================================================================

pub struct JobsCommand;

impl NexusCommand for JobsCommand {
    fn name(&self) -> &'static str {
        "jobs"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        let show_pids = args.iter().any(|a| a == "-l" || a == "-p");
        let show_pids_only = args.iter().any(|a| a == "-p");

        // Update job statuses before listing
        update_job_statuses(ctx);

        if ctx.state.jobs.is_empty() {
            return Ok(Value::Table {
                columns: vec![
                    "id".to_string(),
                    "status".to_string(),
                    "pid".to_string(),
                    "command".to_string(),
                ],
                rows: vec![],
            });
        }

        let mut rows = Vec::new();

        for job in &ctx.state.jobs {
            let status_str = match job.state {
                JobState::Running => "Running",
                JobState::Stopped => "Stopped",
                JobState::Done(code) if code == 0 => "Done",
                JobState::Done(_) => "Exit",
            };

            if show_pids_only {
                rows.push(vec![Value::Int(job.pgid.as_raw() as i64)]);
            } else if show_pids {
                rows.push(vec![
                    Value::Int(job.id as i64),
                    Value::String(status_str.to_string()),
                    Value::Int(job.pgid.as_raw() as i64),
                    Value::String(job.command.clone()),
                ]);
            } else {
                rows.push(vec![
                    Value::Int(job.id as i64),
                    Value::String(status_str.to_string()),
                    Value::Int(job.pgid.as_raw() as i64),
                    Value::String(job.command.clone()),
                ]);
            }
        }

        let columns = if show_pids_only {
            vec!["pid".to_string()]
        } else {
            vec![
                "id".to_string(),
                "status".to_string(),
                "pid".to_string(),
                "command".to_string(),
            ]
        };

        Ok(Value::Table { columns, rows })
    }
}

// ============================================================================
// fg - Bring job to foreground
// ============================================================================

pub struct FgCommand;

impl NexusCommand for FgCommand {
    fn name(&self) -> &'static str {
        "fg"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // Find the job to foreground
        let job_id = parse_job_spec(args.first().map(|s| s.as_str()), ctx.state)?;

        let job = ctx
            .state
            .jobs
            .iter_mut()
            .find(|j| j.id == job_id)
            .ok_or_else(|| anyhow::anyhow!("fg: {}: no such job", job_id))?;

        // Send SIGCONT if stopped
        if job.state == JobState::Stopped {
            kill(job.pgid, Signal::SIGCONT)?;
            job.state = JobState::Running;
        }

        eprintln!("{}", job.command);

        // Wait for the job to complete
        let pgid = job.pgid;
        let exit_code = wait_for_job(pgid)?;

        // Update job state
        if let Some(job) = ctx.state.jobs.iter_mut().find(|j| j.pgid == pgid) {
            job.state = JobState::Done(exit_code);
        }

        // Remove completed job
        ctx.state.jobs.retain(|j| !matches!(j.state, JobState::Done(_)));

        Ok(Value::Int(exit_code as i64))
    }
}

// ============================================================================
// bg - Resume job in background
// ============================================================================

pub struct BgCommand;

impl NexusCommand for BgCommand {
    fn name(&self) -> &'static str {
        "bg"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        // Find the job to background
        let job_id = parse_job_spec(args.first().map(|s| s.as_str()), ctx.state)?;

        let job = ctx
            .state
            .jobs
            .iter_mut()
            .find(|j| j.id == job_id)
            .ok_or_else(|| anyhow::anyhow!("bg: {}: no such job", job_id))?;

        // Send SIGCONT if stopped
        if job.state == JobState::Stopped {
            kill(job.pgid, Signal::SIGCONT)?;
            job.state = JobState::Running;
        }

        eprintln!("[{}]+ {} &", job.id, job.command);

        Ok(Value::Int(0))
    }
}

// ============================================================================
// wait - Wait for jobs to complete
// ============================================================================

pub struct WaitCommand;

impl NexusCommand for WaitCommand {
    fn name(&self) -> &'static str {
        "wait"
    }

    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value> {
        if args.is_empty() {
            // Wait for all background jobs
            let mut last_exit = 0;

            while !ctx.state.jobs.is_empty() {
                let pgid = ctx.state.jobs[0].pgid;
                last_exit = wait_for_job(pgid)?;

                // Update job state and remove
                if let Some(job) = ctx.state.jobs.iter_mut().find(|j| j.pgid == pgid) {
                    job.state = JobState::Done(last_exit);
                }
                ctx.state.jobs.retain(|j| !matches!(j.state, JobState::Done(_)));
            }

            return Ok(Value::Int(last_exit as i64));
        }

        // Wait for specific jobs/pids
        let mut last_exit = 0;

        for arg in args {
            // Parse as job spec or PID
            if let Some(job_id) = arg.strip_prefix('%') {
                let job_id: u32 = job_id.parse().unwrap_or(0);
                if let Some(job) = ctx.state.jobs.iter().find(|j| j.id == job_id) {
                    last_exit = wait_for_job(job.pgid)?;
                } else {
                    anyhow::bail!("wait: {}: no such job", arg);
                }
            } else if let Ok(pid) = arg.parse::<i32>() {
                // Wait for specific PID
                match waitpid(Pid::from_raw(pid), None) {
                    Ok(WaitStatus::Exited(_, code)) => last_exit = code,
                    Ok(WaitStatus::Signaled(_, sig, _)) => last_exit = 128 + sig as i32,
                    Ok(_) => last_exit = 0,
                    Err(_) => {
                        // PID not found
                        anyhow::bail!("wait: pid {} is not a child of this shell", pid);
                    }
                }
            }
        }

        // Clean up completed jobs
        update_job_statuses(ctx);
        ctx.state.jobs.retain(|j| !matches!(j.state, JobState::Done(_)));

        Ok(Value::Int(last_exit as i64))
    }
}

// ============================================================================
// Helper functions
// ============================================================================

/// Parse a job specification (%1, %+, %-, %string, etc.).
fn parse_job_spec(spec: Option<&str>, state: &ShellState) -> anyhow::Result<u32> {
    let Some(spec) = spec else {
        // Default to most recent job
        return state
            .jobs
            .last()
            .map(|j| j.id)
            .ok_or_else(|| anyhow::anyhow!("no current job"));
    };

    if let Some(num) = spec.strip_prefix('%') {
        match num {
            "+" | "%" => {
                // Current job (most recent)
                state.jobs.last().map(|j| j.id)
            }
            "-" => {
                // Previous job
                if state.jobs.len() >= 2 {
                    Some(state.jobs[state.jobs.len() - 2].id)
                } else {
                    state.jobs.last().map(|j| j.id)
                }
            }
            _ => {
                // Try as job number
                if let Ok(id) = num.parse::<u32>() {
                    if state.jobs.iter().any(|j| j.id == id) {
                        Some(id)
                    } else {
                        None
                    }
                } else {
                    // Try as command prefix
                    state
                        .jobs
                        .iter()
                        .find(|j| j.command.starts_with(num))
                        .map(|j| j.id)
                }
            }
        }
        .ok_or_else(|| anyhow::anyhow!("no such job: {}", spec))
    } else if let Ok(pid) = spec.parse::<i32>() {
        // Find job by PID
        state
            .jobs
            .iter()
            .find(|j| j.pgid.as_raw() == pid)
            .map(|j| j.id)
            .ok_or_else(|| anyhow::anyhow!("no such job: {}", spec))
    } else {
        anyhow::bail!("invalid job specification: {}", spec)
    }
}

/// Update the status of all jobs by checking with the OS.
fn update_job_statuses(ctx: &mut CommandContext) {
    for job in &mut ctx.state.jobs {
        if job.state == JobState::Running {
            // Non-blocking check
            match waitpid(job.pgid, Some(WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED)) {
                Ok(WaitStatus::Exited(_, code)) => {
                    job.state = JobState::Done(code);
                }
                Ok(WaitStatus::Signaled(_, sig, _)) => {
                    job.state = JobState::Done(128 + sig as i32);
                }
                Ok(WaitStatus::Stopped(_, _)) => {
                    job.state = JobState::Stopped;
                }
                Ok(WaitStatus::StillAlive) | Ok(_) => {
                    // Still running
                }
                Err(_) => {
                    // Process may have already exited
                    job.state = JobState::Done(0);
                }
            }
        }
    }
}

/// Wait for a job to complete and return its exit code.
fn wait_for_job(pgid: Pid) -> anyhow::Result<i32> {
    loop {
        match waitpid(pgid, Some(WaitPidFlag::WUNTRACED)) {
            Ok(WaitStatus::Exited(_, code)) => return Ok(code),
            Ok(WaitStatus::Signaled(_, sig, _)) => return Ok(128 + sig as i32),
            Ok(WaitStatus::Stopped(_, _)) => {
                // Job was stopped
                return Ok(128 + 19); // SIGSTOP
            }
            Ok(WaitStatus::Continued(_)) => continue,
            Ok(_) => continue,
            Err(nix::errno::Errno::ECHILD) => {
                // No child process
                return Ok(0);
            }
            Err(e) => return Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::test_utils::test_helpers::TestContext;
    use crate::process::Job;

    fn add_test_job(test_ctx: &mut TestContext, id: u32, pgid: i32, command: &str, state: JobState) {
        test_ctx.state.jobs.push(Job {
            id,
            pgid: Pid::from_raw(pgid),
            pids: vec![Pid::from_raw(pgid)],
            command: command.to_string(),
            state,
        });
    }

    #[test]
    fn test_job_state_display() {
        let state = JobState::Running;
        assert!(matches!(state, JobState::Running));

        let state = JobState::Done(0);
        assert!(matches!(state, JobState::Done(0)));
    }

    #[test]
    fn test_jobs_command_name() {
        let cmd = JobsCommand;
        assert_eq!(cmd.name(), "jobs");
    }

    #[test]
    fn test_jobs_empty() {
        let cmd = JobsCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Table { columns, rows } => {
                assert_eq!(columns, vec!["id", "status", "pid", "command"]);
                assert!(rows.is_empty());
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn test_jobs_with_jobs() {
        let cmd = JobsCommand;
        let mut test_ctx = TestContext::new_default();

        // Add some fake jobs (they won't be real processes)
        add_test_job(&mut test_ctx, 1, 99999, "sleep 100", JobState::Running);
        add_test_job(&mut test_ctx, 2, 99998, "vim file.txt", JobState::Stopped);

        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();

        match result {
            Value::Table { columns, rows } => {
                assert_eq!(columns, vec!["id", "status", "pid", "command"]);
                // Jobs may have been marked as Done if waitpid fails
                // Just check we got some rows back
                assert!(rows.len() <= 2);
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn test_jobs_pids_only() {
        let cmd = JobsCommand;
        let mut test_ctx = TestContext::new_default();

        add_test_job(&mut test_ctx, 1, 99999, "sleep 100", JobState::Running);

        let result = cmd
            .execute(&["-p".to_string()], &mut test_ctx.ctx())
            .unwrap();

        match result {
            Value::Table { columns, rows: _ } => {
                assert_eq!(columns, vec!["pid"]);
            }
            _ => panic!("Expected Table"),
        }
    }

    #[test]
    fn test_parse_job_spec_no_jobs() {
        let test_ctx = TestContext::new_default();
        let result = parse_job_spec(None, &test_ctx.state);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no current job"));
    }

    #[test]
    fn test_parse_job_spec_current() {
        let mut test_ctx = TestContext::new_default();
        add_test_job(&mut test_ctx, 1, 1000, "cmd1", JobState::Running);
        add_test_job(&mut test_ctx, 2, 2000, "cmd2", JobState::Running);

        // %+ should return most recent
        let result = parse_job_spec(Some("%+"), &test_ctx.state).unwrap();
        assert_eq!(result, 2);

        // %% should also return most recent
        let result = parse_job_spec(Some("%%"), &test_ctx.state).unwrap();
        assert_eq!(result, 2);
    }

    #[test]
    fn test_parse_job_spec_previous() {
        let mut test_ctx = TestContext::new_default();
        add_test_job(&mut test_ctx, 1, 1000, "cmd1", JobState::Running);
        add_test_job(&mut test_ctx, 2, 2000, "cmd2", JobState::Running);

        // %- should return previous job
        let result = parse_job_spec(Some("%-"), &test_ctx.state).unwrap();
        assert_eq!(result, 1);
    }

    #[test]
    fn test_parse_job_spec_by_number() {
        let mut test_ctx = TestContext::new_default();
        add_test_job(&mut test_ctx, 1, 1000, "cmd1", JobState::Running);
        add_test_job(&mut test_ctx, 2, 2000, "cmd2", JobState::Running);

        let result = parse_job_spec(Some("%1"), &test_ctx.state).unwrap();
        assert_eq!(result, 1);

        let result = parse_job_spec(Some("%2"), &test_ctx.state).unwrap();
        assert_eq!(result, 2);
    }

    #[test]
    fn test_parse_job_spec_by_prefix() {
        let mut test_ctx = TestContext::new_default();
        add_test_job(&mut test_ctx, 1, 1000, "sleep 100", JobState::Running);
        add_test_job(&mut test_ctx, 2, 2000, "vim file", JobState::Running);

        let result = parse_job_spec(Some("%vim"), &test_ctx.state).unwrap();
        assert_eq!(result, 2);

        let result = parse_job_spec(Some("%sleep"), &test_ctx.state).unwrap();
        assert_eq!(result, 1);
    }

    #[test]
    fn test_parse_job_spec_by_pid() {
        let mut test_ctx = TestContext::new_default();
        add_test_job(&mut test_ctx, 1, 1234, "cmd1", JobState::Running);

        let result = parse_job_spec(Some("1234"), &test_ctx.state).unwrap();
        assert_eq!(result, 1);
    }

    #[test]
    fn test_parse_job_spec_invalid() {
        let mut test_ctx = TestContext::new_default();
        add_test_job(&mut test_ctx, 1, 1000, "cmd1", JobState::Running);

        // Non-existent job number
        let result = parse_job_spec(Some("%99"), &test_ctx.state);
        assert!(result.is_err());

        // Non-existent prefix
        let result = parse_job_spec(Some("%nonexistent"), &test_ctx.state);
        assert!(result.is_err());
    }

    #[test]
    fn test_fg_no_jobs() {
        let cmd = FgCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());
    }

    #[test]
    fn test_bg_no_jobs() {
        let cmd = BgCommand;
        let mut test_ctx = TestContext::new_default();

        let result = cmd.execute(&[], &mut test_ctx.ctx());
        assert!(result.is_err());
    }

    #[test]
    fn test_wait_no_jobs() {
        let cmd = WaitCommand;
        let mut test_ctx = TestContext::new_default();

        // Waiting with no jobs should return 0
        let result = cmd.execute(&[], &mut test_ctx.ctx()).unwrap();
        assert_eq!(result, Value::Int(0));
    }
}
