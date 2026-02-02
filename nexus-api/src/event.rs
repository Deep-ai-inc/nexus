//! Shell events emitted by the kernel to subscribers (UI, history, etc.)

use crate::Value;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Unique identifier for a command block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlockId(pub u64);

/// Events emitted by the shell kernel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShellEvent {
    /// A command has started executing.
    CommandStarted {
        block_id: BlockId,
        command: String,
        cwd: PathBuf,
    },

    /// A chunk of stdout data is available.
    StdoutChunk {
        block_id: BlockId,
        data: Vec<u8>,
    },

    /// A chunk of stderr data is available.
    StderrChunk {
        block_id: BlockId,
        data: Vec<u8>,
    },

    /// Structured output from a native (in-process) command.
    CommandOutput {
        block_id: BlockId,
        value: Value,
    },

    /// A command has finished executing.
    CommandFinished {
        block_id: BlockId,
        exit_code: i32,
        duration_ms: u64,
    },

    /// The current working directory changed.
    CwdChanged {
        old: PathBuf,
        new: PathBuf,
    },

    /// An environment variable was set or modified.
    EnvChanged {
        key: String,
        value: Option<String>, // None means unset
    },

    /// A job state changed (started, stopped, continued, terminated).
    JobStateChanged {
        job_id: u32,
        state: JobState,
    },

    /// Incremental streaming update from a long-running command.
    StreamingUpdate {
        block_id: BlockId,
        /// Monotonic sequence number for ordering.
        seq: u64,
        /// The update payload.
        update: Value,
        /// If true, replaces previous coalesced state (e.g., progress bar).
        /// If false, appends to the stream log (e.g., ping replies).
        coalesce: bool,
    },
}

/// State of a background job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Running,
    Stopped,
    Done(i32),
}
