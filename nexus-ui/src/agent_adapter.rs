//! Agent adapter for Iced UI.
//!
//! This module defines the events that flow from the Claude CLI to the UI.
//! Previously this bridged nexus-agent to Iced, but now it just defines
//! the event types that claude_cli.rs produces.

use crate::agent_block::ToolStatus;

/// Events sent from Claude CLI to UI.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Session initialized with ID (for conversation continuity).
    SessionStarted { session_id: String },
    /// Agent started processing.
    Started { request_id: u64 },
    /// Agent produced response text.
    ResponseText(String),
    /// Agent is thinking (reasoning).
    ThinkingText(String),
    /// Tool invocation started.
    ToolStarted { id: String, name: String },
    /// Tool parameter being streamed.
    ToolParameter { tool_id: String, name: String, value: String },
    /// Tool output chunk.
    ToolOutput { tool_id: String, chunk: String },
    /// Tool ended (before result).
    ToolEnded { id: String },
    /// Tool status update.
    ToolStatus {
        id: String,
        status: ToolStatus,
        message: Option<String>,
        output: Option<String>,
    },
    /// Image added.
    ImageAdded { media_type: String, data: String },
    /// Permission requested.
    PermissionRequested {
        id: String,
        tool_name: String,
        tool_id: String,
        description: String,
        action: String,
        working_dir: Option<String>,
    },
    /// Agent finished processing.
    Finished {
        request_id: u64,
        messages: Vec<()>, // Placeholder - CLI manages its own history
    },
    /// Agent was interrupted (Escape/Ctrl+C).
    Interrupted {
        request_id: u64,
        messages: Vec<()>, // Placeholder - CLI manages its own history
    },
    /// Agent encountered an error.
    Error(String),
}

/// Permission response from user.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PermissionResponse {
    GrantedOnce,
    GrantedSession,
    Denied,
}
