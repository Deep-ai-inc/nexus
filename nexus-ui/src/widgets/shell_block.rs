//! Shell block widget.
//!
//! Renders a shell command block with terminal output, native values, and viewers.
//!
//! # Future Work
//!
//! This module will eventually contain the full ShellBlockWidget implementation,
//! migrated from `nexus_widgets.rs`. For now, it re-exports the existing type.

// Re-export from existing location during migration
pub use crate::nexus_widgets::ShellBlockWidget;

// Schema for shell block source IDs (using existing source_ids module)
// These constants enable the SourceId::child() pattern for click handling.
pub mod id {
    pub const HEADER: u64 = 1;
    pub const TERMINAL: u64 = 2;
    pub const NATIVE_OUTPUT: u64 = 3;
    pub const KILL_BUTTON: u64 = 4;
    pub const EXIT_BUTTON: u64 = 5;
    pub const DURATION: u64 = 6;
}

// Message type for shell block interactions (future use)
#[derive(Debug, Clone)]
pub enum ShellBlockMessage {
    Kill,
    ExitViewer,
    ToggleCollapse,
    AnchorClick(strata::content_address::SourceId),
    TreeToggle(std::path::PathBuf),
}
