//! Capability negotiation types for client↔agent handshake.

use serde::{Deserialize, Serialize};

/// Capabilities advertised by the client during Hello.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClientCaps {
    /// Client supports credit-based flow control.
    pub flow_control: bool,
    /// Client supports session resume.
    pub resume: bool,
    /// Client supports nesting (Nest/Unnest).
    pub nesting: bool,
    /// Client supports file transfer (FileRead/FileWrite).
    pub file_transfer: bool,
}

/// Capabilities advertised by the agent during HelloOk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentCaps {
    /// Agent supports credit-based flow control.
    pub flow_control: bool,
    /// Agent supports session resume with ring buffer replay.
    pub resume: bool,
    /// Agent supports nesting (Nest/Unnest relay mode).
    pub nesting: bool,
    /// Agent supports file transfer (FileRead/FileWrite).
    pub file_transfer: bool,
    /// Agent supports PTY allocation.
    pub pty: bool,
}
