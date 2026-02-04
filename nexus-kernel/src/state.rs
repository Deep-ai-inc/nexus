//! Shell state - environment, variables, jobs, working directory.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use nexus_api::{BlockId, Value};

use crate::parser::FunctionDef;
use crate::process::Job;

/// Stored output from a command block.
#[derive(Debug, Clone)]
pub struct BlockOutput {
    /// The block ID
    pub id: BlockId,
    /// The command that was run
    pub command: String,
    /// The structured output
    pub value: Value,
    /// When this was executed (unix timestamp)
    pub timestamp: u64,
}

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

/// Get the current block ID for this execution context.
/// If an external block_id was set (from UI), use that; otherwise generate new.
pub fn get_or_create_block_id(external_id: Option<BlockId>) -> BlockId {
    external_id.unwrap_or_else(next_block_id)
}

/// The shell's mutable state.
#[derive(Debug)]
pub struct ShellState {
    /// Environment variables (exported to child processes).
    pub env: HashMap<String, String>,

    /// Shell variables (not exported) - string values for bash compatibility.
    pub vars: HashMap<String, String>,

    /// Rich variables - hold any Value type (images, tables, etc.).
    /// These are Nexus-specific and support Mathematica-style value passing.
    pub rich_vars: HashMap<String, Value>,

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

    /// Shell functions (name -> definition).
    pub functions: HashMap<String, FunctionDef>,

    /// Local variable scope stack (for function calls).
    /// Each entry is a set of local variable names for that scope.
    local_scopes: Vec<HashMap<String, String>>,

    // === Persistent Memory (Year 3000 Terminal) ===
    /// Last command output - accessible via $_ or $prev
    pub last_output: Option<Value>,

    /// Recent block outputs - ring buffer of last N outputs
    /// Accessible via $_1, $_2, etc. (1 = most recent, 2 = second most recent)
    pub block_outputs: VecDeque<BlockOutput>,

    /// Maximum number of block outputs to retain
    pub max_block_outputs: usize,
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
            rich_vars: HashMap::new(),
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
            functions: HashMap::new(),
            local_scopes: Vec::new(),
            last_output: None,
            block_outputs: VecDeque::new(),
            max_block_outputs: 100, // Keep last 100 outputs
        })
    }

    /// Create a new shell state with a specific working directory.
    /// Inherits environment from the current process.
    pub fn from_cwd(cwd: PathBuf) -> Self {
        let env: HashMap<String, String> = std::env::vars().collect();

        Self {
            env,
            vars: HashMap::new(),
            rich_vars: HashMap::new(),
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
            functions: HashMap::new(),
            local_scopes: Vec::new(),
            last_output: None,
            block_outputs: VecDeque::new(),
            max_block_outputs: 100,
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

    /// Get a shell variable as string (checks vars first, then env).
    /// For rich variables, converts to text representation.
    pub fn get_var(&self, key: &str) -> Option<&str> {
        self.vars
            .get(key)
            .map(|s| s.as_str())
            .or_else(|| self.get_env(key))
    }

    /// Get a shell variable as a Value.
    /// Checks rich_vars first (returns the Value directly),
    /// then falls back to string vars (wrapped in Value::String).
    pub fn get_var_value(&self, key: &str) -> Option<Value> {
        // Check rich variables first
        if let Some(value) = self.rich_vars.get(key) {
            return Some(value.clone());
        }
        // Fall back to string variables
        if let Some(s) = self.vars.get(key) {
            return Some(Value::String(s.clone()));
        }
        // Finally check environment
        if let Some(s) = self.env.get(key) {
            return Some(Value::String(s.clone()));
        }
        None
    }

    /// Set a shell variable (not exported) - string value.
    pub fn set_var(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key = key.into();
        // Remove from rich_vars if it exists there
        self.rich_vars.remove(&key);
        self.vars.insert(key, value.into());
    }

    /// Set a shell variable to a rich Value.
    /// For simple strings, stores in vars. For complex types, stores in rich_vars.
    pub fn set_var_value(&mut self, key: impl Into<String>, value: Value) {
        let key = key.into();
        match value {
            Value::String(s) => {
                // Simple string - store in regular vars for bash compatibility
                self.rich_vars.remove(&key);
                self.vars.insert(key, s);
            }
            Value::Int(n) => {
                // Store integers as strings for bash compatibility
                self.rich_vars.remove(&key);
                self.vars.insert(key, n.to_string());
            }
            _ => {
                // Complex value - store in rich_vars
                self.vars.remove(&key);
                self.rich_vars.insert(key, value);
            }
        }
    }

    /// Check if a variable holds a rich (non-string) value.
    pub fn is_rich_var(&self, key: &str) -> bool {
        self.rich_vars.contains_key(key)
    }

    /// Change the working directory.
    ///
    /// Only updates the kernel's internal CWD — does NOT call
    /// `std::env::set_current_dir()`. The process-level CWD is
    /// meaningless in multi-window mode; child processes receive
    /// the correct CWD via fork/exec setup.
    pub fn set_cwd(&mut self, path: PathBuf) -> std::io::Result<()> {
        if !path.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{}: not a directory", path.display()),
            ));
        }
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

    // === Persistent Memory Methods ===

    /// Store a command's output, making it available via $_ and $_N references.
    pub fn store_output(&mut self, block_id: BlockId, command: String, value: Value) {
        use std::time::{SystemTime, UNIX_EPOCH};

        // Update last_output for $_ / $prev
        self.last_output = Some(value.clone());

        // Add to block_outputs ring buffer
        let output = BlockOutput {
            id: block_id,
            command,
            value,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        };

        self.block_outputs.push_front(output);

        // Trim if over max
        while self.block_outputs.len() > self.max_block_outputs {
            self.block_outputs.pop_back();
        }
    }

    /// Get the last output ($_ or $prev).
    pub fn get_last_output(&self) -> Option<&Value> {
        self.last_output.as_ref()
    }

    /// Get output by index (1 = most recent, 2 = second most recent, etc.).
    pub fn get_output_by_index(&self, index: usize) -> Option<&Value> {
        if index == 0 {
            return None;
        }
        self.block_outputs.get(index - 1).map(|o| &o.value)
    }

    /// Get output by block ID.
    pub fn get_output_by_id(&self, block_id: BlockId) -> Option<&Value> {
        self.block_outputs
            .iter()
            .find(|o| o.id == block_id)
            .map(|o| &o.value)
    }

    // === Function Methods ===

    /// Define a function.
    pub fn define_function(&mut self, name: String, def: FunctionDef) {
        self.functions.insert(name, def);
    }

    /// Get a function definition.
    pub fn get_function(&self, name: &str) -> Option<&FunctionDef> {
        self.functions.get(name)
    }

    /// Unset a function.
    pub fn unset_function(&mut self, name: &str) {
        self.functions.remove(name);
    }

    // === Local Variable Scope Methods ===

    /// Enter a new local scope (for function calls).
    pub fn push_scope(&mut self) {
        self.local_scopes.push(HashMap::new());
    }

    /// Exit the current local scope (restore old values).
    pub fn pop_scope(&mut self) {
        if let Some(scope) = self.local_scopes.pop() {
            // Remove all local variables from this scope
            for (name, old_value) in scope {
                if old_value.is_empty() {
                    // Variable didn't exist before, remove it
                    self.vars.remove(&name);
                } else {
                    // Restore old value
                    self.vars.insert(name, old_value);
                }
            }
        }
    }

    /// Declare a local variable (only valid inside a function).
    /// Returns true if successful, false if not in a function.
    pub fn declare_local(&mut self, name: impl Into<String>, value: impl Into<String>) -> bool {
        let name = name.into();
        let value = value.into();

        if self.local_scopes.is_empty() {
            // Not in a function - local does nothing (or could error)
            return false;
        }

        // Save the old value (or empty string if didn't exist) in the current scope
        let old_value = self.vars.get(&name).cloned().unwrap_or_default();
        if let Some(scope) = self.local_scopes.last_mut() {
            // Only save if we haven't already saved this variable in this scope
            scope.entry(name.clone()).or_insert(old_value);
        }

        // Set the new value
        self.vars.insert(name, value);
        true
    }

    /// Check if we're currently inside a function (for return builtin).
    pub fn in_function(&self) -> bool {
        !self.local_scopes.is_empty()
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
