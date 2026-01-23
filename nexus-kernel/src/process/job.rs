//! Job control structures.

use nix::unistd::Pid;

/// State of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Running,
    Stopped,
    Done(i32),
}

/// A background job.
#[derive(Debug)]
pub struct Job {
    /// Job number (for %N references).
    pub id: u32,

    /// Process group ID (same as the leader's PID).
    pub pgid: Pid,

    /// PIDs of all processes in the job.
    pub pids: Vec<Pid>,

    /// The command string.
    pub command: String,

    /// Current state.
    pub state: JobState,
}

impl Job {
    /// Create a new job.
    pub fn new(id: u32, pgid: Pid, command: String) -> Self {
        Self {
            id,
            pgid,
            pids: vec![pgid],
            command,
            state: JobState::Running,
        }
    }

    /// Check if the job is still running.
    pub fn is_running(&self) -> bool {
        self.state == JobState::Running
    }

    /// Check if the job is stopped.
    pub fn is_stopped(&self) -> bool {
        self.state == JobState::Stopped
    }

    /// Check if the job is done.
    pub fn is_done(&self) -> bool {
        matches!(self.state, JobState::Done(_))
    }
}
