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
        /// The highest echo epoch the agent had written to the PTY master
        /// at the time this chunk was read. The client uses this to confirm
        /// or roll back local echo predictions.
        last_echo_epoch: u64,
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

    /// Progress update during remote connection establishment.
    RemoteConnectProgress {
        block_id: BlockId,
        stage: String,
        detail: Option<String>,
        /// 0.0–1.0 for determinate progress, None for indeterminate.
        progress: Option<f32>,
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

    /// Snapshot of terminal grid state for a block (sent on reconnect).
    /// Provides a complete viewport so the UI can render correctly even
    /// if the ring buffer has evicted older escape sequences.
    TerminalSnapshot {
        block_id: BlockId,
        grid: nexus_term::TerminalGrid,
        alt_screen: bool,
        app_cursor: bool,
        bracketed_paste: bool,
    },

    /// Terminal mode flags changed for a PTY block.
    /// Enables the UI to make intelligent decisions about local echo,
    /// cursor key encoding, paste wrapping, etc.
    TerminalModeChanged {
        block_id: BlockId,
        modes: TerminalModes,
    },

    /// Scrollback history sent on reconnect.
    /// Contains structured Cell rows from the agent's shadow parser,
    /// allowing the UI to populate the scrollback buffer with styled content
    /// (not just raw bytes that need re-parsing).
    ScrollbackHistory {
        block_id: BlockId,
        /// Flat row-major cell data: `rows * cols` cells, oldest row first.
        cells: Vec<nexus_term::Cell>,
        /// Number of columns per row.
        cols: u16,
    },
}

/// Terminal mode flags reported by the agent's shadow parser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalModes {
    /// Slave PTY has ECHO enabled (characters are echoed by the kernel).
    pub echo: bool,
    /// Slave PTY is in canonical (line-buffered) mode.
    pub icanon: bool,
    /// Alternate screen buffer is active (full-screen TUI app).
    pub alt_screen: bool,
    /// Application Cursor Keys mode (DECCKM) — arrows emit SS3.
    pub app_cursor: bool,
    /// Bracketed paste mode is enabled.
    pub bracketed_paste: bool,
}

/// State of a background job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobState {
    Running,
    Stopped,
    Done(i32),
}
