//! Nexus UI - GPU-accelerated terminal interface.
//!
//! This crate provides the graphical user interface for the Nexus terminal,
//! built on the Strata GPU rendering system.
//!
//! # Module Organization
//!
//! - `strata`: GPU-accelerated layout and rendering engine
//! - `blocks`: Block types and management
//! - `systems`: External system integrations (PTY, kernel, agent)
//! - `context`: Nexus context for environment info

// Strata: High-performance GUI abstraction layer
pub mod strata;

// Context system (minimal dependencies)
pub mod context;

// Existing modules needed by others
pub mod agent_adapter;
pub mod agent_block;
pub mod claude_cli;

// Block types (depends on agent_block)
pub mod blocks;

// PTY handling (depends on blocks)
pub mod pty;

// Shell context (depends on blocks)
pub mod shell_context;

// Systems (depends on agent_adapter)
pub mod systems;
