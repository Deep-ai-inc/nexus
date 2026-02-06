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

    /// Get the exit code if the job is done.
    pub fn exit_code(&self) -> Option<i32> {
        match self.state {
            JobState::Done(code) => Some(code),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // JobState tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_job_state_equality() {
        assert_eq!(JobState::Running, JobState::Running);
        assert_eq!(JobState::Stopped, JobState::Stopped);
        assert_eq!(JobState::Done(0), JobState::Done(0));
        assert_ne!(JobState::Running, JobState::Stopped);
        assert_ne!(JobState::Done(0), JobState::Done(1));
    }

    #[test]
    fn test_job_state_clone() {
        let state = JobState::Done(42);
        let cloned = state.clone();
        assert_eq!(state, cloned);
    }

    #[test]
    fn test_job_state_debug() {
        let state = JobState::Running;
        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("Running"));
    }

    // -------------------------------------------------------------------------
    // Job::new tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_job_new() {
        let job = Job::new(1, Pid::from_raw(1234), "sleep 10".to_string());
        assert_eq!(job.id, 1);
        assert_eq!(job.pgid, Pid::from_raw(1234));
        assert_eq!(job.command, "sleep 10");
        assert_eq!(job.state, JobState::Running);
        assert_eq!(job.pids, vec![Pid::from_raw(1234)]);
    }

    #[test]
    fn test_job_new_starts_running() {
        let job = Job::new(5, Pid::from_raw(9999), "ls".to_string());
        assert!(job.is_running());
        assert!(!job.is_stopped());
        assert!(!job.is_done());
    }

    // -------------------------------------------------------------------------
    // Job state query tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_job_is_running() {
        let mut job = Job::new(1, Pid::from_raw(100), "cmd".to_string());
        assert!(job.is_running());

        job.state = JobState::Stopped;
        assert!(!job.is_running());

        job.state = JobState::Done(0);
        assert!(!job.is_running());
    }

    #[test]
    fn test_job_is_stopped() {
        let mut job = Job::new(1, Pid::from_raw(100), "cmd".to_string());
        assert!(!job.is_stopped());

        job.state = JobState::Stopped;
        assert!(job.is_stopped());

        job.state = JobState::Done(0);
        assert!(!job.is_stopped());
    }

    #[test]
    fn test_job_is_done() {
        let mut job = Job::new(1, Pid::from_raw(100), "cmd".to_string());
        assert!(!job.is_done());

        job.state = JobState::Stopped;
        assert!(!job.is_done());

        job.state = JobState::Done(0);
        assert!(job.is_done());

        job.state = JobState::Done(1);
        assert!(job.is_done());
    }

    #[test]
    fn test_job_exit_code() {
        let mut job = Job::new(1, Pid::from_raw(100), "cmd".to_string());
        assert_eq!(job.exit_code(), None);

        job.state = JobState::Stopped;
        assert_eq!(job.exit_code(), None);

        job.state = JobState::Done(0);
        assert_eq!(job.exit_code(), Some(0));

        job.state = JobState::Done(127);
        assert_eq!(job.exit_code(), Some(127));
    }

    // -------------------------------------------------------------------------
    // Job state transitions (simulating real scenarios)
    // -------------------------------------------------------------------------

    #[test]
    fn test_job_lifecycle_success() {
        let mut job = Job::new(1, Pid::from_raw(100), "echo hello".to_string());

        // Starts running
        assert!(job.is_running());

        // Completes successfully
        job.state = JobState::Done(0);
        assert!(job.is_done());
        assert_eq!(job.exit_code(), Some(0));
    }

    #[test]
    fn test_job_lifecycle_failure() {
        let mut job = Job::new(1, Pid::from_raw(100), "false".to_string());

        // Starts running
        assert!(job.is_running());

        // Fails with exit code 1
        job.state = JobState::Done(1);
        assert!(job.is_done());
        assert_eq!(job.exit_code(), Some(1));
    }

    #[test]
    fn test_job_lifecycle_stopped_then_resumed() {
        let mut job = Job::new(1, Pid::from_raw(100), "vim".to_string());

        // Starts running
        assert!(job.is_running());

        // User presses Ctrl+Z
        job.state = JobState::Stopped;
        assert!(job.is_stopped());
        assert!(!job.is_running());

        // User runs 'fg'
        job.state = JobState::Running;
        assert!(job.is_running());
        assert!(!job.is_stopped());
    }

    #[test]
    fn test_job_multiple_pids() {
        let mut job = Job::new(1, Pid::from_raw(100), "cmd1 | cmd2".to_string());
        job.pids.push(Pid::from_raw(101));

        assert_eq!(job.pids.len(), 2);
        assert!(job.pids.contains(&Pid::from_raw(100)));
        assert!(job.pids.contains(&Pid::from_raw(101)));
    }

    #[test]
    fn test_job_debug_output() {
        let job = Job::new(1, Pid::from_raw(100), "test".to_string());
        let debug_str = format!("{:?}", job);
        assert!(debug_str.contains("id: 1"));
        assert!(debug_str.contains("test"));
    }
}
