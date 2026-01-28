//! Action Registry for Nexus.
//!
//! Centralizes all user-invokable actions with metadata for:
//! - Command palette (fuzzy search by name/keywords)
//! - Keybindings (configurable shortcuts)
//! - Context menus (filtered by availability)
//! - Discoverability (browsable action list)
//!
//! This is the "control layer" that decouples UI from logic.

mod registry;
mod types;

pub use registry::ActionRegistry;
pub use types::{Action, ActionContext, ActionId, KeyCombo, Modifiers};
