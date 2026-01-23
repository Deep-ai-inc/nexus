//! Block representation - a command and its output.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

use crate::BlockId;

/// The detected format of command output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OutputFormat {
    #[default]
    PlainText,
    AnsiText,
    Json,
    JsonLines,
    Csv,
    Tsv,
    Xml,
    Binary,
}

/// Execution state of a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockState {
    /// Command is currently running.
    Running,
    /// Command completed successfully (exit code 0).
    Success,
    /// Command failed (non-zero exit code).
    Failed(i32),
    /// Command was killed by a signal.
    Killed(i32),
}

/// Metadata for a command block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockMeta {
    pub id: BlockId,
    pub command: String,
    pub cwd: PathBuf,
    pub started_at: SystemTime,
    pub finished_at: Option<SystemTime>,
    pub state: BlockState,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    pub detected_format: OutputFormat,
    pub truncated: bool,
}

impl BlockMeta {
    pub fn new(id: BlockId, command: String, cwd: PathBuf) -> Self {
        Self {
            id,
            command,
            cwd,
            started_at: SystemTime::now(),
            finished_at: None,
            state: BlockState::Running,
            stdout_bytes: 0,
            stderr_bytes: 0,
            detected_format: OutputFormat::default(),
            truncated: false,
        }
    }

    pub fn duration_ms(&self) -> Option<u64> {
        self.finished_at.map(|end| {
            end.duration_since(self.started_at)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        })
    }
}
