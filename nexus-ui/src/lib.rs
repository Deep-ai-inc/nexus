//! Nexus UI - Iced-based frontend for the Nexus shell.
//!
//! This crate provides the graphical user interface for the Nexus terminal,
//! built using the Iced GUI framework following the Elm architecture.
//!
//! # Module Organization
//!
//! - `app`: Main application coordinator (update, view, subscription)
//! - `state`: Application state (Nexus struct)
//! - `msg`: Message types that drive the update loop
//! - `constants`: Shared constants for rendering
//! - `blocks`: Block types and management
//! - `keymap`: Core logic (key handling, etc.)
//! - `ui`: View rendering modules
//! - `systems`: External system integrations (PTY, kernel, agent)
//! - `utils`: Utility functions

// Shared constants (no dependencies)
pub mod constants;

// Utility modules (minimal dependencies)
pub mod utils;

// Existing modules needed by others
pub mod agent_adapter;
pub mod agent_block;
pub mod claude_cli;
pub mod glyph_cache;
pub mod theme;
pub mod widgets;

// Block types (depends on agent_block, widgets)
pub mod blocks;

// Keymap (renamed from core to avoid conflict with std::core)
pub mod keymap;

// Message types (depends on agent_adapter, agent_block)
pub mod msg;

// State (depends on agent_adapter, agent_block, blocks)
pub mod state;

// PTY handling (depends on blocks)
pub mod pty;

// Shell context (depends on blocks)
pub mod shell_context;

// Systems (depends on agent_adapter, msg)
pub mod systems;

// UI views (depends on blocks, msg, state, utils)
pub mod ui;

// Handlers (depends on blocks, msg, state, systems)
pub mod handlers;

// Agent widgets (depends on agent_block)
pub mod agent_widgets;

// Main app (depends on everything)
pub mod app;
