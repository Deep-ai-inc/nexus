//! Shell state - environment, variables, jobs, working directory.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use nexus_api::BlockId;

use crate::process::Job;

/// Actions that can be taken for a trapped signal.
#[derive(Debug, Clone)]
pub enum TrapAction {
    /// Reset to default signal handling
    Default,
    /// Ignore the signal
    Ignore,
    /// Execute a command string
    Command(String),
}

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

    /// Next job ID to assign.
    pub next_job_id: u32,

    /// Whether this is an interactive shell.
    pub interactive: bool,

    /// Last exit code.
    pub last_exit_code: i32,

    /// Last background job PID (for $!).
    pub last_bg_pid: Option<u32>,

    /// Shell aliases (name -> expansion).
    pub aliases: HashMap<String, String>,

    /// Read-only variables (cannot be unset or modified).
    pub readonly_vars: HashSet<String>,

    /// Positional parameters ($1, $2, ...).
    pub positional_params: Vec<String>,

    /// Shell options (set -e, set -x, etc.).
    pub options: ShellOptions,

    /// Signal traps (signal number -> action).
    pub traps: HashMap<i32, TrapAction>,

    /// Cached command paths (for hash builtin).
    pub command_hash: HashMap<String, PathBuf>,
}

/// Shell options controlled by `set` builtin.
#[derive(Debug, Default)]
pub struct ShellOptions {
    /// -e: Exit on error.
    pub errexit: bool,
    /// -u: Treat unset variables as error.
    pub nounset: bool,
    /// -x: Print commands before execution.
    pub xtrace: bool,
    /// -v: Print input lines as read.
    pub verbose: bool,
    /// -n: Read commands but do not execute.
    pub noexec: bool,
    /// -f: Disable filename expansion (globbing).
    pub noglob: bool,
    /// -C: Prevent overwriting with >.
    pub noclobber: bool,
    /// -a: Mark all variables for export.
    pub allexport: bool,
    /// -b: Notify of job completion immediately.
    pub notify: bool,
    /// -h: Remember command locations.
    pub hashall: bool,
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
            next_job_id: 1,
            interactive: true,
            last_exit_code: 0,
            last_bg_pid: None,
            aliases: HashMap::new(),
            readonly_vars: HashSet::new(),
            positional_params: Vec::new(),
            options: ShellOptions::default(),
            traps: HashMap::new(),
            command_hash: HashMap::new(),
        })
    }

    /// Create a new shell state with a specific working directory.
    /// Inherits environment from the current process.
    pub fn from_cwd(cwd: PathBuf) -> Self {
        let env: HashMap<String, String> = std::env::vars().collect();

        Self {
            env,
            vars: HashMap::new(),
            cwd,
            jobs: Vec::new(),
            next_job_id: 1,
            interactive: true,
            last_exit_code: 0,
            last_bg_pid: None,
            aliases: HashMap::new(),
            readonly_vars: HashSet::new(),
            positional_params: Vec::new(),
            options: ShellOptions::default(),
            traps: HashMap::new(),
            command_hash: HashMap::new(),
        }
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

    /// Check if a variable is readonly.
    pub fn is_readonly(&self, name: &str) -> bool {
        self.readonly_vars.contains(name)
    }

    /// Mark a variable as readonly.
    pub fn mark_readonly(&mut self, name: &str) {
        self.readonly_vars.insert(name.to_string());
    }
}

impl ShellOptions {
    /// Set a shell option by short flag (e.g., 'e' for errexit).
    pub fn set_option(&mut self, flag: char, value: bool) -> bool {
        match flag {
            'e' => self.errexit = value,
            'u' => self.nounset = value,
            'x' => self.xtrace = value,
            'v' => self.verbose = value,
            'n' => self.noexec = value,
            'f' => self.noglob = value,
            'C' => self.noclobber = value,
            'a' => self.allexport = value,
            'b' => self.notify = value,
            'h' => self.hashall = value,
            _ => return false,
        }
        true
    }

    /// Get a shell option by short flag.
    pub fn get_option(&self, flag: char) -> Option<bool> {
        match flag {
            'e' => Some(self.errexit),
            'u' => Some(self.nounset),
            'x' => Some(self.xtrace),
            'v' => Some(self.verbose),
            'n' => Some(self.noexec),
            'f' => Some(self.noglob),
            'C' => Some(self.noclobber),
            'a' => Some(self.allexport),
            'b' => Some(self.notify),
            'h' => Some(self.hashall),
            _ => None,
        }
    }

    /// Print current options in a format suitable for `set -o`.
    pub fn print_options(&self) -> String {
        let opts = [
            ("errexit", self.errexit),
            ("nounset", self.nounset),
            ("xtrace", self.xtrace),
            ("verbose", self.verbose),
            ("noexec", self.noexec),
            ("noglob", self.noglob),
            ("noclobber", self.noclobber),
            ("allexport", self.allexport),
            ("notify", self.notify),
            ("hashall", self.hashall),
        ];
        opts.iter()
            .map(|(name, val)| format!("set {}o {}", if *val { "-" } else { "+" }, name))
            .collect::<Vec<_>>()
            .join("\n")
    }
}
