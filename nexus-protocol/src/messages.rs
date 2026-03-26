//! Protocol message types for client↔agent communication.

use std::collections::HashMap;
use std::path::PathBuf;

use nexus_api::{BlockId, ShellEvent};
use serde::{Deserialize, Serialize};

// =========================================================================
// Client → Agent
// =========================================================================

/// Messages sent from the Nexus UI to the remote agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    // -- Handshake --
    /// Initial handshake with protocol version and capabilities.
    Hello {
        protocol_version: u32,
        capabilities: crate::ClientCaps,
        forwarded_env: HashMap<String, String>,
    },

    // -- Shell (maps to Kernel methods) --
    /// Execute a command through the kernel.
    Execute {
        id: u32,
        command: String,
        block_id: BlockId,
    },
    /// Classify a command as Kernel or Pty.
    Classify {
        id: u32,
        command: String,
    },
    /// Request tab completions.
    Complete {
        id: u32,
        input: String,
        cursor: usize,
    },
    /// Send SIGINT to the child process for this block.
    CancelBlock {
        id: u32,
        block_id: BlockId,
    },
    /// Search shell history.
    SearchHistory {
        id: u32,
        query: String,
        limit: u32,
    },

    // -- PTY --
    /// Spawn a PTY process.
    PtySpawn {
        id: u32,
        command: String,
        block_id: BlockId,
        cols: u16,
        rows: u16,
        term: String,
        cwd: String,
    },
    /// Send input data to a PTY.
    PtyInput {
        block_id: BlockId,
        data: Vec<u8>,
        /// Monotonically increasing epoch for local echo prediction.
        /// The agent reflects this back on StdoutChunk so the client
        /// knows which predictions have been consumed by the PTY.
        echo_epoch: u64,
    },
    /// Resize a PTY.
    PtyResize {
        block_id: BlockId,
        cols: u16,
        rows: u16,
    },
    /// Kill a PTY process.
    PtyKill {
        block_id: BlockId,
        signal: i32,
    },
    /// Send EOF (Ctrl+D) to a PTY without killing the process group.
    PtyClose {
        block_id: BlockId,
    },

    // -- Global viewport --
    /// Update viewport dimensions (affects native command output like `ls` column layout).
    TerminalResize {
        cols: u16,
        rows: u16,
    },

    // -- Filesystem --
    /// Read a file (chunked via offset/len).
    FileRead {
        id: u32,
        path: String,
        offset: u64,
        len: Option<u64>,
    },
    /// Write file data (chunked via offset).
    FileWrite {
        id: u32,
        path: String,
        offset: u64,
        data: Vec<u8>,
    },
    /// Cancel an in-progress file read. The agent should stop sending FileData
    /// chunks for this request ID.
    CancelFileRead {
        id: u32,
    },

    // -- Nesting --
    /// Deploy a child agent via the given transport and enter relay mode.
    Nest {
        id: u32,
        transport: Transport,
        force_redeploy: bool,
    },
    /// Tear down the child agent and exit relay mode.
    Unnest {
        id: u32,
    },

    // -- Flow control --
    /// Grant output credits to the agent (bytes it may send before pausing).
    GrantCredits {
        bytes: u64,
    },

    // -- Connection --
    /// Keepalive ping.
    Ping {
        seq: u64,
    },
    /// Resume a previous session after reconnection.
    Resume {
        session_token: [u8; 16],
        last_seen_seq: u64,
    },
    /// Graceful shutdown.
    Shutdown,
}

// =========================================================================
// Agent → Client
// =========================================================================

/// Messages sent from the remote agent to the Nexus UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    // -- Handshake --
    /// Successful handshake response.
    HelloOk {
        agent_version: String,
        env: EnvInfo,
        capabilities: crate::AgentCaps,
        session_token: [u8; 16],
    },

    // -- Shell events --
    /// A shell event from the kernel (wraps existing ShellEvent).
    Event {
        seq: u64,
        event: ShellEvent,
    },

    // -- Request responses --
    /// Result of a Classify request.
    ClassifyResult {
        id: u32,
        classification: CommandClassification,
    },
    /// Result of a Complete request.
    CompleteResult {
        id: u32,
        completions: Vec<CompletionItem>,
        start: usize,
    },
    /// Result of a SearchHistory request.
    HistoryResult {
        id: u32,
        entries: Vec<HistoryEntry>,
    },
    /// A chunk of file data.
    FileData {
        id: u32,
        data: Vec<u8>,
        /// True when this is the final chunk.
        eof: bool,
    },
    /// Result of a FileWrite request.
    FileWriteOk {
        id: u32,
        bytes_written: u64,
    },

    // -- Nesting --
    /// Child agent connected successfully.
    NestOk {
        id: u32,
        env: EnvInfo,
    },
    /// Child agent disconnected successfully.
    UnnestOk {
        id: u32,
        env: EnvInfo,
    },
    /// The child agent/connection in a relay died unexpectedly.
    /// `surviving_env` identifies the agent that is still alive (the parent).
    ChildLost {
        reason: String,
        surviving_env: EnvInfo,
    },

    // -- Flow control --
    /// Grant upload credits to the client (bytes it may send before pausing).
    GrantCredits {
        bytes: u64,
    },

    // -- Connection --
    /// Keepalive pong.
    Pong {
        seq: u64,
    },
    /// Session state for resume.
    SessionState {
        token: [u8; 16],
        env: EnvInfo,
        active_blocks: Vec<BlockId>,
    },
    /// Error response for a specific request.
    Error {
        id: u32,
        message: String,
    },
}

// =========================================================================
// Shared Types
// =========================================================================

/// Remote environment information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvInfo {
    /// Unique identifier for this agent session (UUID v4).
    /// Prevents identity collisions (docker containers, bastion hosts, `ssh localhost`).
    #[serde(default)]
    pub instance_id: String,
    pub user: String,
    pub hostname: String,
    pub cwd: PathBuf,
    pub os: String,
    pub arch: String,
}

/// Transport method for nesting (connecting to a deeper level).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Transport {
    Ssh {
        destination: String,
        port: Option<u16>,
        identity: Option<String>,
        extra_args: Vec<String>,
    },
    Docker {
        container: String,
        user: Option<String>,
    },
    Kubectl {
        pod: String,
        namespace: Option<String>,
        container: Option<String>,
    },
    Command {
        argv: Vec<String>,
    },
}

/// Mirrors `nexus_kernel::CommandClassification` for wire transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandClassification {
    Kernel,
    Pty,
    RemoteTransport,
}

/// A completion item sent over the wire.
/// Mirrors `nexus_kernel::Completion` with serde support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionItem {
    pub text: String,
    pub display: String,
    pub kind: CompletionKind,
    pub score: i32,
}

/// Completion kind for wire transport.
/// Mirrors `nexus_kernel::CompletionKind` with serde support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompletionKind {
    File,
    Directory,
    Executable,
    Builtin,
    NativeCommand,
    Function,
    Alias,
    Variable,
    GitBranch,
    Flag,
}

/// A shell history entry sent over the wire.
/// Mirrors `nexus_kernel::ShellHistoryEntry` with serde support.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub command: String,
    pub timestamp: Option<u64>,
}

impl Request {
    /// Returns the priority level for this request.
    pub fn priority(&self) -> u8 {
        match self {
            // Control plane — always first
            Request::CancelBlock { .. }
            | Request::CancelFileRead { .. }
            | Request::PtyInput { .. }
            | Request::Ping { .. }
            | Request::Shutdown => crate::priority::CONTROL,

            // Bulk data
            Request::FileWrite { .. } | Request::FileRead { .. } => crate::priority::BULK,

            // Everything else is interactive
            _ => crate::priority::INTERACTIVE,
        }
    }
}

impl Response {
    /// Returns the priority level for this response.
    pub fn priority(&self) -> u8 {
        match self {
            // Control plane
            Response::Pong { .. } | Response::ChildLost { .. } => crate::priority::CONTROL,

            // Bulk data
            Response::FileData { .. } => crate::priority::BULK,

            // Everything else is interactive
            _ => crate::priority::INTERACTIVE,
        }
    }
}
