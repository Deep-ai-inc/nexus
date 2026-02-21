//! Shared data models for the Nexus UI.
//!
//! Contains block types, agent blocks, jobs, and context.

pub mod blocks;
pub mod agent_block;
pub mod jobs;
pub mod providers;
pub mod context;

pub use blocks::{Block, ColumnFilter, FileTreeState, Focus, InputMode, ProcSort, PtyEvent, TableFilter, TableSort, UnifiedBlock, UnifiedBlockRef, ViewState};
pub use jobs::{VisualJob, VisualJobState};
