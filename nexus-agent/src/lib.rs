//! Nexus Agent - AI-powered coding assistant library.
//!
//! This crate provides the agent loop, tool system, and session management
//! for the Nexus shell's agent mode.

// ACP (Agent Client Protocol) integration
pub mod acp;

// Core modules (public API)
pub mod agent;
pub mod config;
pub mod mcp;
pub mod permissions;
pub mod persistence;
pub mod session;
pub mod tools;
pub mod types;
pub mod ui;
pub mod utils;

// Re-export commonly used types
pub use agent::{Agent, AgentComponents};
pub use session::{SessionConfig, SessionManager, SessionState};
pub use types::{PlanState, ToolSyntax};
pub use ui::{DisplayFragment, ToolStatus, UiEvent, UserInterface};

#[cfg(test)]
mod tests;
