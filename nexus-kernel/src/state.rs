//! Shell state - environment, variables, jobs, working directory.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use nexus_api::BlockId;

use crate::process::Job;

/// Counter for generating unique block IDs.
static BLOCK_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a new unique block ID.
pub fn next_block_id() -> BlockId {
    BlockId(BLOCK_ID_COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// The shell's mutable state.
#[derive(Debug)]
pub struct ShellState {
    /// Environment variables (exported to child processes).
    pub env: HashMap<String, String>,

    /// Shell variables (not exported).
    pub vars: HashMap<String, String>,

    /// Current working directory.
    pub cwd: PathBuf,

    /// Active jobs.
    pub jobs: Vec<Job>,

    /// Last exit code.
    pub last_exit_code: i32,

    /// Last background job PID (for $!).
    pub last_bg_pid: Option<u32>,
}

impl ShellState {
    /// Create a new shell state, inheriting environment from the current process.
    pub fn new() -> anyhow::Result<Self> {
        let cwd = std::env::current_dir()?;
        let env: HashMap<String, String> = std::env::vars().collect();

        Ok(Self {
            env,
            vars: HashMap::new(),
            cwd,
            jobs: Vec::new(),
            last_exit_code: 0,
            last_bg_pid: None,
        })
    }

    /// Get an environment variable.
    pub fn get_env(&self, key: &str) -> Option<&str> {
        self.env.get(key).map(|s| s.as_str())
    }

    /// Set an environment variable.
    pub fn set_env(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.env.insert(key.into(), value.into());
    }

    /// Unset an environment variable.
    pub fn unset_env(&mut self, key: &str) {
        self.env.remove(key);
    }

    /// Get a shell variable (checks vars first, then env).
    pub fn get_var(&self, key: &str) -> Option<&str> {
        self.vars
            .get(key)
            .map(|s| s.as_str())
            .or_else(|| self.get_env(key))
    }

    /// Set a shell variable (not exported).
    pub fn set_var(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.vars.insert(key.into(), value.into());
    }

    /// Change the working directory.
    pub fn set_cwd(&mut self, path: PathBuf) -> std::io::Result<()> {
        std::env::set_current_dir(&path)?;
        self.cwd = path;
        Ok(())
    }
}
