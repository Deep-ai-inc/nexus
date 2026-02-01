//! Unified mouse interaction state machine — Pending/Active/Inactive with hysteresis.
//!
//! All mousedown-initiated interactions (anchor drags, text selection, future column
//! resize, etc.) flow through a single `PendingIntent`-discriminated state machine.
//! The 5px threshold prevents swallowing clicks on draggable elements.

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Instant;

use nexus_api::BlockId;
use strata::content_address::{ContentAddress, SourceId};
use strata::primitives::Point;

/// Drag hysteresis threshold in pixels (squared for faster comparison).
pub const DRAG_THRESHOLD_SQ: f32 = 25.0; // 5px

/// The interaction state machine.
pub struct DragState {
    pub status: DragStatus,
    pub click_tracker: ClickTracker,
    /// Auto-scroll speed (pixels per tick). Positive = scroll down, negative = scroll up.
    /// `None` when not auto-scrolling.
    pub auto_scroll: Cell<Option<f32>>,
}

impl DragState {
    pub fn new() -> Self {
        Self {
            status: DragStatus::Inactive,
            click_tracker: ClickTracker::new(),
            auto_scroll: Cell::new(None),
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
    /// No interaction in progress.
    Inactive,
    /// Mouse is down, hasn't moved 5px yet. The `intent` determines what happens
    /// on threshold or release. Used for anchors and selection drags where there's
    /// genuine ambiguity between click and drag.
    Pending {
        origin: Point,
        intent: PendingIntent,
    },
    /// Interaction confirmed (>5px move, or immediate for text selection).
    Active(ActiveKind),
}

/// The specific intent determined at the moment of MouseDown.
/// Prevents "fallthrough" bugs (e.g., a failed anchor drag accidentally
/// becoming a text selection).
#[derive(Debug, Clone)]
pub enum PendingIntent {
    /// Clicked an anchor (link, file path, PID). Click = open, drag = ghost.
    Anchor {
        source: SourceId,
        payload: DragPayload,
    },
    /// Clicked inside an existing text selection. Click = place caret, drag = move text.
    SelectionDrag {
        source: SourceId,
        text: String,
        origin_addr: ContentAddress,
    },
    /// Clicked a table header edge (future: column resize).
    ColumnResize {
        source: SourceId,
        block_id: BlockId,
        col_index: usize,
        start_width: f32,
    },
    /// Clicked a table header center (future: column reorder).
    ColumnReorder {
        source: SourceId,
        block_id: BlockId,
        col_index: usize,
    },
    /// Clicked a row gutter (future: row drag).
    RowDrag {
        source: SourceId,
        block_id: BlockId,
        row_index: usize,
    },
    /// PTY app requested mouse (future: terminal capture).
    TerminalCapture {
        source: SourceId,
        block_id: BlockId,
    },
}

impl PendingIntent {
    /// The SourceId to capture mouse events to during Pending.
    pub fn source_id(&self) -> SourceId {
        match self {
            Self::Anchor { source, .. } => *source,
            Self::SelectionDrag { source, .. } => *source,
            Self::ColumnResize { source, .. } => *source,
            Self::ColumnReorder { source, .. } => *source,
            Self::RowDrag { source, .. } => *source,
            Self::TerminalCapture { source, .. } => *source,
        }
    }
}

/// What kind of active interaction is in progress.
#[derive(Debug, Clone)]
pub enum ActiveKind {
    /// Text selection in progress. Drives SelectionMsg::Extend on move.
    Selecting {
        start_addr: ContentAddress,
        mode: SelectMode,
    },
}

/// Text selection granularity, determined by click count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectMode {
    /// Normal single-click: character-level selection.
    Char,
    /// Double-click: word-level selection.
    Word,
    /// Triple-click: line-level selection.
    Line,
}

impl Default for SelectMode {
    fn default() -> Self {
        Self::Char
    }
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
    /// An image (raw data + filename for temp file).
    Image {
        data: Vec<u8>,
        filename: String,
    },
}

impl DragPayload {
    /// Display text for the ghost preview (max 8 lines, 80 chars each).
    pub fn preview_text(&self) -> String {
        let raw = match self {
            Self::Text(s) => s.clone(),
            Self::FilePath(p) => p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned()),
            Self::TableRow { display, .. } => display.clone(),
            Self::Block(id) => format!("Block #{}", id.0),
            Self::Selection { text, .. } => text.clone(),
            Self::Image { filename, .. } => filename.clone(),
        };
        let max_lines = 8;
        let max_line_len = 80;
        let lines: Vec<&str> = raw.lines().collect();
        let truncated_lines = lines.len() > max_lines;
        let mut result: Vec<String> = lines.iter()
            .take(max_lines)
            .map(|line| {
                if line.len() > max_line_len {
                    format!("{}...", &line[..max_line_len - 3])
                } else {
                    line.to_string()
                }
            })
            .collect();
        if truncated_lines {
            result.push("...".to_string());
        }
        result.join("\n")
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

/// Tracks rapid successive clicks at the same position for double/triple click detection.
///
/// Uses `Cell` for interior mutability since `on_mouse` takes `&self`.
/// State: (last_click_time, last_click_position, click_count).
pub struct ClickTracker {
    state: Cell<(Option<Instant>, Point, u8)>,
}

impl ClickTracker {
    pub fn new() -> Self {
        Self {
            state: Cell::new((None, Point::new(0.0, 0.0), 0)),
        }
    }

    /// Register a click and return the selection mode based on click count.
    ///
    /// - 1 click → Char
    /// - 2 clicks (within 500ms, <5px) → Word
    /// - 3 clicks → Line
    /// - 4+ clicks → wraps back to Char
    pub fn register_click(&self, pos: Point, now: Instant) -> SelectMode {
        let (last_time, last_pos, count) = self.state.get();
        if let Some(t) = last_time {
            let dt = now.duration_since(t).as_millis();
            let dx = pos.x - last_pos.x;
            let dy = pos.y - last_pos.y;
            if dt < 500 && (dx * dx + dy * dy) < DRAG_THRESHOLD_SQ {
                let new_count = if count >= 3 { 1 } else { count + 1 };
                self.state.set((Some(now), pos, new_count));
                return match new_count {
                    2 => SelectMode::Word,
                    3 => SelectMode::Line,
                    _ => SelectMode::Char,
                };
            }
        }
        self.state.set((Some(now), pos, 1));
        SelectMode::Char
    }
}
