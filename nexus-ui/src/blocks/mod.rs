//! Block management for the Nexus UI.
//!
//! This module contains types and logic for managing command blocks
//! (both shell and agent blocks) in the UI.

pub mod model;

pub use model::{Block, Focus, InputMode, PtyEvent, UnifiedBlock, UnifiedBlockRef};
