//! Shared data models for the Nexus UI.
//!
//! Contains block types, agent blocks, jobs, and context.

pub mod blocks;
pub mod agent_block;
pub mod jobs;
pub mod project;
pub mod context;

pub use blocks::{Block, FileTreeState, Focus, InputMode, ProcSort, PtyEvent, TableSort, UnifiedBlock, UnifiedBlockRef, ViewState, VisualJob, VisualJobState};
