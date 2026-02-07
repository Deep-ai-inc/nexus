//! Job tracking — owns the visual job list and handles kernel job events.

/// A job displayed in the status bar.
#[derive(Debug, Clone)]
pub struct VisualJob {
    pub id: u32,
    pub command: String,
    pub state: VisualJobState,
}

/// Visual state of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualJobState {
    Running,
    Stopped,
}

impl VisualJob {
    pub fn new(id: u32, command: String, state: VisualJobState) -> Self {
        Self { id, command, state }
    }

    /// Get a shortened display name for the job.
    pub fn display_name(&self) -> String {
        if self.command.len() > 20 {
            format!("{}...", &self.command[..17])
        } else {
            self.command.clone()
        }
    }

    /// Get the icon for this job state.
    pub fn icon(&self) -> &'static str {
        match self.state {
            VisualJobState::Running => "●",
            VisualJobState::Stopped => "⏸",
        }
    }
}

/// Manages the list of visual jobs (background processes shown in the job bar).
pub(crate) struct JobManager {
    jobs: Vec<VisualJob>,
}

impl JobManager {
    pub fn new() -> Self {
        Self { jobs: Vec::new() }
    }

    /// Process a kernel `JobStateChanged` event: create, update, or remove a job.
    pub fn handle_event(&mut self, job_id: u32, state: nexus_api::JobState) {
        match state {
            nexus_api::JobState::Running => {
                if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
                    job.state = VisualJobState::Running;
                } else {
                    self.jobs.push(VisualJob::new(
                        job_id,
                        format!("Job {}", job_id),
                        VisualJobState::Running,
                    ));
                }
            }
            nexus_api::JobState::Stopped => {
                if let Some(job) = self.jobs.iter_mut().find(|j| j.id == job_id) {
                    job.state = VisualJobState::Stopped;
                } else {
                    self.jobs.push(VisualJob::new(
                        job_id,
                        format!("Job {}", job_id),
                        VisualJobState::Stopped,
                    ));
                }
            }
            nexus_api::JobState::Done(_) => {
                self.jobs.retain(|j| j.id != job_id);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn as_slice(&self) -> &[VisualJob] {
        &self.jobs
    }

    pub fn iter(&self) -> std::slice::Iter<'_, VisualJob> {
        self.jobs.iter()
    }

    pub fn clear(&mut self) {
        self.jobs.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== VisualJob tests ==========

    #[test]
    fn test_visual_job_new() {
        let job = VisualJob::new(1, "sleep 100".to_string(), VisualJobState::Running);
        assert_eq!(job.id, 1);
        assert_eq!(job.command, "sleep 100");
        assert_eq!(job.state, VisualJobState::Running);
    }

    #[test]
    fn test_visual_job_display_name_short() {
        let job = VisualJob::new(1, "ls -la".to_string(), VisualJobState::Running);
        assert_eq!(job.display_name(), "ls -la");
    }

    #[test]
    fn test_visual_job_display_name_truncates_long() {
        let job = VisualJob::new(1, "this is a very long command that exceeds twenty chars".to_string(), VisualJobState::Running);
        let name = job.display_name();
        assert_eq!(name.len(), 20); // 17 chars + "..."
        assert!(name.ends_with("..."));
    }

    #[test]
    fn test_visual_job_display_name_exactly_20() {
        let job = VisualJob::new(1, "12345678901234567890".to_string(), VisualJobState::Running);
        assert_eq!(job.display_name(), "12345678901234567890");
    }

    #[test]
    fn test_visual_job_icon_running() {
        let job = VisualJob::new(1, "cmd".to_string(), VisualJobState::Running);
        assert_eq!(job.icon(), "●");
    }

    #[test]
    fn test_visual_job_icon_stopped() {
        let job = VisualJob::new(1, "cmd".to_string(), VisualJobState::Stopped);
        assert_eq!(job.icon(), "⏸");
    }

    #[test]
    fn test_visual_job_state_eq() {
        assert_eq!(VisualJobState::Running, VisualJobState::Running);
        assert_eq!(VisualJobState::Stopped, VisualJobState::Stopped);
        assert_ne!(VisualJobState::Running, VisualJobState::Stopped);
    }
}
