//! Internal drag state machine — Pending/Active/Inactive with hysteresis.
//!
//! The 5px threshold prevents swallowing clicks on draggable elements.
//! When a drag is Pending and the mouse is released, it's forwarded as a
//! normal click rather than consumed as a drag.

use std::path::PathBuf;

use nexus_api::BlockId;
use strata::content_address::SourceId;
use strata::primitives::Point;

/// Drag hysteresis threshold in pixels (squared for faster comparison).
pub const DRAG_THRESHOLD_SQ: f32 = 25.0; // 5px

/// The drag state machine.
pub struct DragState {
    pub status: DragStatus,
}

impl DragState {
    pub fn new() -> Self {
        Self {
            status: DragStatus::Inactive,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.status, DragStatus::Active(_))
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.status, DragStatus::Pending { .. })
    }

    pub fn is_idle(&self) -> bool {
        matches!(self.status, DragStatus::Inactive)
    }
}

#[derive(Debug, Clone)]
pub enum DragStatus {
    /// No drag in progress.
    Inactive,
    /// Mouse is down on a draggable element, hasn't moved 5px yet.
    /// If ButtonReleased fires here, treat as normal click (forward to on_click).
    Pending {
        origin: Point,
        payload: DragPayload,
        source: SourceId,
    },
    /// Mouse has moved >5px from origin — drag is live.
    /// Ghost preview renders, drop targets highlight.
    Active(ActiveDrag),
}

#[derive(Debug, Clone)]
pub struct ActiveDrag {
    pub payload: DragPayload,
    pub origin: Point,
    pub current_pos: Point,
    pub source: SourceId,
}

/// What's being dragged.
#[derive(Debug, Clone)]
pub enum DragPayload {
    /// A text snippet.
    Text(String),
    /// A file path from ls/find output.
    FilePath(PathBuf),
    /// A table row — carries display text + semantic value.
    TableRow {
        block_id: BlockId,
        row_index: usize,
        display: String,
    },
    /// An entire block reference.
    Block(BlockId),
    /// An active cross-block selection with extracted text.
    Selection {
        text: String,
        structured: Option<StructuredSelection>,
    },
}

impl DragPayload {
    /// Short display text for the ghost preview (max 40 chars).
    pub fn preview_text(&self) -> String {
        let text = match self {
            Self::Text(s) => s.clone(),
            Self::FilePath(p) => p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned()),
            Self::TableRow { display, .. } => display.clone(),
            Self::Block(id) => format!("Block #{}", id.0),
            Self::Selection { text, .. } => text.clone(),
        };
        if text.len() > 40 {
            format!("{}...", &text[..37])
        } else {
            text
        }
    }
}

/// When a selection falls entirely within structured output, we can export
/// it as structured data rather than plain text.
#[derive(Debug, Clone)]
pub enum StructuredSelection {
    /// Selected rows from a table — export as TSV/JSON.
    TableRows {
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
}
