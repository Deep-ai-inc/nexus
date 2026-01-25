//! Native Claude Code UI integration for Nexus.
//!
//! This crate provides typed integration with Claude Code using the `claude-codes`
//! crate, enabling rich native UI rendering of Claude's output.
//!
//! # Architecture
//!
//! Uses `claude-codes` AsyncClient to communicate with Claude CLI via JSON Lines
//! protocol. This gives us typed access to:
//!
//! - Text responses
//! - Thinking blocks (collapsible reasoning)
//! - Tool executions (with status and output)
//! - Permission requests
//!
//! # Usage
//!
//! ```rust,ignore
//! use nexus_claude::{ClaudeSession, ClaudeConversation};
//! use std::path::PathBuf;
//!
//! // Create a session
//! let (mut session, mut rx) = ClaudeSession::new(PathBuf::from("/project")).await?;
//!
//! // Create conversation state for UI
//! let mut conversation = ClaudeConversation::new();
//!
//! // Send a message
//! session.send_message("Hello, Claude!").await?;
//!
//! // Process responses
//! while let Some(msg) = rx.recv().await {
//!     match msg {
//!         ReaderMessage::Output(output) => {
//!             // Update conversation state based on output
//!         }
//!         ReaderMessage::Closed => break,
//!         ReaderMessage::Error(e) => eprintln!("Error: {}", e),
//!     }
//! }
//! ```

pub mod block;
pub mod process;

// Re-export main types
pub use block::{
    ClaudeBlock, ClaudeConversation, ContextFile, ConversationState, DiffStatus, MessageLevel,
    ToolStatus,
};
pub use process::{BlockAccumulator, ClaudeSession, ProcessError, ReaderMessage};

// Re-export claude-codes types that consumers need
pub use claude_codes::{ClaudeOutput, ClaudeInput, ContentBlock, ControlRequestPayload};
