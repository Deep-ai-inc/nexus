//! Unified mouse interaction state machine — Pending/Active/Inactive with hysteresis.
//!
//! All mousedown-initiated interactions (anchor drags, text selection, future column
//! resize, etc.) flow through a single `PendingIntent`-discriminated state machine.
//! The 5px threshold prevents swallowing clicks on draggable elements.

use std::cell::Cell;
use std::path::PathBuf;
use std::time::Instant;

use nexus_api::BlockId;
use strata::MouseResponse;
use strata::content_address::{ContentAddress, SourceId};
use strata::event_context::{MouseButton, MouseEvent};
use strata::layout_snapshot::HitResult;
use strata::primitives::{Point, Rect};

use crate::app::message::{DragMsg, NexusMessage, SelectionMsg};

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
        /// Widget bounds for Quick Look zoom animation (local coordinates).
        source_rect: Option<Rect>,
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
    Selecting,
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

    /// Convert this payload to a native drag source.
    ///
    /// Takes a block lookup function for payloads that need to access block data
    /// (TableRow, Block). This keeps DragPayload decoupled from NexusState.
    pub fn to_drag_source<F>(&self, lookup_block: F) -> strata::DragSource
    where
        F: Fn(BlockId) -> Option<BlockSnapshot>,
    {
        match self {
            DragPayload::FilePath(p) => {
                if p.exists() {
                    strata::DragSource::File(p.clone())
                } else {
                    strata::DragSource::Text(p.to_string_lossy().into_owned())
                }
            }
            DragPayload::Text(s) => strata::DragSource::Text(s.clone()),
            DragPayload::TableRow { block_id, row_index, display } => {
                if let Some(snapshot) = lookup_block(*block_id) {
                    if let Some(tsv) = snapshot.row_as_tsv(*row_index) {
                        return strata::DragSource::Tsv(tsv);
                    }
                }
                strata::DragSource::Text(display.clone())
            }
            DragPayload::Block(id) => {
                if let Some(snapshot) = lookup_block(*id) {
                    // Prefer table TSV, then native output text, then terminal text
                    if let Some(tsv) = snapshot.table_as_tsv() {
                        let filename = format!("{}-output.tsv",
                            snapshot.command.split_whitespace().next().unwrap_or("block"));
                        match super::drop::write_drag_temp_file(&filename, tsv.as_bytes()) {
                            Ok(path) => return strata::DragSource::File(path),
                            Err(e) => {
                                tracing::warn!("Failed to write drag temp file: {}", e);
                                return strata::DragSource::Tsv(tsv);
                            }
                        }
                    }
                    if let Some(text) = snapshot.structured_output_text {
                        return strata::DragSource::Text(text);
                    }
                    return strata::DragSource::Text(snapshot.terminal_text);
                }
                strata::DragSource::Text(format!("block#{}", id.0))
            }
            DragPayload::Image { data, filename } => {
                let temp_dir = std::env::temp_dir().join("nexus-drag");
                let _ = std::fs::create_dir_all(&temp_dir);
                let path = temp_dir.join(filename);
                match std::fs::write(&path, data) {
                    Ok(()) => strata::DragSource::Image(path),
                    Err(e) => {
                        tracing::warn!("Failed to write image temp file: {}", e);
                        strata::DragSource::Text(filename.clone())
                    }
                }
            }
            DragPayload::Selection { text, structured } => {
                if let Some(StructuredSelection::TableRows { columns, rows }) = structured {
                    let mut tsv = columns.join("\t");
                    tsv.push('\n');
                    for row in rows {
                        tsv.push_str(&row.join("\t"));
                        tsv.push('\n');
                    }
                    strata::DragSource::Tsv(tsv)
                } else {
                    strata::DragSource::Text(text.clone())
                }
            }
        }
    }
}

/// Snapshot of block data needed for drag source conversion.
/// Avoids holding references across async boundaries.
pub struct BlockSnapshot {
    pub command: String,
    pub terminal_text: String,
    pub structured_output_text: Option<String>,
    /// Table data if structured_output is a Table
    table_columns: Option<Vec<String>>,
    table_rows: Option<Vec<Vec<String>>>,
}

impl BlockSnapshot {
    /// Create a snapshot from a Block.
    pub fn from_block(block: &crate::data::Block) -> Self {
        use nexus_api::Value;

        let (table_columns, table_rows) = if let Some(Value::Table { columns, rows }) = &block.structured_output {
            let cols: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
            let row_data: Vec<Vec<String>> = rows.iter()
                .map(|row| row.iter().map(|v| v.to_text()).collect())
                .collect();
            (Some(cols), Some(row_data))
        } else {
            (None, None)
        };

        Self {
            command: block.command.clone(),
            terminal_text: block.parser.grid_with_scrollback().to_string(),
            structured_output_text: block.structured_output.as_ref().map(|v| v.to_text()),
            table_columns,
            table_rows,
        }
    }

    /// Get a single row as TSV (header + row).
    pub fn row_as_tsv(&self, row_index: usize) -> Option<String> {
        let columns = self.table_columns.as_ref()?;
        let rows = self.table_rows.as_ref()?;
        let row = rows.get(row_index)?;
        Some(format!("{}\n{}", columns.join("\t"), row.join("\t")))
    }

    /// Get entire table as TSV.
    pub fn table_as_tsv(&self) -> Option<String> {
        let columns = self.table_columns.as_ref()?;
        let rows = self.table_rows.as_ref()?;
        let mut buf = columns.join("\t");
        buf.push('\n');
        for row in rows {
            buf.push_str(&row.join("\t"));
            buf.push('\n');
        }
        Some(buf)
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

    /// Would the next click at this position be a multi-click (double or triple)?
    ///
    /// Non-mutating peek — used to skip selection-drag routing on multi-clicks.
    pub fn would_be_multi_click(&self, pos: Point) -> bool {
        let (last_time, last_pos, count) = self.state.get();
        if let Some(t) = last_time {
            let dt = Instant::now().duration_since(t).as_millis();
            let dx = pos.x - last_pos.x;
            let dy = pos.y - last_pos.y;
            if dt < 500 && (dx * dx + dy * dy) < DRAG_THRESHOLD_SQ {
                return count >= 1; // next click would be count+1 ≥ 2
            }
        }
        false
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

// =========================================================================
// Drag mouse routing (extracted from event_routing.rs)
// =========================================================================

/// Route mouse events through the drag state machine.
///
/// Returns `Some(response)` if the drag intercepted the event,
/// `None` if the event should fall through to normal routing.
pub fn route_drag_mouse(
    status: &DragStatus,
    event: &MouseEvent,
    hit: Option<HitResult>,
    auto_scroll: &Cell<Option<f32>>,
    scroll_bounds: Rect,
) -> Option<MouseResponse<NexusMessage>> {
    match status {
        DragStatus::Active(ActiveKind::Selecting { .. }) => {
            Some(match event {
                MouseEvent::CursorMoved { position, .. } => {
                    update_auto_scroll(auto_scroll, scroll_bounds, position);
                    if let Some(HitResult::Content(addr)) = hit {
                        MouseResponse::message(NexusMessage::Selection(SelectionMsg::Extend(addr)))
                    } else {
                        MouseResponse::none()
                    }
                }
                MouseEvent::ButtonReleased {
                    button: MouseButton::Left,
                    ..
                } => {
                    auto_scroll.set(None);
                    MouseResponse::message_and_release(NexusMessage::Drag(DragMsg::Cancel))
                }
                MouseEvent::CursorLeft => {
                    auto_scroll.set(None);
                    MouseResponse::message_and_release(NexusMessage::Drag(DragMsg::Cancel))
                }
                _ => MouseResponse::none(),
            })
        }
        DragStatus::Pending { origin, .. } => {
            Some(match event {
                MouseEvent::CursorMoved { position, .. } => {
                    let dx = position.x - origin.x;
                    let dy = position.y - origin.y;
                    if dx * dx + dy * dy > DRAG_THRESHOLD_SQ {
                        MouseResponse::message(NexusMessage::Drag(DragMsg::Activate(*position)))
                    } else {
                        MouseResponse::none()
                    }
                }
                MouseEvent::ButtonReleased {
                    button: MouseButton::Left,
                    ..
                } => {
                    MouseResponse::message(NexusMessage::Drag(DragMsg::Cancel))
                }
                _ => MouseResponse::none(),
            })
        }
        DragStatus::Inactive => None,
    }
}

/// Start a text selection from a content click. Returns `None` if hit is not Content.
pub fn route_text_selection_start(
    click_tracker: &ClickTracker,
    hit: Option<HitResult>,
    position: Point,
) -> Option<MouseResponse<NexusMessage>> {
    if let Some(HitResult::Content(addr)) = hit {
        let mode = click_tracker.register_click(position, std::time::Instant::now());
        let capture_source = addr.source_id;
        Some(MouseResponse::message_and_capture(
            NexusMessage::Drag(DragMsg::StartSelecting(addr, mode)),
            capture_source,
        ))
    } else {
        None
    }
}

/// Compute auto-scroll speed based on cursor distance from scroll container edges.
///
/// 40px edge zone, proportional speed up to 8px per tick (~480px/s at 60fps).
fn update_auto_scroll(auto_scroll: &Cell<Option<f32>>, bounds: Rect, pos: &Point) {
    let edge = 40.0;
    let max_speed = 8.0;

    let speed = if pos.y < bounds.y + edge {
        let dist = bounds.y + edge - pos.y;
        -(dist / edge) * max_speed
    } else if pos.y > bounds.y + bounds.height - edge {
        let dist = pos.y - (bounds.y + bounds.height - edge);
        (dist / edge) * max_speed
    } else {
        0.0
    };

    if speed.abs() > 0.1 {
        auto_scroll.set(Some(speed));
    } else {
        auto_scroll.set(None);
    }
}
