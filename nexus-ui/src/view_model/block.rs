//! Block view model - a single command and its output.

use std::path::PathBuf;
use std::time::Instant;

use nexus_api::{BlockId, BlockState, OutputFormat};
use nexus_pump::sniffer;

/// View model for a single block.
pub struct BlockViewModel {
    /// Block ID.
    pub id: BlockId,

    /// The command that was executed.
    pub command: String,

    /// Working directory when command was executed.
    pub cwd: PathBuf,

    /// Stdout output.
    stdout: Vec<u8>,

    /// Stderr output.
    stderr: Vec<u8>,

    /// Current state.
    pub state: BlockState,

    /// Detected output format.
    pub format: OutputFormat,

    /// Duration in milliseconds (set when finished).
    pub duration_ms: Option<u64>,

    /// Whether the block is collapsed in the UI.
    pub collapsed: bool,

    /// When the command started.
    started_at: Instant,
}

impl BlockViewModel {
    /// Create a new block view model.
    pub fn new(id: BlockId, command: String, cwd: PathBuf) -> Self {
        Self {
            id,
            command,
            cwd,
            stdout: Vec::new(),
            stderr: Vec::new(),
            state: BlockState::Running,
            format: OutputFormat::PlainText,
            duration_ms: None,
            collapsed: false,
            started_at: Instant::now(),
        }
    }

    /// Append data to stdout.
    pub fn append_stdout(&mut self, data: &[u8]) {
        self.stdout.extend_from_slice(data);

        // Re-detect format if we have enough data
        if self.stdout.len() >= 512 && self.format == OutputFormat::PlainText {
            self.format = sniffer::detect_format(&self.stdout).kind;
        }
    }

    /// Append data to stderr.
    pub fn append_stderr(&mut self, data: &[u8]) {
        self.stderr.extend_from_slice(data);
    }

    /// Mark the block as finished.
    pub fn finish(&mut self, exit_code: i32, duration_ms: u64) {
        self.state = if exit_code == 0 {
            BlockState::Success
        } else {
            BlockState::Failed(exit_code)
        };
        self.duration_ms = Some(duration_ms);

        // Final format detection
        if !self.stdout.is_empty() {
            self.format = sniffer::detect_format(&self.stdout).kind;
        }
    }

    /// Get stdout as a string (lossy conversion).
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    /// Get stderr as a string (lossy conversion).
    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }

    /// Get raw stdout bytes.
    pub fn stdout_bytes(&self) -> &[u8] {
        &self.stdout
    }

    /// Get raw stderr bytes.
    pub fn stderr_bytes(&self) -> &[u8] {
        &self.stderr
    }

    /// Check if there's any stderr output.
    pub fn has_stderr(&self) -> bool {
        !self.stderr.is_empty()
    }

    /// Get the total output size in bytes.
    pub fn output_size(&self) -> usize {
        self.stdout.len() + self.stderr.len()
    }

    /// Toggle collapsed state.
    pub fn toggle_collapsed(&mut self) {
        self.collapsed = !self.collapsed;
    }
}
