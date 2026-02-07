//! Job tracking — owns the visual job list and handles kernel job events.

use crate::blocks::{VisualJob, VisualJobState};

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
