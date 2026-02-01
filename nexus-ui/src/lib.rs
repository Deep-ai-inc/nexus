//! Nexus UI - GPU-accelerated terminal interface.
//!
//! This crate provides the graphical user interface for the Nexus terminal,
//! built on the Strata GPU rendering engine.
//!
//! # Module Organization
//!
//! - `nexus_app`: Main application (StrataApp implementation)
//! - `nexus_widgets`: UI widget components
//! - `blocks`: Block types and management
//! - `systems`: External system integrations (PTY, kernel, agent)
//! - `context`: Nexus context for environment info

// Nexus application (StrataApp implementation)
pub mod nexus_app;
pub mod nexus_widgets;

// Context system (minimal dependencies)
pub mod context;

// Existing modules needed by others
pub mod agent_adapter;
pub mod agent_block;
pub mod claude_cli;
pub mod mcp_proxy;

// Block types (depends on agent_block)
pub mod blocks;

// PTY handling (depends on blocks)
pub mod pty;

// Shell context (depends on blocks)
pub mod shell_context;

// Systems (depends on agent_adapter)
pub mod systems;
