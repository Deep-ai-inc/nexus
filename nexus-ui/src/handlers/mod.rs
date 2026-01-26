//! Handler modules for the Nexus application.
//!
//! Handlers are organized by domain:
//! - `input`: Text input, completion, submission, key handling
//! - `terminal`: PTY output, kernel events, blocks, command execution
//! - `agent`: AI agent events and widget interactions
//! - `window`: Window resize, zoom, global shortcuts
//!
//! Supporting handlers:
//! - `history`: History search (pure functions)

pub mod agent;
pub mod history;
pub mod input;
pub mod terminal;
pub mod window;
