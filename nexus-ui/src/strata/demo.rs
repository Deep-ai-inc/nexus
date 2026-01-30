//! Demo application exercising all Strata rendering features needed for Nexus.
//!
//! Uses composed widget structs (from `demo_widgets`) for the main UI blocks,
//! proving that the layout engine handles real nexus layouts. Overlay elements
//! (context menu, completion popup, table) remain as free functions since
//! they are absolutely positioned and don't participate in flex layout.
//!
//! Run with: `cargo run -p nexus-ui --example strata_demo`

use std::cell::Cell;
use std::time::Instant;

use crate::strata::content_address::{ContentAddress, SourceId};
use crate::strata::layout_snapshot::HitResult;
use crate::strata::demo_widgets::{
    AgentBlock, JobPanel, JobPill, PermissionDialog, ShellBlock, StatusIndicator,
    StatusPanel, ToolInvocation,
};
use crate::strata::event_context::{CaptureState, Key, KeyEvent, MouseButton, MouseEvent, NamedKey};
use crate::strata::layout::containers::Length;
use crate::strata::layout::primitives::LineStyle;
use crate::strata::primitives::{Color, Point, Rect};
use crate::strata::gpu::{ImageHandle, ImageStore};
use crate::strata::{
    AppConfig, Column, Command, LayoutSnapshot, MouseResponse, Row, ScrollColumn, Selection,
    StrataApp, Subscription, TableCell, TableElement, TextElement, TextInputElement,
};

// =========================================================================
// Nexus color palette (matches real app)
// =========================================================================
pub(crate) mod colors {
    use crate::strata::primitives::Color;

    // Backgrounds
    pub const BG_APP: Color = Color { r: 0.04, g: 0.04, b: 0.06, a: 1.0 };
    pub const BG_BLOCK: Color = Color { r: 0.08, g: 0.08, b: 0.11, a: 1.0 };
    pub const BG_INPUT: Color = Color { r: 0.10, g: 0.10, b: 0.13, a: 1.0 };
    pub const BG_CARD: Color = Color { r: 0.12, g: 0.12, b: 0.16, a: 1.0 };
    pub const BG_HOVER: Color = Color { r: 0.18, g: 0.30, b: 0.50, a: 0.5 };
    pub const BG_OVERLAY: Color = Color { r: 0.12, g: 0.12, b: 0.18, a: 0.95 };

    // Status
    pub const SUCCESS: Color = Color { r: 0.3, g: 0.8, b: 0.5, a: 1.0 };
    pub const ERROR: Color = Color { r: 0.8, g: 0.3, b: 0.3, a: 1.0 };
    pub const WARNING: Color = Color { r: 0.8, g: 0.5, b: 0.2, a: 1.0 };
    pub const RUNNING: Color = Color { r: 0.3, g: 0.7, b: 1.0, a: 1.0 };
    pub const PENDING: Color = Color { r: 0.6, g: 0.6, b: 0.3, a: 1.0 };
    pub const KILLED: Color = Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };

    // Text
    pub const TEXT_PRIMARY: Color = Color { r: 0.85, g: 0.85, b: 0.88, a: 1.0 };
    pub const TEXT_SECONDARY: Color = Color { r: 0.55, g: 0.55, b: 0.60, a: 1.0 };
    pub const TEXT_MUTED: Color = Color { r: 0.40, g: 0.40, b: 0.45, a: 1.0 };
    pub const TEXT_PATH: Color = Color { r: 0.39, g: 0.58, b: 0.93, a: 1.0 };
    pub const TEXT_QUERY: Color = Color { r: 0.5, g: 0.7, b: 1.0, a: 1.0 };
    pub const TEXT_PURPLE: Color = Color { r: 0.6, g: 0.4, b: 0.9, a: 1.0 };

    // Buttons
    pub const BTN_DENY: Color = Color { r: 0.6, g: 0.15, b: 0.15, a: 1.0 };
    pub const BTN_ALLOW: Color = Color { r: 0.15, g: 0.5, b: 0.25, a: 1.0 };
    pub const BTN_ALWAYS: Color = Color { r: 0.1, g: 0.35, b: 0.18, a: 1.0 };
    pub const BTN_KILL: Color = Color { r: 0.6, g: 0.2, b: 0.2, a: 1.0 };
    pub const BTN_MODE_SH: Color = Color { r: 0.12, g: 0.35, b: 0.18, a: 1.0 };

    // Borders
    pub const BORDER_SUBTLE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.08 };
    pub const BORDER_INPUT: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.12 };

    // Cursor
    pub const CURSOR: Color = Color { r: 0.85, g: 0.85, b: 0.88, a: 0.8 };
}

/// Monospace character width (must match containers.rs CHAR_WIDTH).
const CHAR_WIDTH: f32 = 8.4;
/// Line height (must match containers.rs LINE_HEIGHT).
const LINE_HEIGHT: f32 = 18.0;

/// Demo message type.
#[derive(Debug, Clone)]
pub enum DemoMessage {
    SelectionStart(ContentAddress),
    SelectionExtend(ContentAddress),
    SelectionEnd,
    Scroll(f32),
    /// Start dragging the scrollbar thumb. Carries the mouse Y at click time.
    ScrollDragStart(f32),
    ScrollDragMove(f32),
    ScrollDragEnd,
    ClearSelection,
    ScrollByKey(f32),
    ButtonClicked(SourceId),
    InputFocus(SourceId),
    InputBlur,
    InputChar(String),
    InputBackspace,
    InputDelete,
    InputLeft,
    InputRight,
    InputHome,
    InputEnd,
    InputSelectLeft,
    InputSelectRight,
    InputSelectAll,
    InputSubmit,
    /// Click in input at x offset (relative to input start)
    InputClickAt(SourceId, f32),
    /// Drag selection in input to x offset
    InputDragTo(f32),
    TimerTick,
    Copy,
    TableSort(SourceId),
    // Right panel scroll
    RightScroll(f32),
    RightScrollDragStart(f32),
    RightScrollDragMove(f32),
    RightScrollDragEnd,
    // Multi-line editor messages
    EditorChar(String),
    EditorBackspace,
    EditorDelete,
    EditorLeft,
    EditorRight,
    EditorUp,
    EditorDown,
    EditorHome,
    EditorEnd,
    EditorEnter,
    EditorScroll(f32),
    /// Click in editor at (x, y) relative to editor widget
    EditorClickAt(f32, f32),
    /// Drag in editor to (x, y) relative to editor widget
    EditorDragTo(f32, f32),
    EditorSelectAll,
}

/// Demo application state.
pub struct DemoState {
    // Stable source IDs for selectable content
    query_source: SourceId,
    response_source: SourceId,
    terminal_source: SourceId,
    tool_output_source: SourceId,
    // Widget IDs
    scroll_id: SourceId,
    scroll_thumb_id: SourceId,
    status_panel_id: SourceId,
    job_panel_id: SourceId,
    // Scroll state
    scroll_offset: f32,
    scroll_max: Cell<f32>,
    scroll_track: Cell<Option<crate::strata::layout_snapshot::ScrollTrackInfo>>,
    /// Distance from mouse click to top of thumb when drag started.
    scroll_grab_offset: f32,
    // Selection state
    selection: Option<Selection>,
    is_selecting: bool,
    // FPS tracking (Cell for interior mutability in view())
    last_frame: Cell<Instant>,
    fps_smooth: Cell<f32>,
    // Animation start time
    start_time: Instant,
    // Button IDs
    deny_btn_id: SourceId,
    allow_btn_id: SourceId,
    always_btn_id: SourceId,
    // Text input state
    input_id: SourceId,
    input_text: String,
    input_cursor: usize,
    input_selection: Option<(usize, usize)>,
    focused_input: Option<SourceId>,
    /// Last time the cursor moved or text was edited (for blink reset)
    last_edit_time: Instant,
    // Test image handle
    test_image: ImageHandle,
    // Subscription demo
    elapsed_seconds: u32,
    // Right panel scroll
    right_scroll_id: SourceId,
    right_scroll_thumb_id: SourceId,
    right_scroll_offset: f32,
    right_scroll_max: Cell<f32>,
    right_scroll_track: Cell<Option<crate::strata::layout_snapshot::ScrollTrackInfo>>,
    right_scroll_grab_offset: f32,
    right_scroll_bounds: Cell<Rect>,
    // Right panel overlay placeholder IDs
    context_menu_placeholder_id: SourceId,
    drawing_styles_placeholder_id: SourceId,
    // Table state
    sort_name_btn: SourceId,
    sort_size_btn: SourceId,
    table_source: SourceId,
    /// Which column is sorted: 0 = NAME, 1 = SIZE
    table_sort_col: usize,
    /// Sort ascending?
    table_sort_asc: bool,
    /// Table rows: (name, size_display, size_bytes, type)
    table_rows: Vec<(&'static str, &'static str, u32, &'static str, Color)>,
    // Multi-line editor state
    editor_panel_id: SourceId,
    editor_id: SourceId,
    editor_text: String,
    editor_cursor: usize,
    editor_selection: Option<(usize, usize)>,
    editor_scroll_offset: f32,
    // Widget bounds for mouse click → relative position
    input_bounds: Cell<Rect>,
    editor_bounds: Cell<Rect>,
}

/// Demo application.
pub struct DemoApp;

impl StrataApp for DemoApp {
    type State = DemoState;
    type Message = DemoMessage;

    fn init(images: &mut ImageStore) -> (Self::State, Command<Self::Message>) {
        let test_image = images
            .load_png("nexus-ui/assets/demo.png")
            .unwrap_or_else(|e| {
                eprintln!("Failed to load demo.png: {e}");
                images.load_test_gradient(128, 128)
            });
        let state = DemoState {
            query_source: SourceId::new(),
            response_source: SourceId::new(),
            terminal_source: SourceId::new(),
            tool_output_source: SourceId::new(),
            scroll_id: SourceId::new(),
            scroll_thumb_id: SourceId::new(),
            status_panel_id: SourceId::new(),
            job_panel_id: SourceId::new(),
            deny_btn_id: SourceId::new(),
            allow_btn_id: SourceId::new(),
            always_btn_id: SourceId::new(),
            input_id: SourceId::new(),
            input_text: String::new(),
            input_cursor: 0,
            input_selection: None,
            focused_input: None,
            last_edit_time: Instant::now(),
            scroll_offset: 0.0,
            scroll_max: Cell::new(f32::MAX),
            scroll_track: Cell::new(None),
            scroll_grab_offset: 0.0,
            selection: None,
            is_selecting: false,
            last_frame: Cell::new(Instant::now()),
            fps_smooth: Cell::new(0.0),
            start_time: Instant::now(),
            test_image,
            elapsed_seconds: 0,
            right_scroll_id: SourceId::new(),
            right_scroll_thumb_id: SourceId::new(),
            right_scroll_offset: 0.0,
            right_scroll_max: Cell::new(f32::MAX),
            right_scroll_track: Cell::new(None),
            right_scroll_grab_offset: 0.0,
            right_scroll_bounds: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            context_menu_placeholder_id: SourceId::new(),
            drawing_styles_placeholder_id: SourceId::new(),
            sort_name_btn: SourceId::new(),
            sort_size_btn: SourceId::new(),
            table_source: SourceId::new(),
            table_sort_col: 0,
            table_sort_asc: true,
            table_rows: vec![
                ("src/",       "256B", 256,  "dir",  colors::TEXT_PATH),
                ("main.rs",    "420B", 420,  "rust", colors::TEXT_PRIMARY),
                ("lib.rs",     "1.2K", 1200, "rust", colors::TEXT_PRIMARY),
                ("Cargo.toml", "890B", 890,  "toml", colors::TEXT_PRIMARY),
                ("README.md",  "2.4K", 2400, "md",   colors::TEXT_PRIMARY),
            ],
            editor_panel_id: SourceId::new(),
            editor_id: SourceId::new(),
            editor_text: "Hello, world!\nThis is a multi-line editor.\n\nTry typing here.".to_string(),
            editor_cursor: 0,
            editor_selection: None,
            editor_scroll_offset: 0.0,
            input_bounds: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
            editor_bounds: Cell::new(Rect::new(0.0, 0.0, 0.0, 0.0)),
        };
        (state, Command::none())
    }

    fn update(state: &mut Self::State, message: Self::Message, _images: &mut ImageStore) -> Command<Self::Message> {
        // Reset cursor blink on any edit/cursor action
        match &message {
            DemoMessage::InputChar(_) | DemoMessage::InputBackspace | DemoMessage::InputDelete
            | DemoMessage::InputLeft | DemoMessage::InputRight | DemoMessage::InputHome
            | DemoMessage::InputEnd | DemoMessage::InputSelectLeft | DemoMessage::InputSelectRight
            | DemoMessage::InputSelectAll | DemoMessage::InputFocus(_) | DemoMessage::InputClickAt(..)
            | DemoMessage::EditorChar(_) | DemoMessage::EditorBackspace | DemoMessage::EditorDelete
            | DemoMessage::EditorLeft | DemoMessage::EditorRight | DemoMessage::EditorUp
            | DemoMessage::EditorDown | DemoMessage::EditorHome | DemoMessage::EditorEnd
            | DemoMessage::EditorEnter | DemoMessage::EditorClickAt(..) | DemoMessage::EditorSelectAll => {
                state.last_edit_time = Instant::now();
            }
            _ => {}
        }
        match message {
            DemoMessage::SelectionStart(addr) => {
                state.selection = Some(Selection::new(addr.clone(), addr));
                state.is_selecting = true;
            }
            DemoMessage::SelectionExtend(addr) => {
                if let Some(sel) = &mut state.selection {
                    sel.focus = addr;
                }
            }
            DemoMessage::SelectionEnd => {
                state.is_selecting = false;
            }
            DemoMessage::Scroll(delta) => {
                let max = state.scroll_max.get();
                state.scroll_offset = (state.scroll_offset - delta).clamp(0.0, max);
            }
            DemoMessage::ScrollDragStart(mouse_y) => {
                if let Some(track) = state.scroll_track.get() {
                    let effective_offset = state.scroll_offset.clamp(0.0, state.scroll_max.get());
                    let thumb_top = track.thumb_y(effective_offset);
                    let thumb_bottom = thumb_top + track.thumb_height;

                    // Tolerance absorbs float rounding between layout and event frames.
                    const GRAB_TOLERANCE: f32 = 4.0;
                    if mouse_y >= (thumb_top - GRAB_TOLERANCE) && mouse_y <= (thumb_bottom + GRAB_TOLERANCE) {
                        // Clicked on the thumb: preserve grab offset so it doesn't jump.
                        state.scroll_grab_offset = mouse_y - thumb_top;
                    } else {
                        // Clicked on the track: jump thumb center to click point.
                        state.scroll_grab_offset = track.thumb_height / 2.0;
                        let new_offset = track.offset_from_y(mouse_y, state.scroll_grab_offset);
                        state.scroll_offset = new_offset.clamp(0.0, state.scroll_max.get());
                    }
                }
            }
            DemoMessage::ScrollDragMove(mouse_y) => {
                // Clamp output to prevent dead zones when dragging past edges.
                if let Some(track) = state.scroll_track.get() {
                    let new_offset = track.offset_from_y(mouse_y, state.scroll_grab_offset);
                    state.scroll_offset = new_offset.clamp(0.0, state.scroll_max.get());
                }
            }
            DemoMessage::ScrollDragEnd => {
                state.scroll_grab_offset = 0.0;
            }
            DemoMessage::RightScroll(delta) => {
                let max = state.right_scroll_max.get();
                state.right_scroll_offset = (state.right_scroll_offset - delta).clamp(0.0, max);
            }
            DemoMessage::RightScrollDragStart(mouse_y) => {
                if let Some(track) = state.right_scroll_track.get() {
                    let effective_offset = state.right_scroll_offset.clamp(0.0, state.right_scroll_max.get());
                    let thumb_top = track.thumb_y(effective_offset);
                    let thumb_bottom = thumb_top + track.thumb_height;
                    const GRAB_TOLERANCE: f32 = 4.0;
                    if mouse_y >= (thumb_top - GRAB_TOLERANCE) && mouse_y <= (thumb_bottom + GRAB_TOLERANCE) {
                        state.right_scroll_grab_offset = mouse_y - thumb_top;
                    } else {
                        state.right_scroll_grab_offset = track.thumb_height / 2.0;
                        let new_offset = track.offset_from_y(mouse_y, state.right_scroll_grab_offset);
                        state.right_scroll_offset = new_offset.clamp(0.0, state.right_scroll_max.get());
                    }
                }
            }
            DemoMessage::RightScrollDragMove(mouse_y) => {
                if let Some(track) = state.right_scroll_track.get() {
                    let new_offset = track.offset_from_y(mouse_y, state.right_scroll_grab_offset);
                    state.right_scroll_offset = new_offset.clamp(0.0, state.right_scroll_max.get());
                }
            }
            DemoMessage::RightScrollDragEnd => {
                state.right_scroll_grab_offset = 0.0;
            }
            DemoMessage::ClearSelection => {
                state.selection = None;
                state.is_selecting = false;
            }
            DemoMessage::ScrollByKey(delta) => {
                let max = state.scroll_max.get();
                state.scroll_offset = (state.scroll_offset - delta).clamp(0.0, max);
            }
            DemoMessage::ButtonClicked(id) => {
                if id == state.deny_btn_id {
                    eprintln!("[demo] Button clicked: Deny");
                } else if id == state.allow_btn_id {
                    eprintln!("[demo] Button clicked: Allow Once");
                } else if id == state.always_btn_id {
                    eprintln!("[demo] Button clicked: Allow Always");
                }
            }
            DemoMessage::InputFocus(id) => {
                state.focused_input = Some(id);
                state.input_selection = None;
            }
            DemoMessage::InputBlur => {
                state.focused_input = None;
                state.input_selection = None;
            }
            DemoMessage::InputChar(c) => {
                // Delete selection first
                if let Some((s, e)) = state.input_selection.take() {
                    let (lo, hi) = (s.min(e), s.max(e));
                    let lo_byte = state.input_text.char_indices().nth(lo).map(|(i, _)| i).unwrap_or(state.input_text.len());
                    let hi_byte = state.input_text.char_indices().nth(hi).map(|(i, _)| i).unwrap_or(state.input_text.len());
                    state.input_text.replace_range(lo_byte..hi_byte, "");
                    state.input_cursor = lo;
                }
                let byte_pos = state.input_text.char_indices().nth(state.input_cursor).map(|(i, _)| i).unwrap_or(state.input_text.len());
                state.input_text.insert_str(byte_pos, &c);
                state.input_cursor += c.chars().count();
            }
            DemoMessage::InputBackspace => {
                if let Some((s, e)) = state.input_selection.take() {
                    let (lo, hi) = (s.min(e), s.max(e));
                    let lo_byte = state.input_text.char_indices().nth(lo).map(|(i, _)| i).unwrap_or(state.input_text.len());
                    let hi_byte = state.input_text.char_indices().nth(hi).map(|(i, _)| i).unwrap_or(state.input_text.len());
                    state.input_text.replace_range(lo_byte..hi_byte, "");
                    state.input_cursor = lo;
                } else if state.input_cursor > 0 {
                    state.input_cursor -= 1;
                    let byte_pos = state.input_text.char_indices().nth(state.input_cursor).map(|(i, _)| i).unwrap_or(0);
                    let next = state.input_text.char_indices().nth(state.input_cursor + 1).map(|(i, _)| i).unwrap_or(state.input_text.len());
                    state.input_text.replace_range(byte_pos..next, "");
                }
            }
            DemoMessage::InputDelete => {
                if let Some((s, e)) = state.input_selection.take() {
                    let (lo, hi) = (s.min(e), s.max(e));
                    let lo_byte = state.input_text.char_indices().nth(lo).map(|(i, _)| i).unwrap_or(state.input_text.len());
                    let hi_byte = state.input_text.char_indices().nth(hi).map(|(i, _)| i).unwrap_or(state.input_text.len());
                    state.input_text.replace_range(lo_byte..hi_byte, "");
                    state.input_cursor = lo;
                } else {
                    let char_count = state.input_text.chars().count();
                    if state.input_cursor < char_count {
                        let byte_pos = state.input_text.char_indices().nth(state.input_cursor).map(|(i, _)| i).unwrap_or(0);
                        let next = state.input_text.char_indices().nth(state.input_cursor + 1).map(|(i, _)| i).unwrap_or(state.input_text.len());
                        state.input_text.replace_range(byte_pos..next, "");
                    }
                }
            }
            DemoMessage::InputLeft => {
                state.input_selection = None;
                if state.input_cursor > 0 { state.input_cursor -= 1; }
            }
            DemoMessage::InputRight => {
                state.input_selection = None;
                let len = state.input_text.chars().count();
                if state.input_cursor < len { state.input_cursor += 1; }
            }
            DemoMessage::InputHome => {
                state.input_selection = None;
                state.input_cursor = 0;
            }
            DemoMessage::InputEnd => {
                state.input_selection = None;
                state.input_cursor = state.input_text.chars().count();
            }
            DemoMessage::InputSelectLeft => {
                let anchor = match state.input_selection {
                    Some((a, _)) => a,
                    None => state.input_cursor,
                };
                if state.input_cursor > 0 {
                    state.input_cursor -= 1;
                    state.input_selection = Some((anchor, state.input_cursor));
                }
            }
            DemoMessage::InputSelectRight => {
                let anchor = match state.input_selection {
                    Some((a, _)) => a,
                    None => state.input_cursor,
                };
                let len = state.input_text.chars().count();
                if state.input_cursor < len {
                    state.input_cursor += 1;
                    state.input_selection = Some((anchor, state.input_cursor));
                }
            }
            DemoMessage::InputSelectAll => {
                let len = state.input_text.chars().count();
                state.input_selection = Some((0, len));
                state.input_cursor = len;
            }
            DemoMessage::InputSubmit => {
                eprintln!("[demo] Input submitted: {:?}", state.input_text);
                state.input_text.clear();
                state.input_cursor = 0;
                state.input_selection = None;
            }
            DemoMessage::InputClickAt(id, rel_x) => {
                state.focused_input = Some(id);
                let char_count = if id == state.editor_id {
                    state.editor_text.chars().count()
                } else {
                    state.input_text.chars().count()
                };
                let pos = (rel_x / CHAR_WIDTH).round().max(0.0) as usize;
                let pos = pos.min(char_count);
                if id == state.editor_id {
                    state.editor_cursor = pos;
                    state.editor_selection = None;
                } else {
                    state.input_cursor = pos;
                    state.input_selection = None;
                }
            }
            DemoMessage::InputDragTo(rel_x) => {
                let pos = (rel_x / CHAR_WIDTH).round().max(0.0) as usize;
                if state.focused_input == Some(state.editor_id) {
                    let len = state.editor_text.chars().count();
                    let pos = pos.min(len);
                    let anchor = state.editor_selection
                        .map(|(a, _)| a)
                        .unwrap_or(state.editor_cursor);
                    state.editor_selection = Some((anchor, pos));
                    state.editor_cursor = pos;
                } else {
                    let len = state.input_text.chars().count();
                    let pos = pos.min(len);
                    let anchor = state.input_selection
                        .map(|(a, _)| a)
                        .unwrap_or(state.input_cursor);
                    state.input_selection = Some((anchor, pos));
                    state.input_cursor = pos;
                }
            }
            DemoMessage::TimerTick => {
                state.elapsed_seconds += 1;
            }
            DemoMessage::TableSort(id) => {
                let col = if id == state.sort_name_btn { 0 } else { 1 };
                if state.table_sort_col == col {
                    state.table_sort_asc = !state.table_sort_asc;
                } else {
                    state.table_sort_col = col;
                    state.table_sort_asc = true;
                }
                match col {
                    0 => state.table_rows.sort_by(|a, b| {
                        if state.table_sort_asc { a.0.cmp(b.0) } else { b.0.cmp(a.0) }
                    }),
                    _ => state.table_rows.sort_by(|a, b| {
                        if state.table_sort_asc { a.2.cmp(&b.2) } else { b.2.cmp(&a.2) }
                    }),
                }
            }
            DemoMessage::Copy => {
                if let Some(sel) = &state.selection {
                    eprintln!(
                        "[demo] Copy: source={:?} offsets={}..{}",
                        sel.anchor.source_id,
                        sel.anchor.content_offset,
                        sel.focus.content_offset,
                    );
                }
            }
            // Multi-line editor messages
            DemoMessage::EditorChar(c) => {
                if let Some((s, e)) = state.editor_selection.take() {
                    let (lo, hi) = (s.min(e), s.max(e));
                    let lo_byte = state.editor_text.char_indices().nth(lo).map(|(i, _)| i).unwrap_or(state.editor_text.len());
                    let hi_byte = state.editor_text.char_indices().nth(hi).map(|(i, _)| i).unwrap_or(state.editor_text.len());
                    state.editor_text.replace_range(lo_byte..hi_byte, "");
                    state.editor_cursor = lo;
                }
                let byte_pos = state.editor_text.char_indices().nth(state.editor_cursor).map(|(i, _)| i).unwrap_or(state.editor_text.len());
                state.editor_text.insert_str(byte_pos, &c);
                state.editor_cursor += c.chars().count();
            }
            DemoMessage::EditorEnter => {
                if let Some((s, e)) = state.editor_selection.take() {
                    let (lo, hi) = (s.min(e), s.max(e));
                    let lo_byte = state.editor_text.char_indices().nth(lo).map(|(i, _)| i).unwrap_or(state.editor_text.len());
                    let hi_byte = state.editor_text.char_indices().nth(hi).map(|(i, _)| i).unwrap_or(state.editor_text.len());
                    state.editor_text.replace_range(lo_byte..hi_byte, "");
                    state.editor_cursor = lo;
                }
                let byte_pos = state.editor_text.char_indices().nth(state.editor_cursor).map(|(i, _)| i).unwrap_or(state.editor_text.len());
                state.editor_text.insert(byte_pos, '\n');
                state.editor_cursor += 1;
            }
            DemoMessage::EditorBackspace => {
                if let Some((s, e)) = state.editor_selection.take() {
                    let (lo, hi) = (s.min(e), s.max(e));
                    let lo_byte = state.editor_text.char_indices().nth(lo).map(|(i, _)| i).unwrap_or(state.editor_text.len());
                    let hi_byte = state.editor_text.char_indices().nth(hi).map(|(i, _)| i).unwrap_or(state.editor_text.len());
                    state.editor_text.replace_range(lo_byte..hi_byte, "");
                    state.editor_cursor = lo;
                } else if state.editor_cursor > 0 {
                    state.editor_cursor -= 1;
                    let byte_pos = state.editor_text.char_indices().nth(state.editor_cursor).map(|(i, _)| i).unwrap_or(0);
                    let next = state.editor_text.char_indices().nth(state.editor_cursor + 1).map(|(i, _)| i).unwrap_or(state.editor_text.len());
                    state.editor_text.replace_range(byte_pos..next, "");
                }
            }
            DemoMessage::EditorDelete => {
                if let Some((s, e)) = state.editor_selection.take() {
                    let (lo, hi) = (s.min(e), s.max(e));
                    let lo_byte = state.editor_text.char_indices().nth(lo).map(|(i, _)| i).unwrap_or(state.editor_text.len());
                    let hi_byte = state.editor_text.char_indices().nth(hi).map(|(i, _)| i).unwrap_or(state.editor_text.len());
                    state.editor_text.replace_range(lo_byte..hi_byte, "");
                    state.editor_cursor = lo;
                } else {
                    let char_count = state.editor_text.chars().count();
                    if state.editor_cursor < char_count {
                        let byte_pos = state.editor_text.char_indices().nth(state.editor_cursor).map(|(i, _)| i).unwrap_or(0);
                        let next = state.editor_text.char_indices().nth(state.editor_cursor + 1).map(|(i, _)| i).unwrap_or(state.editor_text.len());
                        state.editor_text.replace_range(byte_pos..next, "");
                    }
                }
            }
            DemoMessage::EditorLeft => {
                state.editor_selection = None;
                if state.editor_cursor > 0 { state.editor_cursor -= 1; }
            }
            DemoMessage::EditorRight => {
                state.editor_selection = None;
                let len = state.editor_text.chars().count();
                if state.editor_cursor < len { state.editor_cursor += 1; }
            }
            DemoMessage::EditorHome => {
                state.editor_selection = None;
                // Move to start of current line
                let (line, _col) = editor_line_col(&state.editor_text, state.editor_cursor);
                let mut offset = 0;
                for (i, ch) in state.editor_text.chars().enumerate() {
                    if i == state.editor_cursor { break; }
                    if ch == '\n' { offset = i + 1; }
                }
                let _ = line; // used indirectly
                state.editor_cursor = offset;
            }
            DemoMessage::EditorEnd => {
                state.editor_selection = None;
                // Move to end of current line
                let mut pos = state.editor_cursor;
                for ch in state.editor_text.chars().skip(state.editor_cursor) {
                    if ch == '\n' { break; }
                    pos += 1;
                }
                state.editor_cursor = pos;
            }
            DemoMessage::EditorUp => {
                state.editor_selection = None;
                let (line, col) = editor_line_col(&state.editor_text, state.editor_cursor);
                if line > 0 {
                    state.editor_cursor = editor_line_col_to_offset(&state.editor_text, line - 1, col);
                }
            }
            DemoMessage::EditorDown => {
                state.editor_selection = None;
                let (line, col) = editor_line_col(&state.editor_text, state.editor_cursor);
                let line_count = state.editor_text.split('\n').count();
                if line + 1 < line_count {
                    state.editor_cursor = editor_line_col_to_offset(&state.editor_text, line + 1, col);
                }
            }
            DemoMessage::EditorScroll(delta) => {
                state.editor_scroll_offset = (state.editor_scroll_offset - delta).max(0.0);
                let line_count = state.editor_text.split('\n').count() as f32;
                let max_scroll = (line_count * 18.0 - 80.0).max(0.0); // LINE_HEIGHT = 18
                state.editor_scroll_offset = state.editor_scroll_offset.min(max_scroll);
            }
            DemoMessage::EditorClickAt(rel_x, rel_y) => {
                state.focused_input = Some(state.editor_id);
                let line = ((rel_y + state.editor_scroll_offset) / LINE_HEIGHT).floor().max(0.0) as usize;
                let col = (rel_x / CHAR_WIDTH).round().max(0.0) as usize;
                state.editor_cursor = editor_line_col_to_offset(&state.editor_text, line, col);
                state.editor_selection = None;
            }
            DemoMessage::EditorDragTo(rel_x, rel_y) => {
                let line = ((rel_y + state.editor_scroll_offset) / LINE_HEIGHT).floor().max(0.0) as usize;
                let col = (rel_x / CHAR_WIDTH).round().max(0.0) as usize;
                let pos = editor_line_col_to_offset(&state.editor_text, line, col);
                let anchor = state.editor_selection
                    .map(|(a, _)| a)
                    .unwrap_or(state.editor_cursor);
                state.editor_selection = Some((anchor, pos));
                state.editor_cursor = pos;
            }
            DemoMessage::EditorSelectAll => {
                state.editor_selection = Some((0, state.editor_text.chars().count()));
                state.editor_cursor = state.editor_text.chars().count();
            }
        }
        Command::none()
    }

    fn view(state: &Self::State, snapshot: &mut LayoutSnapshot) {
        // FPS calculation (exponential moving average)
        let now = Instant::now();
        let dt = now.duration_since(state.last_frame.get()).as_secs_f32();
        state.last_frame.set(now);
        let instant_fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
        let prev = state.fps_smooth.get();
        // Smoothing: 95% old + 5% new (avoids flicker)
        let fps = if prev == 0.0 { instant_fps } else { prev * 0.95 + instant_fps * 0.05 };
        state.fps_smooth.set(fps);

        // Cursor blink: 500ms on / 500ms off, reset on edit
        let blink_elapsed = now.duration_since(state.last_edit_time).as_millis();
        let cursor_visible = (blink_elapsed / 500) % 2 == 0;

        // Dynamic viewport — reflows on window resize
        let vp = snapshot.viewport();
        let vw = vp.width;
        let vh = vp.height;

        let outer_padding = 16.0;
        let col_spacing = 20.0;

        // Right column: 30% of viewport, clamped to reasonable range
        let right_col_width = (vw * 0.3).clamp(300.0, 420.0);

        // =================================================================
        // MAIN LAYOUT: Row with two columns
        // =================================================================
        Row::new()
            .padding(outer_padding)
            .spacing(col_spacing)
            .width(Length::Fixed(vw))
            .height(Length::Fixed(vh))
            // =============================================================
            // LEFT COLUMN: Scrollable Nexus App Mockup
            // =============================================================
            .scroll_column(
                ScrollColumn::new(state.scroll_id, state.scroll_thumb_id)
                    .scroll_offset(state.scroll_offset)
                    .spacing(16.0)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    // Test image (loaded from demo.png via ImageStore)
                    .image(
                        crate::strata::layout::containers::ImageElement::new(
                            state.test_image,
                            336.0,
                            296.0,
                        )
                        .corner_radius(8.0),
                    )
                    // Shell Block
                    .column(
                        ShellBlock {
                            cmd: "ls -la",
                            status_icon: "\u{2713}",
                            status_color: colors::SUCCESS,
                            terminal_source: state.terminal_source,
                            rows: vec![
                                ("total 32", Color::rgb(0.7, 0.7, 0.7)),
                                (
                                    "drwxr-xr-x  8 kevin staff  256 Jan 29 src/",
                                    Color::rgb(0.4, 0.6, 1.0),
                                ),
                                (
                                    "-rw-r--r--  1 kevin staff  420 Jan 29 main.rs",
                                    Color::rgb(0.7, 0.7, 0.7),
                                ),
                                (
                                    "-rw-r--r--  1 kevin staff 1247 Jan 29 lib.rs",
                                    Color::rgb(0.7, 0.7, 0.7),
                                ),
                                (
                                    "-rw-r--r--  1 kevin staff  890 Jan 29 Cargo.toml",
                                    Color::rgb(0.7, 0.7, 0.7),
                                ),
                            ],
                            cols: 75,
                            row_count: 5,
                        }
                        .build(),
                    )
                    // Agent Block
                    .column(
                        AgentBlock {
                            query: "How do I parse JSON in Rust?",
                            query_source: state.query_source,
                            tools: vec![
                                ToolInvocation {
                                    icon: "\u{25B6}",
                                    status_icon: "\u{2713}",
                                    label: "Read src/parser.rs",
                                    color: colors::SUCCESS,
                                    expanded: false,
                                    output_source: None,
                                    output_rows: vec![],
                                    output_cols: 65,
                                },
                                ToolInvocation {
                                    icon: "\u{25BC}",
                                    status_icon: "\u{25CF}",
                                    label: "Running bash",
                                    color: colors::RUNNING,
                                    expanded: true,
                                    output_source: Some(state.tool_output_source),
                                    output_rows: vec![
                                        ("  $ cargo test", Color::rgb(0.6, 0.6, 0.6)),
                                        (
                                            "  running 3 tests... ok",
                                            Color::rgb(0.6, 0.6, 0.6),
                                        ),
                                    ],
                                    output_cols: 65,
                                },
                            ],
                            response_lines: vec![
                                "You can parse JSON in Rust using the serde_json crate.",
                                "Add serde_json = \"1.0\" to your Cargo.toml, then use",
                                "serde_json::from_str() to deserialize a JSON string",
                                "into any type that implements Deserialize.",
                            ],
                            response_source: state.response_source,
                            status_text: "\u{2713} Completed \u{00B7} 2.3s",
                            status_color: colors::SUCCESS,
                        }
                        .build(),
                    )
                    // Permission Dialog
                    .column(PermissionDialog {
                        command: "rm -rf /tmp/cache",
                        deny_id: state.deny_btn_id,
                        allow_id: state.allow_btn_id,
                        always_id: state.always_btn_id,
                    }.build())
                    // Input Bar (with real TextInput)
                    .row(
                        Row::new()
                            .padding_custom(crate::strata::layout::containers::Padding::new(8.0, 12.0, 8.0, 12.0))
                            .spacing(10.0)
                            .background(colors::BG_INPUT)
                            .corner_radius(6.0)
                            .border(colors::BORDER_INPUT, 1.0)
                            .width(Length::Fill)
                            .cross_align(crate::strata::layout::containers::CrossAxisAlignment::Center)
                            .text(TextElement::new("~/Desktop/nexus").color(colors::TEXT_PATH))
                            .column(
                                Column::new()
                                    .padding_custom(crate::strata::layout::containers::Padding::new(1.0, 8.0, 1.0, 8.0))
                                    .background(colors::BTN_MODE_SH)
                                    .corner_radius(3.0)
                                    .text(TextElement::new("SH").color(colors::SUCCESS).size(12.0)),
                            )
                            .text(TextElement::new("$").color(colors::SUCCESS))
                            .text_input(
                                TextInputElement::new(
                                    state.input_id,
                                    &state.input_text,
                                )
                                .cursor(state.input_cursor)
                                .selection(state.input_selection)
                                .focused(state.focused_input == Some(state.input_id))
                                .placeholder("Type a command...")
                                .background(Color::rgba(0.0, 0.0, 0.0, 0.0))
                                .border_color(Color::rgba(0.0, 0.0, 0.0, 0.0))
                                .focus_border_color(Color::rgba(0.0, 0.0, 0.0, 0.0))
                                .corner_radius(0.0)
                                .padding(crate::strata::layout::containers::Padding::new(0.0, 4.0, 0.0, 4.0))
                                .width(Length::Fill)
                                .cursor_visible(cursor_visible),
                            ),
                    )
                    ,
            )
            // =============================================================
            // RIGHT COLUMN: Scrollable Component Catalog
            // =============================================================
            .scroll_column({
                let arrow = if state.table_sort_asc { " \u{25B2}" } else { " \u{25BC}" };
                let name_header: String = if state.table_sort_col == 0 { format!("NAME{}", arrow) } else { "NAME".into() };
                let size_header: String = if state.table_sort_col == 1 { format!("SIZE{}", arrow) } else { "SIZE".into() };

                let mut table = TableElement::new(state.table_source)
                    .column_sortable(&name_header, 140.0, state.sort_name_btn)
                    .column_sortable(&size_header, 70.0, state.sort_size_btn)
                    .column("TYPE", 70.0);

                for &(name, size_str, _size_bytes, kind, name_color) in &state.table_rows {
                    table = table.row(vec![
                        TableCell { text: name.into(), color: name_color },
                        TableCell { text: size_str.into(), color: colors::TEXT_SECONDARY },
                        TableCell { text: kind.into(), color: colors::TEXT_MUTED },
                    ]);
                }

                ScrollColumn::new(state.right_scroll_id, state.right_scroll_thumb_id)
                    .scroll_offset(state.right_scroll_offset)
                    .spacing(16.0)
                    .width(Length::Fixed(right_col_width))
                    .height(Length::Fill)
                    // Status Indicators
                    .column(
                        StatusPanel {
                            indicators: vec![
                                StatusIndicator {
                                    icon: "\u{25CF}",
                                    label: "Running",
                                    color: colors::RUNNING,
                                },
                                StatusIndicator {
                                    icon: "\u{2713}",
                                    label: "Success",
                                    color: colors::SUCCESS,
                                },
                                StatusIndicator {
                                    icon: "\u{2717}",
                                    label: "Error",
                                    color: colors::ERROR,
                                },
                                StatusIndicator {
                                    icon: "\u{2620}",
                                    label: "Killed",
                                    color: colors::KILLED,
                                },
                            ],
                            uptime_seconds: state.elapsed_seconds,
                        }
                        .build()
                        .id(state.status_panel_id),
                    )
                    // Job Pills
                    .column(
                        JobPanel {
                            jobs: vec![
                                JobPill {
                                    name: "vim",
                                    prefix: "\u{25CF} ",
                                    text_color: colors::SUCCESS,
                                    bg_color: Color::rgba(0.15, 0.35, 0.18, 0.8),
                                },
                                JobPill {
                                    name: "top",
                                    prefix: "\u{23F8} ",
                                    text_color: colors::PENDING,
                                    bg_color: Color::rgba(0.35, 0.30, 0.10, 0.8),
                                },
                                JobPill {
                                    name: "cargo",
                                    prefix: "\u{25CF} ",
                                    text_color: colors::RUNNING,
                                    bg_color: Color::rgba(0.15, 0.25, 0.40, 0.8),
                                },
                            ],
                        }
                        .build()
                        .id(state.job_panel_id),
                    )
                    // Multi-line Editor
                    .column(
                        Column::new()
                            .padding(10.0)
                            .spacing(6.0)
                            .background(colors::BG_BLOCK)
                            .corner_radius(6.0)
                            .width(Length::Fill)
                            .text(TextElement::new("Multi-line Editor").color(colors::TEXT_SECONDARY))
                            .text_input(
                                TextInputElement::new(state.editor_id, &state.editor_text)
                                    .multiline(true)
                                    .height(Length::Fixed(120.0))
                                    .cursor(state.editor_cursor)
                                    .selection(state.editor_selection)
                                    .focused(state.focused_input == Some(state.editor_id))
                                    .placeholder("Multi-line editor...")
                                    .scroll_offset(state.editor_scroll_offset)
                                    .cursor_visible(cursor_visible),
                            )
                            .id(state.editor_panel_id),
                    )
                    // Context menu placeholder (fixed height, rendered as primitives after layout)
                    .column(
                        Column::new()
                            .width(Length::Fill)
                            .height(Length::Fixed(194.0)) // "Context Menu" label + menu box
                            .id(state.context_menu_placeholder_id),
                    )
                    // Drawing styles placeholder
                    .column(
                        Column::new()
                            .width(Length::Fill)
                            .height(Length::Fixed(180.0))
                            .id(state.drawing_styles_placeholder_id),
                    )
                    // Table (fully layout-driven)
                    .column(
                        Column::new()
                            .padding(10.0)
                            .spacing(6.0)
                            .background(colors::BG_BLOCK)
                            .corner_radius(6.0)
                            .width(Length::Fill)
                            .text(TextElement::new("Table").color(colors::TEXT_SECONDARY))
                            .table(table),
                    )
            })
            .layout(snapshot, Rect::new(0.0, 0.0, vw, vh));

        // Update scroll limits from layout
        if let Some(max) = snapshot.scroll_limit(&state.scroll_id) {
            state.scroll_max.set(max);
        }
        if let Some(track) = snapshot.scroll_track(&state.scroll_id) {
            state.scroll_track.set(Some(*track));
        }
        if let Some(max) = snapshot.scroll_limit(&state.right_scroll_id) {
            state.right_scroll_max.set(max);
        }
        if let Some(track) = snapshot.scroll_track(&state.right_scroll_id) {
            state.right_scroll_track.set(Some(*track));
        }
        if let Some(bounds) = snapshot.widget_bounds(&state.right_scroll_id) {
            state.right_scroll_bounds.set(bounds);
        }
        // Save input/editor widget bounds for mouse hit → relative position
        if let Some(bounds) = snapshot.widget_bounds(&state.input_id) {
            state.input_bounds.set(bounds);
        }
        if let Some(bounds) = snapshot.widget_bounds(&state.editor_id) {
            state.editor_bounds.set(bounds);
        }

        // =================================================================
        // POST-LAYOUT: Render primitives into placeholder positions
        // =================================================================
        let anim_t = now.duration_since(state.start_time).as_secs_f32();

        // Render context menu at its placeholder position
        if let Some(bounds) = snapshot.widget_bounds(&state.context_menu_placeholder_id) {
            view_context_menu(snapshot, bounds.x, bounds.y);
        }

        // Render drawing styles at placeholder position
        if let Some(bounds) = snapshot.widget_bounds(&state.drawing_styles_placeholder_id) {
            view_drawing_styles(snapshot, bounds.x, bounds.y, bounds.width, anim_t);
        }

        // FPS counter (top-right corner)
        let fps_text = format!("{:.0} FPS", fps);
        snapshot.primitives_mut().add_text(
            fps_text,
            Point::new(vw - 70.0, 4.0),
            colors::TEXT_MUTED,
        );
    }

    fn selection(state: &Self::State) -> Option<&Selection> {
        state.selection.as_ref()
    }

    fn on_mouse(
        state: &Self::State,
        event: MouseEvent,
        hit: Option<HitResult>,
        capture: &CaptureState,
    ) -> MouseResponse<Self::Message> {
        use crate::strata::event_context::ScrollDelta;

        match event {
            MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                position,
            } => {
                if let Some(HitResult::Widget(id)) = &hit {
                    // Text input click: focus + position cursor
                    if *id == state.input_id {
                        let bounds = state.input_bounds.get();
                        let padding_left = 6.0; // text padding inside input
                        let rel_x = (position.x - bounds.x - padding_left).max(0.0);
                        return MouseResponse::message_and_capture(
                            DemoMessage::InputClickAt(state.input_id, rel_x),
                            state.input_id,
                        );
                    }
                    // Multi-line editor click: focus + position cursor
                    if *id == state.editor_id {
                        let bounds = state.editor_bounds.get();
                        let padding = 6.0;
                        let rel_x = (position.x - bounds.x - padding).max(0.0);
                        let rel_y = (position.y - bounds.y - padding).max(0.0);
                        return MouseResponse::message_and_capture(
                            DemoMessage::EditorClickAt(rel_x, rel_y),
                            state.editor_id,
                        );
                    }
                    // Button clicks
                    if *id == state.deny_btn_id || *id == state.allow_btn_id || *id == state.always_btn_id {
                        return MouseResponse::message(DemoMessage::ButtonClicked(*id));
                    }
                    // Table sort header clicks
                    if *id == state.sort_name_btn || *id == state.sort_size_btn {
                        return MouseResponse::message(DemoMessage::TableSort(*id));
                    }
                    // Scrollbar thumb drag (left panel)
                    if *id == state.scroll_thumb_id {
                        return MouseResponse::message_and_capture(
                            DemoMessage::ScrollDragStart(position.y),
                            state.scroll_thumb_id,
                        );
                    }
                    // Scrollbar thumb drag (right panel)
                    if *id == state.right_scroll_thumb_id {
                        return MouseResponse::message_and_capture(
                            DemoMessage::RightScrollDragStart(position.y),
                            state.right_scroll_thumb_id,
                        );
                    }
                }
                // Text / grid cell selection
                if let Some(HitResult::Content(addr)) = hit {
                    if state.focused_input.is_some() {
                        return MouseResponse::message(DemoMessage::InputBlur);
                    }
                    let capture_source = addr.source_id;
                    return MouseResponse::message_and_capture(
                        DemoMessage::SelectionStart(addr),
                        capture_source,
                    );
                }
                // Clicked on empty space: blur input
                if state.focused_input.is_some() {
                    return MouseResponse::message(DemoMessage::InputBlur);
                }
            }
            MouseEvent::CursorMoved { position } => {
                if let CaptureState::Captured(id) = capture {
                    // Scrollbar thumb drag (left)
                    if *id == state.scroll_thumb_id {
                        return MouseResponse::message(DemoMessage::ScrollDragMove(position.y));
                    }
                    // Scrollbar thumb drag (right)
                    if *id == state.right_scroll_thumb_id {
                        return MouseResponse::message(DemoMessage::RightScrollDragMove(position.y));
                    }
                    // Input drag selection
                    if *id == state.input_id {
                        let bounds = state.input_bounds.get();
                        let padding_left = 6.0;
                        let rel_x = (position.x - bounds.x - padding_left).max(0.0);
                        return MouseResponse::message(DemoMessage::InputDragTo(rel_x));
                    }
                    // Editor drag selection
                    if *id == state.editor_id {
                        let bounds = state.editor_bounds.get();
                        let padding = 6.0;
                        let rel_x = (position.x - bounds.x - padding).max(0.0);
                        let rel_y = (position.y - bounds.y - padding).max(0.0);
                        return MouseResponse::message(DemoMessage::EditorDragTo(rel_x, rel_y));
                    }
                    // Text/grid selection
                    if let Some(HitResult::Content(addr)) = hit {
                        return MouseResponse::message(DemoMessage::SelectionExtend(addr));
                    }
                }
            }
            MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            } => {
                if let CaptureState::Captured(id) = capture {
                    if *id == state.scroll_thumb_id {
                        return MouseResponse::message_and_release(DemoMessage::ScrollDragEnd);
                    }
                    if *id == state.right_scroll_thumb_id {
                        return MouseResponse::message_and_release(DemoMessage::RightScrollDragEnd);
                    }
                    // Input/editor: just release capture (selection already set via drag)
                    if *id == state.input_id || *id == state.editor_id {
                        return MouseResponse::release();
                    }
                    return MouseResponse::message_and_release(DemoMessage::SelectionEnd);
                }
            }
            MouseEvent::WheelScrolled { delta, position } => {
                let dy = match delta {
                    ScrollDelta::Lines { y, .. } => y * 40.0,
                    ScrollDelta::Pixels { y, .. } => y,
                };
                // Route based on cursor position over scroll containers
                let right_bounds = state.right_scroll_bounds.get();
                if right_bounds.contains_xy(position.x, position.y) {
                    return MouseResponse::message(DemoMessage::RightScroll(dy));
                }
                // Default: left panel scroll
                if hit.is_some() {
                    return MouseResponse::message(DemoMessage::Scroll(dy));
                }
            }
            _ => {}
        }
        MouseResponse::none()
    }

    fn on_key(
        state: &Self::State,
        event: KeyEvent,
    ) -> Option<Self::Message> {
        if let KeyEvent::Pressed { key, modifiers } = event {
            // Route to focused input first
            if state.focused_input == Some(state.editor_id) {
                // Multi-line editor: Enter inserts newline, Up/Down navigate lines
                match (&key, modifiers.shift, modifiers.meta || modifiers.ctrl) {
                    (Key::Named(NamedKey::Escape), _, _) => return Some(DemoMessage::InputBlur),
                    (Key::Named(NamedKey::Enter), _, _) => return Some(DemoMessage::EditorEnter),
                    (Key::Named(NamedKey::Backspace), _, _) => return Some(DemoMessage::EditorBackspace),
                    (Key::Named(NamedKey::Delete), _, _) => return Some(DemoMessage::EditorDelete),
                    (Key::Named(NamedKey::ArrowLeft), _, _) => return Some(DemoMessage::EditorLeft),
                    (Key::Named(NamedKey::ArrowRight), _, _) => return Some(DemoMessage::EditorRight),
                    (Key::Named(NamedKey::ArrowUp), _, _) => return Some(DemoMessage::EditorUp),
                    (Key::Named(NamedKey::ArrowDown), _, _) => return Some(DemoMessage::EditorDown),
                    (Key::Named(NamedKey::Home), _, _) => return Some(DemoMessage::EditorHome),
                    (Key::Named(NamedKey::End), _, _) => return Some(DemoMessage::EditorEnd),
                    (Key::Character(c), _, true) if c == "a" => return Some(DemoMessage::EditorSelectAll),
                    (Key::Character(c), _, false) => return Some(DemoMessage::EditorChar(c.clone())),
                    (Key::Named(NamedKey::Space), _, false) => return Some(DemoMessage::EditorChar(" ".into())),
                    _ => return None,
                }
            } else if state.focused_input.is_some() {
                // Single-line input
                match (&key, modifiers.shift, modifiers.meta || modifiers.ctrl) {
                    (Key::Named(NamedKey::Escape), _, _) => return Some(DemoMessage::InputBlur),
                    (Key::Named(NamedKey::Enter), _, _) => return Some(DemoMessage::InputSubmit),
                    (Key::Named(NamedKey::Backspace), _, _) => return Some(DemoMessage::InputBackspace),
                    (Key::Named(NamedKey::Delete), _, _) => return Some(DemoMessage::InputDelete),
                    (Key::Named(NamedKey::ArrowLeft), true, _) => return Some(DemoMessage::InputSelectLeft),
                    (Key::Named(NamedKey::ArrowRight), true, _) => return Some(DemoMessage::InputSelectRight),
                    (Key::Named(NamedKey::ArrowLeft), _, _) => return Some(DemoMessage::InputLeft),
                    (Key::Named(NamedKey::ArrowRight), _, _) => return Some(DemoMessage::InputRight),
                    (Key::Named(NamedKey::Home), _, _) => return Some(DemoMessage::InputHome),
                    (Key::Named(NamedKey::End), _, _) => return Some(DemoMessage::InputEnd),
                    (Key::Character(c), _, true) if c == "a" => return Some(DemoMessage::InputSelectAll),
                    (Key::Character(c), _, false) => return Some(DemoMessage::InputChar(c.clone())),
                    (Key::Named(NamedKey::Space), _, false) => return Some(DemoMessage::InputChar(" ".into())),
                    _ => return None, // Swallow unhandled keys when focused
                }
            }
            // Global shortcuts
            match (&key, modifiers.meta) {
                (Key::Character(c), true) if c == "c" => return Some(DemoMessage::Copy),
                (Key::Named(NamedKey::Escape), _) => return Some(DemoMessage::ClearSelection),
                (Key::Named(NamedKey::ArrowUp), _) => return Some(DemoMessage::ScrollByKey(60.0)),
                (Key::Named(NamedKey::ArrowDown), _) => return Some(DemoMessage::ScrollByKey(-60.0)),
                (Key::Named(NamedKey::PageUp), _) => return Some(DemoMessage::ScrollByKey(300.0)),
                (Key::Named(NamedKey::PageDown), _) => return Some(DemoMessage::ScrollByKey(-300.0)),
                _ => {}
            }
        }
        None
    }

    fn subscription(_state: &Self::State) -> Subscription<Self::Message> {
        Subscription::from_iced(
            iced::time::every(std::time::Duration::from_secs(1))
                .map(|_| DemoMessage::TimerTick),
        )
    }

    fn title(_state: &Self::State) -> String {
        String::from("Strata — Nexus Widget Demo")
    }
}

// =========================================================================
// Overlay: Context Menu (absolute positioned)
// =========================================================================

fn view_context_menu(snapshot: &mut LayoutSnapshot, x: f32, y: f32) {
    let w = 180.0;
    let h = 150.0;

    let p = snapshot.primitives_mut();

    p.add_text("Context Menu", Point::new(x, y), colors::TEXT_SECONDARY);

    let my = y + 22.0;

    p.add_shadow(
        Rect::new(x + 4.0, my + 4.0, w, h),
        8.0, 12.0,
        Color::rgba(0.0, 0.0, 0.0, 0.5),
    );
    p.add_rounded_rect(Rect::new(x, my, w, h), 8.0, colors::BG_OVERLAY);
    p.add_border(Rect::new(x, my, w, h), 8.0, 1.0, colors::BORDER_SUBTLE);

    let ix = x + 8.0;
    let iw = w - 16.0;
    let row_h = 26.0;
    let sep_gap = 8.0;

    // Copy (hover)
    let iy = my + 4.0;
    p.add_rounded_rect(Rect::new(ix, iy, iw, row_h - 2.0), 4.0, colors::BG_HOVER);
    p.add_text("Copy", Point::new(ix + 8.0, iy + 4.0), Color::WHITE);
    p.add_text("\u{2318}C", Point::new(ix + iw - 30.0, iy + 4.0), colors::TEXT_MUTED);

    // Paste
    let iy = iy + row_h;
    p.add_text("Paste", Point::new(ix + 8.0, iy + 4.0), colors::TEXT_PRIMARY);
    p.add_text("\u{2318}V", Point::new(ix + iw - 30.0, iy + 4.0), colors::TEXT_MUTED);

    // Separator
    let iy = iy + row_h;
    p.add_line(
        Point::new(ix, iy + sep_gap * 0.5),
        Point::new(ix + iw, iy + sep_gap * 0.5),
        1.0, Color::rgba(1.0, 1.0, 1.0, 0.08),
    );

    // Select All
    let iy = iy + sep_gap;
    p.add_text("Select All", Point::new(ix + 8.0, iy + 4.0), colors::TEXT_PRIMARY);
    p.add_text("\u{2318}A", Point::new(ix + iw - 30.0, iy + 4.0), colors::TEXT_MUTED);

    // Clear Selection (disabled)
    let iy = iy + row_h;
    p.add_text("Clear Selection", Point::new(ix + 8.0, iy + 4.0), colors::TEXT_MUTED);

    // Separator
    let iy = iy + row_h;
    p.add_line(
        Point::new(ix, iy + sep_gap * 0.5),
        Point::new(ix + iw, iy + sep_gap * 0.5),
        1.0, Color::rgba(1.0, 1.0, 1.0, 0.08),
    );

    // Search
    let iy = iy + sep_gap;
    p.add_text("Search", Point::new(ix + 8.0, iy + 4.0), colors::TEXT_PRIMARY);
    p.add_text("\u{2318}F", Point::new(ix + iw - 30.0, iy + 4.0), colors::TEXT_MUTED);
}

// =========================================================================
// Overlay: Drawing Styles (lines, curves, polylines)
// =========================================================================

fn view_drawing_styles(snapshot: &mut LayoutSnapshot, x: f32, y: f32, width: f32, time: f32) {
    let p = snapshot.primitives_mut();

    p.add_rounded_rect(Rect::new(x, y, width, 180.0), 6.0, colors::BG_BLOCK);
    p.add_text("Drawing Styles", Point::new(x + 10.0, y + 6.0), colors::TEXT_SECONDARY);

    let lx = x + 14.0;
    let lw = width - 28.0;

    // --- Solid lines (various thickness) ---
    let ly = y + 32.0;
    p.add_text("Solid", Point::new(lx, ly), colors::TEXT_MUTED);
    p.add_line(Point::new(lx + 50.0, ly + 9.0), Point::new(lx + lw * 0.5, ly + 9.0), 1.0, colors::RUNNING);
    p.add_line(Point::new(lx + lw * 0.5 + 8.0, ly + 9.0), Point::new(lx + lw, ly + 9.0), 2.0, colors::SUCCESS);

    // --- Dashed lines ---
    let ly = ly + 24.0;
    p.add_text("Dashed", Point::new(lx, ly), colors::TEXT_MUTED);
    p.add_line_styled(
        Point::new(lx + 50.0, ly + 9.0), Point::new(lx + lw, ly + 9.0),
        1.5, colors::WARNING, LineStyle::Dashed,
    );

    // --- Dotted lines ---
    let ly = ly + 24.0;
    p.add_text("Dotted", Point::new(lx, ly), colors::TEXT_MUTED);
    p.add_line_styled(
        Point::new(lx + 50.0, ly + 9.0), Point::new(lx + lw, ly + 9.0),
        1.5, colors::ERROR, LineStyle::Dotted,
    );

    // --- Polyline (zigzag) ---
    let ly = ly + 24.0;
    p.add_text("Poly", Point::new(lx, ly), colors::TEXT_MUTED);
    let seg_w = (lw - 50.0) / 8.0;
    let zigzag: Vec<Point> = (0..9)
        .map(|i| {
            let px = lx + 50.0 + i as f32 * seg_w;
            let py = ly + if i % 2 == 0 { 14.0 } else { 2.0 };
            Point::new(px, py)
        })
        .collect();
    p.add_polyline(zigzag, 1.5, colors::TEXT_PURPLE);

    // --- Polyline (animated sine wave) ---
    let ly = ly + 28.0;
    p.add_text("Curve", Point::new(lx, ly), colors::TEXT_MUTED);
    let curve_w = lw - 50.0;
    let phase = time * 2.0; // scrolling phase
    let curve: Vec<Point> = (0..40)
        .map(|i| {
            let t = i as f32 / 39.0;
            let px = lx + 50.0 + t * curve_w;
            let py = ly + 8.0 - (t * std::f32::consts::PI * 2.0 + phase).sin() * 8.0;
            Point::new(px, py)
        })
        .collect();
    p.add_polyline(curve, 1.5, colors::RUNNING);

    // --- Dashed polyline (animated wave) ---
    let ly = ly + 28.0;
    p.add_text("Wave", Point::new(lx, ly), colors::TEXT_MUTED);
    let wave_phase = time * 3.0;
    let wave: Vec<Point> = (0..40)
        .map(|i| {
            let t = i as f32 / 39.0;
            let px = lx + 50.0 + t * curve_w;
            let py = ly + 8.0 - (t * std::f32::consts::PI * 3.0 + wave_phase).sin() * 6.0;
            Point::new(px, py)
        })
        .collect();
    p.add_polyline_styled(wave, 1.0, colors::SUCCESS, LineStyle::Dashed);
}

/// Run the demo application.
pub fn run() -> Result<(), crate::strata::shell::Error> {
    crate::strata::shell::run_with_config::<DemoApp>(AppConfig {
        title: String::from("Strata — Nexus Widget Demo"),
        window_size: (1050.0, 672.0),
        antialiasing: true,
        background_color: colors::BG_APP,
    })
}

/// Convert a char offset to (line, col) in newline-delimited text.
fn editor_line_col(text: &str, char_offset: usize) -> (usize, usize) {
    let mut line = 0;
    let mut col = 0;
    for (i, ch) in text.chars().enumerate() {
        if i == char_offset {
            return (line, col);
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Convert (line, col) back to a char offset, clamping col to line length.
fn editor_line_col_to_offset(text: &str, target_line: usize, target_col: usize) -> usize {
    let mut offset = 0;
    for (line_idx, line) in text.split('\n').enumerate() {
        if line_idx == target_line {
            let line_len = line.chars().count();
            return offset + target_col.min(line_len);
        }
        offset += line.chars().count() + 1; // +1 for '\n'
    }
    text.chars().count() // past end
}
