//! External system integrations for the Nexus UI.
//!
//! This module contains subscriptions and spawning logic for:
//! - PTY (pseudo-terminal) processes
//! - Kernel (native command execution)
//! - Agent (AI assistant)

pub mod agent;
pub mod kernel;
pub mod permission_server;
pub mod pty;

pub use agent::{agent_subscription, spawn_agent_task};
pub use kernel::kernel_subscription;
pub use pty::pty_subscription;
