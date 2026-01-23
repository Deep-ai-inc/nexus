//! Block component - renders a single command block.

#![allow(dead_code)]

use nexus_api::{BlockState, OutputFormat};

/// Display mode for a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockDisplayMode {
    /// Full view with output.
    Expanded,
    /// Collapsed, showing only header.
    Collapsed,
    /// Fullscreen mode (for TUI apps).
    Fullscreen,
}

/// A block component representing a command and its output.
pub struct Block {
    /// Current display mode.
    mode: BlockDisplayMode,

    /// Selected lens for viewing output.
    lens: LensType,

    /// Whether the block is selected.
    selected: bool,
}

/// Type of lens to use for viewing output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LensType {
    #[default]
    Raw,
    Json,
    Table,
    Hex,
}

impl Block {
    /// Create a new block component.
    pub fn new() -> Self {
        Self {
            mode: BlockDisplayMode::Expanded,
            lens: LensType::Raw,
            selected: false,
        }
    }

    /// Get the recommended lens for the given format.
    pub fn recommended_lens(format: OutputFormat) -> LensType {
        match format {
            OutputFormat::Json | OutputFormat::JsonLines => LensType::Json,
            OutputFormat::Csv | OutputFormat::Tsv => LensType::Table,
            OutputFormat::Binary => LensType::Hex,
            _ => LensType::Raw,
        }
    }

    /// Set the display mode.
    pub fn set_mode(&mut self, mode: BlockDisplayMode) {
        self.mode = mode;
    }

    /// Get the current display mode.
    pub fn mode(&self) -> BlockDisplayMode {
        self.mode
    }

    /// Set the lens type.
    pub fn set_lens(&mut self, lens: LensType) {
        self.lens = lens;
    }

    /// Get the current lens type.
    pub fn lens(&self) -> LensType {
        self.lens
    }

    /// Toggle collapsed state.
    pub fn toggle_collapsed(&mut self) {
        self.mode = match self.mode {
            BlockDisplayMode::Collapsed => BlockDisplayMode::Expanded,
            _ => BlockDisplayMode::Collapsed,
        };
    }

    /// Set selected state.
    pub fn set_selected(&mut self, selected: bool) {
        self.selected = selected;
    }

    /// Check if selected.
    pub fn is_selected(&self) -> bool {
        self.selected
    }

    /// Get the status indicator for a block state.
    pub fn status_indicator(state: BlockState) -> &'static str {
        match state {
            BlockState::Running => "⟳",
            BlockState::Success => "✓",
            BlockState::Failed(_) => "✗",
            BlockState::Killed(_) => "⚡",
        }
    }

    /// Get the status color for a block state.
    pub fn status_color(state: BlockState) -> &'static str {
        match state {
            BlockState::Running => "#3794ff",  // Blue
            BlockState::Success => "#4ec9b0",  // Green
            BlockState::Failed(_) => "#f14c4c", // Red
            BlockState::Killed(_) => "#cca700", // Yellow
        }
    }
}

impl Default for Block {
    fn default() -> Self {
        Self::new()
    }
}
