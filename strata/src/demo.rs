//! Demo application exercising all Strata rendering features needed for Nexus.
//!
//! Features demonstrated:
//! - **Async Commands**: Submit triggers a `Command::message` round-trip
//! - **Dynamic Lists**: Chat history grows at runtime with stable SourceIds
//! - **Centralized Focus**: Single `focused_id` governs all widget focus state
//! - **Stateful Buttons**: Hover highlight via `Cell<Option<SourceId>>` tracking
//! - **Zero-allocation Event Routing**: Composable `handle_mouse` + `map`
//!
//! Run with: `cargo run -p nexus-ui --example strata_demo`

use std::cell::Cell;
use std::time::Instant;

use crate::route_mouse;
use crate::content_address::{ContentAddress, SourceId};
use crate::demo_widgets::{Card, ShellBlock, StatusIndicator, StatusPanel};
use crate::event_context::{
    CaptureState, Key, KeyEvent, MouseButton, MouseEvent, NamedKey,
};
use crate::layout_snapshot::HitResult;
use crate::primitives::{Color, Point, Rect};
use crate::{
    AppConfig, ButtonElement, Column, Command, CrossAxisAlignment, ImageElement, ImageHandle,
    ImageStore, LayoutSnapshot, Length, LineStyle, MouseResponse, Padding, Row, ScrollAction,
    ScrollColumn, ScrollState, Selection, StrataApp, Subscription, TableCell, TableElement,
    TextElement, TextInputAction, TextInputElement, TextInputMouseAction, TextInputState,
};

// =========================================================================
// Nexus color palette (matches real app)
// =========================================================================
pub(crate) mod colors {
    use crate::primitives::Color;

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
    pub const KILLED: Color = Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };

    // Text
    pub const TEXT_PRIMARY: Color = Color { r: 0.85, g: 0.85, b: 0.88, a: 1.0 };
    pub const TEXT_SECONDARY: Color = Color { r: 0.55, g: 0.55, b: 0.60, a: 1.0 };
    pub const TEXT_MUTED: Color = Color { r: 0.40, g: 0.40, b: 0.45, a: 1.0 };
    pub const TEXT_PATH: Color = Color { r: 0.39, g: 0.58, b: 0.93, a: 1.0 };
    pub const TEXT_PURPLE: Color = Color { r: 0.6, g: 0.4, b: 0.9, a: 1.0 };
    pub const TEXT_QUERY: Color = Color { r: 0.5, g: 0.7, b: 1.0, a: 1.0 };

    // Buttons
    pub const BTN_DENY: Color = Color { r: 0.6, g: 0.15, b: 0.15, a: 1.0 };
    pub const BTN_ALLOW: Color = Color { r: 0.15, g: 0.5, b: 0.25, a: 1.0 };
    pub const BTN_ALWAYS: Color = Color { r: 0.1, g: 0.35, b: 0.18, a: 1.0 };
    pub const BTN_KILL: Color = Color { r: 0.6, g: 0.2, b: 0.2, a: 1.0 };
    // Borders
    pub const BORDER_SUBTLE: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.08 };
    pub const BORDER_INPUT: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.12 };

    // Cursor
    pub const CURSOR: Color = Color { r: 0.85, g: 0.85, b: 0.88, a: 0.8 };
}

// =========================================================================
// Dynamic chat item (proves Vec<T> in view)
// =========================================================================

struct ChatItem {
    /// Stable ID for this item — survives across frames.
    source: SourceId,
    role: &'static str,
    text: String,
}

// =========================================================================
// Message type
// =========================================================================

/// Demo message type.
#[derive(Debug, Clone)]
pub enum DemoMessage {
    // Content selection (cross-widget text selection)
    SelectionStart(ContentAddress),
    SelectionExtend(ContentAddress),
    SelectionEnd,
    ClearSelection,
    Copy,

    // Scroll panels
    LeftScroll(ScrollAction),
    RightScroll(ScrollAction),

    // Text input events (routed from on_key/on_mouse)
    InputKey(KeyEvent),
    InputMouse(TextInputMouseAction),
    EditorKey(KeyEvent),
    EditorMouse(TextInputMouseAction),

    // Dynamic list: submit command from input bar
    SubmitCommand,

    // Buttons
    ButtonClicked(SourceId),

    // Table
    TableSort(SourceId),

    // Timer subscription
    TimerTick,

    // Focus management
    BlurAll,
}

// =========================================================================
// Application state
// =========================================================================

/// Demo application state.
pub struct DemoState {
    // Scroll panels
    left_scroll: ScrollState,
    right_scroll: ScrollState,
    // Text inputs
    input: TextInputState,
    editor: TextInputState,

    // --- Focus Management (centralized) ---
    /// The currently focused widget, or None if nothing focused.
    focused_id: Option<SourceId>,

    // --- Dynamic List ---
    /// Chat history — grows at runtime when commands are submitted.
    chat_history: Vec<ChatItem>,

    // --- Hover Tracking (Cell for interior mutability in on_mouse/view) ---
    /// SourceId of the widget currently under the cursor.
    hovered_widget: Cell<Option<SourceId>>,

    // Content selection (cross-widget)
    selection: Option<Selection>,
    is_selecting: bool,
    // Cursor blink
    last_edit_time: Instant,
    // FPS tracking (Cell for interior mutability in view())
    last_frame: Cell<Instant>,
    fps_smooth: Cell<f32>,
    // Animation start time
    start_time: Instant,
    // Test image handle
    test_image: ImageHandle,
    // Subscription demo
    elapsed_seconds: u32,
    // Virtualization: content-space Y of the chat list within the scroll column.
    // Updated after layout each frame; used next frame to adjust scroll offset.
    chat_content_top: Cell<f32>,
    // Table state
    table_sort_col: usize,
    table_sort_asc: bool,
    table_rows: Vec<(&'static str, &'static str, u32, &'static str, Color)>,
}

impl DemoState {
    // ------------------------------------------------------------------
    // Centralized focus helpers
    // ------------------------------------------------------------------

    /// Focus a specific widget. Automatically blurs all others.
    fn focus(&mut self, id: SourceId) {
        self.focused_id = Some(id);
        // Propagate to widget states
        self.input.focused = self.input.id() == id;
        self.editor.focused = self.editor.id() == id;
    }

    /// Blur all widgets.
    fn blur_all(&mut self) {
        self.focused_id = None;
        self.input.focused = false;
        self.editor.focused = false;
    }
}

// =========================================================================
// Button helper — hover-aware, zero allocation
// =========================================================================

/// Create a ButtonElement with hover highlight.
///
/// Reads the hovered_widget Cell to determine if this button is hovered,
/// and adjusts the background color accordingly. No heap allocation —
/// just a conditional color tweak on the existing ButtonElement.
fn hover_button(id: SourceId, label: &str, bg: Color, hovered: Option<SourceId>) -> ButtonElement {
    let is_hovered = hovered == Some(id);
    let actual_bg = if is_hovered {
        // Lighten by blending towards white
        Color {
            r: (bg.r + 0.15).min(1.0),
            g: (bg.g + 0.15).min(1.0),
            b: (bg.b + 0.15).min(1.0),
            a: bg.a,
        }
    } else {
        bg
    };
    ButtonElement::new(id, label).background(actual_bg)
}

// =========================================================================
// Component: Chat Bubble (zero-cost view fragment)
// =========================================================================

/// Build a single chat item card as a Column.
///
/// Returns a concrete `Column` — no trait objects, no heap allocation.
/// The caller pushes this into the parent container.
fn chat_bubble(item: &ChatItem) -> Column {
    let (role_color, text_color) = if item.role == "user" {
        (colors::SUCCESS, colors::TEXT_PRIMARY)
    } else {
        (colors::RUNNING, colors::TEXT_SECONDARY)
    };

    let mut card = Column::new()
        .padding(10.0)
        .spacing(4.0)
        .background(colors::BG_BLOCK)
        .corner_radius(6.0)
        .width(Length::Fill)
        .push(TextElement::new(item.role).color(role_color).size(12.0));

    for line in item.text.lines() {
        card = card.push(
            TextElement::new(line)
                .source(item.source)
                .color(text_color),
        );
    }
    card
}

// =========================================================================
// Virtualized list helpers
// =========================================================================

/// Line height used by TextElement (must match containers.rs).
const LINE_HEIGHT: f32 = 18.0;

/// Estimate the laid-out height of a chat item card.
///
/// Must match the padding/spacing/font sizes used in the view() builder.
/// This is cheap: just counts newlines in the text, no allocation.
fn chat_item_height(item: &ChatItem) -> f32 {
    let padding = 20.0;            // 10 top + 10 bottom
    let role_height = 14.0;        // 12pt font, ~14px
    let inner_spacing = 4.0;       // Column spacing between role and body lines
    let line_count = item.text.lines().count().max(1) as f32;
    let body_height = line_count * LINE_HEIGHT;
    let body_spacing = (line_count - 1.0).max(0.0) * inner_spacing;
    padding + role_height + inner_spacing + body_height + body_spacing
}

/// Compute the visible range and spacer heights for a virtualized list.
///
/// Returns `(first_index, last_index, top_spacer_px, bottom_spacer_px)`.
/// The caller should lay out `items[first..last]` and insert fixed spacers
/// for the skipped regions to preserve total content height.
///
/// `content_above` is the estimated height of scroll column children that
/// appear before the chat list (image, shell block, etc.).
fn virtualize_chat(
    items: &[ChatItem],
    scroll_offset: f32,
    viewport_height: f32,
    spacing: f32,
) -> (usize, usize, f32, f32) {
    if items.is_empty() || viewport_height <= 0.0 {
        return (0, 0, 0.0, 0.0);
    }

    // Extra items rendered above/below viewport to prevent pop-in
    const OVERSCAN: usize = 3;

    // Compute all item heights upfront (cheap: just counts newlines)
    let heights: Vec<f32> = items.iter().map(|item| chat_item_height(item)).collect();

    let mut y = 0.0_f32;
    let mut first = items.len();
    let mut last = items.len();

    for (i, h) in heights.iter().enumerate() {
        let item_bottom = y + h;

        // First item whose bottom edge is past the scroll top
        if first == items.len() && item_bottom > scroll_offset {
            first = i.saturating_sub(OVERSCAN);
        }

        // First item whose top edge is past the scroll bottom
        if first != items.len() && y > scroll_offset + viewport_height {
            last = (i + OVERSCAN).min(items.len());
            break;
        }

        y += h + spacing;
    }

    let first = first.min(items.len());

    // Spacer heights from precomputed per-item heights
    let top_spacer: f32 = heights[..first].iter().sum::<f32>()
        + if first > 0 { first as f32 * spacing } else { 0.0 };
    let bottom_spacer: f32 = heights[last..].iter().sum::<f32>()
        + if last < items.len() { (items.len() - last) as f32 * spacing } else { 0.0 };

    (first, last, top_spacer, bottom_spacer)
}

/// Generate N demo chat items with realistic variety.
fn generate_demo_chat(count: usize) -> Vec<ChatItem> {
    let commands = [
        ("ls -la", "total 32\ndrwxr-xr-x  8 kevin staff  256 Jan 29 src/\n-rw-r--r--  1 kevin staff  420 Jan 29 main.rs"),
        ("cargo build", "   Compiling nexus-ui v0.1.0\n    Finished dev [unoptimized + debuginfo] target(s)"),
        ("git status", "On branch main\nnothing to commit, working tree clean"),
        ("cat README.md", "# Nexus\n\nA GPU-accelerated terminal emulator."),
        ("echo hello", "hello"),
        ("pwd", "/Users/kevin/Desktop/nexus"),
        ("wc -l src/*.rs", "  142 src/main.rs\n  867 src/lib.rs\n 1009 total"),
        ("date", "Thu Jan 30 09:45:00 PST 2026"),
        ("cargo test", "running 47 tests\ntest result: ok. 47 passed; 0 failed"),
        ("ps aux | head -5", "USER  PID %CPU %MEM   VSZ    RSS  TT  STAT STARTED  TIME COMMAND\nroot    1  0.0  0.1 34292   9280  ??  Ss   Jan29   0:42 /sbin/launchd"),
    ];

    let mut items = Vec::with_capacity(count);
    for i in 0..count {
        let (cmd, output) = commands[i % commands.len()];
        items.push(ChatItem {
            source: SourceId::new(),
            role: "user",
            text: format!("{cmd} # run {}", i / 2 + 1),
        });
        items.push(ChatItem {
            source: SourceId::new(),
            role: "system",
            text: output.into(),
        });
        if items.len() >= count {
            break;
        }
    }
    items.truncate(count);
    items
}

// =========================================================================
// StrataApp implementation
// =========================================================================

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

        let input = TextInputState::single_line("input");
        let input_id = input.id();

        let state = DemoState {
            left_scroll: ScrollState::new(),
            right_scroll: ScrollState::new(),
            input,
            editor: TextInputState::multi_line_with_text(
                "editor",
                "Hello, world!\nThis is a multi-line editor.\n\nTry typing here.",
            ),
            // Start with input focused
            focused_id: Some(input_id),
            // Seed with 200 chat items to demonstrate virtualized scrolling.
            // Only ~10-15 items are laid out per frame regardless of list size.
            chat_history: generate_demo_chat(200),
            chat_content_top: Cell::new(0.0),
            hovered_widget: Cell::new(None),
            selection: None,
            is_selecting: false,
            last_edit_time: Instant::now(),
            last_frame: Cell::new(Instant::now()),
            fps_smooth: Cell::new(0.0),
            start_time: Instant::now(),
            test_image,
            elapsed_seconds: 0,
            table_sort_col: 0,
            table_sort_asc: true,
            table_rows: vec![
                ("src/",       "256B", 256,  "dir",  colors::TEXT_PATH),
                ("main.rs",    "420B", 420,  "rust", colors::TEXT_PRIMARY),
                ("lib.rs",     "1.2K", 1200, "rust", colors::TEXT_PRIMARY),
                ("Cargo.toml", "890B", 890,  "toml", colors::TEXT_PRIMARY),
                ("README.md",  "2.4K", 2400, "md",   colors::TEXT_PRIMARY),
            ],
        };

        // Focus the input widget
        // (can't call focus() before state is created, so set focused directly)
        let mut state = state;
        state.input.focused = true;

        (state, Command::none())
    }

    fn update(state: &mut Self::State, message: Self::Message, _images: &mut ImageStore) -> Command<Self::Message> {
        // Reset cursor blink on any edit/cursor action
        match &message {
            DemoMessage::InputKey(_) | DemoMessage::InputMouse(_)
            | DemoMessage::EditorKey(_) | DemoMessage::EditorMouse(_) => {
                state.last_edit_time = Instant::now();
            }
            _ => {}
        }

        match message {
            // =============================================================
            // Dynamic list: command submission via async Command round-trip
            // =============================================================
            DemoMessage::SubmitCommand => {
                let text = state.input.text.trim().to_string();
                if !text.is_empty() {
                    // Append user message
                    state.chat_history.push(ChatItem {
                        source: SourceId::new(),
                        role: "user",
                        text: text.clone(),
                    });
                    // Append mock system response
                    state.chat_history.push(ChatItem {
                        source: SourceId::new(),
                        role: "system",
                        text: format!("$ {text}\ncommand not found: {text}"),
                    });
                    // Clear input
                    state.input.text.clear();
                    state.input.cursor = 0;
                    state.input.selection = None;
                    // Auto-scroll to bottom
                    state.left_scroll.offset = state.left_scroll.max.get();
                }
            }

            // =============================================================
            // Text inputs — key events routed from on_key
            // =============================================================
            DemoMessage::InputKey(event) => {
                match state.input.handle_key(&event, false) {
                    TextInputAction::Submit(_) => {
                        // Prove the Command pipeline: round-trip through Command::message
                        return Command::message(DemoMessage::SubmitCommand);
                    }
                    _ => {}
                }
            }
            DemoMessage::EditorKey(event) => {
                state.editor.handle_key(&event, true);
            }

            // =============================================================
            // Text inputs — mouse events (focus managed centrally)
            // =============================================================
            DemoMessage::InputMouse(action) => {
                state.focus(state.input.id());
                state.input.apply_mouse(action);
            }
            DemoMessage::EditorMouse(action) => {
                state.focus(state.editor.id());
                state.editor.apply_mouse(action);
            }
            DemoMessage::BlurAll => {
                state.blur_all();
            }

            // =============================================================
            // Content selection
            // =============================================================
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
            DemoMessage::ClearSelection => {
                state.selection = None;
                state.is_selecting = false;
            }
            DemoMessage::Copy => {
                if let Some(sel) = &state.selection {
                    eprintln!(
                        "[demo] Copy: source={:?} offsets={}..{}",
                        sel.anchor.source_id,
                        sel.anchor.content_offset,
                        sel.focus.content_offset,
                    );
                    // In production: return Command::perform(clipboard_write_future)
                }
            }

            // =============================================================
            // Scroll panels
            // =============================================================
            DemoMessage::LeftScroll(action) => state.left_scroll.apply(action),
            DemoMessage::RightScroll(action) => state.right_scroll.apply(action),

            // =============================================================
            // Buttons
            // =============================================================
            DemoMessage::ButtonClicked(id) => {
                if id == SourceId::named("clear_btn") {
                    state.chat_history.clear();
                    eprintln!("[demo] Chat cleared");
                } else {
                    eprintln!("[demo] Button clicked: {:?}", id);
                }
            }

            // =============================================================
            // Table sorting
            // =============================================================
            DemoMessage::TableSort(id) => {
                let col = if id == SourceId::named("sort_name") { 0 } else { 1 };
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

            // =============================================================
            // Timer
            // =============================================================
            DemoMessage::TimerTick => {
                state.elapsed_seconds += 1;
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
        let fps = if prev == 0.0 { instant_fps } else { prev * 0.95 + instant_fps * 0.05 };
        state.fps_smooth.set(fps);

        // Cursor blink: 500ms on / 500ms off, reset on edit
        let blink_elapsed = now.duration_since(state.last_edit_time).as_millis();
        let cursor_visible = (blink_elapsed / 500) % 2 == 0;

        // Dynamic viewport — reflows on window resize
        let vp = snapshot.viewport();
        let vw = vp.width;
        let vh = vp.height;

        // Right column: 30% of viewport, clamped
        let right_col_width = (vw * 0.3).clamp(300.0, 420.0);

        // Read hover state for button rendering
        let hovered = state.hovered_widget.get();

        // =================================================================
        // BUILD VIRTUALIZED CHAT LIST
        // =================================================================
        // Only lay out items visible in the scroll viewport. Items above
        // and below are replaced by fixed spacers to preserve total height
        // and correct scrollbar proportions. O(n) height scan, O(visible)
        // layout — scales to thousands of items.

        let chat_spacing = 8.0;

        // Use previous frame's measured position of the chat list within
        // the scroll column's content space. Avoids hardcoded magic numbers
        // that break when content above the chat list changes.
        let chat_scroll = (state.left_scroll.offset - state.chat_content_top.get()).max(0.0);

        // Compute visible range using scroll state from previous frame.
        let (first, last, top_spacer, bottom_spacer) = virtualize_chat(
            &state.chat_history,
            chat_scroll,
            state.left_scroll.bounds.get().height,
            chat_spacing,
        );

        let mut chat_col = Column::new()
            .spacing(chat_spacing)
            .width(Length::Fill)
            .id(SourceId::named("chat_list"));

        // Top spacer replaces items above the viewport
        if top_spacer > 0.0 {
            chat_col = chat_col.fixed_spacer(top_spacer);
        }

        // Only lay out visible items (uses chat_bubble component function)
        for item in &state.chat_history[first..last] {
            chat_col = chat_col.push(chat_bubble(item));
        }

        // Bottom spacer replaces items below the viewport
        if bottom_spacer > 0.0 {
            chat_col = chat_col.fixed_spacer(bottom_spacer);
        }

        // =================================================================
        // MAIN LAYOUT: Row with two columns
        // =================================================================
        Row::new()
            .padding(16.0)
            .spacing(20.0)
            .width(Length::Fixed(vw))
            .height(Length::Fixed(vh))
            // =============================================================
            // LEFT COLUMN: Chat History + Shell Block + Input Bar
            // =============================================================
            .push(
                ScrollColumn::from_state(&state.left_scroll)
                    .spacing(16.0)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    // Test image
                    .push(
                        ImageElement::new(state.test_image, 336.0, 296.0)
                            .corner_radius(8.0),
                    )
                    // Static shell block
                    .push(ShellBlock {
                        cmd: "ls -la",
                        status_icon: "\u{2713}",
                        status_color: colors::SUCCESS,
                        terminal_source: SourceId::named("terminal"),
                        rows: vec![
                            ("total 32", Color::rgb(0.7, 0.7, 0.7)),
                            ("drwxr-xr-x  8 kevin staff  256 Jan 29 src/", Color::rgb(0.4, 0.6, 1.0)),
                            ("-rw-r--r--  1 kevin staff  420 Jan 29 main.rs", Color::rgb(0.7, 0.7, 0.7)),
                        ],
                        cols: 75,
                        row_count: 3,
                    })
                    // Dynamic chat history
                    .push(chat_col)
                    // Input bar
                    .push(
                        Row::new()
                            .padding_custom(Padding::new(8.0, 12.0, 8.0, 12.0))
                            .spacing(10.0)
                            .background(colors::BG_INPUT)
                            .corner_radius(6.0)
                            .border(colors::BORDER_INPUT, 1.0)
                            .width(Length::Fill)
                            .cross_align(CrossAxisAlignment::Center)
                            .push(TextElement::new("$").color(colors::SUCCESS))
                            .push(
                                TextInputElement::from_state(&state.input)
                                    .placeholder("Type a command...")
                                    .background(Color::rgba(0.0, 0.0, 0.0, 0.0))
                                    .border_color(Color::rgba(0.0, 0.0, 0.0, 0.0))
                                    .focus_border_color(Color::rgba(0.0, 0.0, 0.0, 0.0))
                                    .corner_radius(0.0)
                                    .padding(Padding::new(0.0, 4.0, 0.0, 4.0))
                                    .width(Length::Fill)
                                    .cursor_visible(cursor_visible),
                            )
                            .push(hover_button(
                                SourceId::named("submit_btn"),
                                "Send",
                                colors::BTN_ALLOW,
                                hovered,
                            )),
                    ),
            )
            // =============================================================
            // RIGHT COLUMN: Component Catalog
            // =============================================================
            .push({
                let arrow = if state.table_sort_asc { " \u{25B2}" } else { " \u{25BC}" };
                let name_header: String = if state.table_sort_col == 0 { format!("NAME{}", arrow) } else { "NAME".into() };
                let size_header: String = if state.table_sort_col == 1 { format!("SIZE{}", arrow) } else { "SIZE".into() };

                let mut table = TableElement::new(SourceId::named("table"))
                    .column_sortable(&name_header, 140.0, SourceId::named("sort_name"))
                    .column_sortable(&size_header, 70.0, SourceId::named("sort_size"))
                    .column("TYPE", 70.0);

                for &(name, size_str, _size_bytes, kind, name_color) in &state.table_rows {
                    table = table.row(vec![
                        TableCell { text: name.into(), lines: vec![name.into()], color: name_color },
                        TableCell { text: size_str.into(), lines: vec![size_str.into()], color: colors::TEXT_SECONDARY },
                        TableCell { text: kind.into(), lines: vec![kind.into()], color: colors::TEXT_MUTED },
                    ]);
                }

                ScrollColumn::from_state(&state.right_scroll)
                    .spacing(16.0)
                    .width(Length::Fixed(right_col_width))
                    .height(Length::Fill)
                    // Status indicators
                    .push(
                        StatusPanel::new(
                            vec![
                                StatusIndicator { icon: "\u{25CF}", label: "Running", color: colors::RUNNING },
                                StatusIndicator { icon: "\u{2713}", label: "Success", color: colors::SUCCESS },
                                StatusIndicator { icon: "\u{2717}", label: "Error", color: colors::ERROR },
                                StatusIndicator { icon: "\u{2620}", label: "Killed", color: colors::KILLED },
                            ],
                            state.elapsed_seconds,
                        )
                        .id(SourceId::named("status_panel")),
                    )
                    // Multi-line editor
                    .push(
                        Card::new("Multi-line Editor")
                            .push(
                                TextInputElement::from_state(&state.editor)
                                    .height(Length::Fixed(120.0))
                                    .placeholder("Multi-line editor...")
                                    .cursor_visible(cursor_visible),
                            )
                            .id(SourceId::named("editor_panel")),
                    )
                    // Action buttons (hover-aware)
                    .push(
                        Row::new()
                            .spacing(8.0)
                            .width(Length::Fill)
                            .push(hover_button(
                                SourceId::named("copy_btn"),
                                "Copy",
                                colors::BG_CARD,
                                hovered,
                            ))
                            .push(hover_button(
                                SourceId::named("clear_btn"),
                                "Clear Chat",
                                colors::BTN_DENY,
                                hovered,
                            )),
                    )
                    // Context menu placeholder
                    .push(
                        Column::new()
                            .width(Length::Fill)
                            .height(Length::Fixed(194.0))
                            .id(SourceId::named("ctx_menu")),
                    )
                    // Drawing styles placeholder
                    .push(
                        Column::new()
                            .width(Length::Fill)
                            .height(Length::Fixed(180.0))
                            .id(SourceId::named("draw_styles")),
                    )
                    // Table
                    .push(Card::new("Table").push(table))
            })
            .layout(snapshot, Rect::new(0.0, 0.0, vw, vh));

        // Sync state helpers from layout snapshot
        state.left_scroll.sync_from_snapshot(snapshot);
        state.right_scroll.sync_from_snapshot(snapshot);
        state.input.sync_from_snapshot(snapshot);
        state.editor.sync_from_snapshot(snapshot);

        // Update chat list content-space position for next frame's virtualization
        if let Some(chat_bounds) = snapshot.widget_bounds(&SourceId::named("chat_list")) {
            let scroll_bounds = state.left_scroll.bounds.get();
            let content_top = chat_bounds.y - scroll_bounds.y + state.left_scroll.offset;
            state.chat_content_top.set(content_top);
        }

        // =================================================================
        // POST-LAYOUT: Render primitives into placeholder positions
        // =================================================================
        let anim_t = now.duration_since(state.start_time).as_secs_f32();

        if let Some(bounds) = snapshot.widget_bounds(&SourceId::named("ctx_menu")) {
            view_context_menu(snapshot, bounds.x, bounds.y);
        }

        if let Some(bounds) = snapshot.widget_bounds(&SourceId::named("draw_styles")) {
            view_drawing_styles(snapshot, bounds.x, bounds.y, bounds.width, anim_t);
        }

        // FPS counter (top-right corner)
        snapshot.primitives_mut().add_text(
            format!("{:.0} FPS", fps),
            Point::new(vw - 70.0, 4.0),
            colors::TEXT_MUTED,
            14.0,
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
        // Track hovered widget for button hover effects (Cell = zero-cost interior mutability)
        if let MouseEvent::CursorMoved { .. } = &event {
            state.hovered_widget.set(match &hit {
                Some(HitResult::Widget(id)) => Some(*id),
                _ => None,
            });
        }

        // Composable handlers: scroll panels + text inputs
        route_mouse!(&event, &hit, capture, [
            state.left_scroll  => DemoMessage::LeftScroll,
            state.right_scroll => DemoMessage::RightScroll,
            state.input        => DemoMessage::InputMouse,
            state.editor       => DemoMessage::EditorMouse,
        ]);

        // Button clicks and table sort headers
        if let MouseEvent::ButtonPressed { button: MouseButton::Left, .. } = &event {
            if let Some(HitResult::Widget(id)) = &hit {
                // Buttons
                if *id == SourceId::named("submit_btn") {
                    return MouseResponse::message(DemoMessage::SubmitCommand);
                }
                if *id == SourceId::named("copy_btn") {
                    return MouseResponse::message(DemoMessage::Copy);
                }
                if *id == SourceId::named("clear_btn") {
                    return MouseResponse::message(DemoMessage::ButtonClicked(*id));
                }
                // Table sort
                if *id == SourceId::named("sort_name") || *id == SourceId::named("sort_size") {
                    return MouseResponse::message(DemoMessage::TableSort(*id));
                }
            }
            // Text / grid cell selection
            if let Some(HitResult::Content(addr)) = hit {
                if state.input.focused || state.editor.focused {
                    return MouseResponse::message(DemoMessage::BlurAll);
                }
                let capture_source = addr.source_id;
                return MouseResponse::message_and_capture(
                    DemoMessage::SelectionStart(addr),
                    capture_source,
                );
            }
            // Clicked on empty space: blur inputs
            if state.input.focused || state.editor.focused {
                return MouseResponse::message(DemoMessage::BlurAll);
            }
        }

        // Content selection drag
        if let MouseEvent::CursorMoved { .. } = &event {
            if let CaptureState::Captured(_) = capture {
                if let Some(HitResult::Content(addr)) = hit {
                    return MouseResponse::message(DemoMessage::SelectionExtend(addr));
                }
            }
        }

        // Content selection release
        if let MouseEvent::ButtonReleased { button: MouseButton::Left, .. } = &event {
            if let CaptureState::Captured(_) = capture {
                return MouseResponse::message_and_release(DemoMessage::SelectionEnd);
            }
        }

        MouseResponse::none()
    }

    fn on_key(
        state: &Self::State,
        event: KeyEvent,
    ) -> Option<Self::Message> {
        // Only handle key presses
        if matches!(&event, KeyEvent::Released { .. }) {
            return None;
        }
        // Route to focused input (centralized focus check)
        if state.editor.focused {
            return Some(DemoMessage::EditorKey(event));
        }
        if state.input.focused {
            return Some(DemoMessage::InputKey(event));
        }
        // Global shortcuts
        if let KeyEvent::Pressed { ref key, ref modifiers, .. } = event {
            match (key, modifiers.meta) {
                (Key::Character(c), true) if c == "c" => return Some(DemoMessage::Copy),
                (Key::Named(NamedKey::Escape), _) => return Some(DemoMessage::ClearSelection),
                (Key::Named(NamedKey::ArrowUp), _) => {
                    return Some(DemoMessage::LeftScroll(ScrollAction::ScrollBy(60.0)));
                }
                (Key::Named(NamedKey::ArrowDown), _) => {
                    return Some(DemoMessage::LeftScroll(ScrollAction::ScrollBy(-60.0)));
                }
                (Key::Named(NamedKey::PageUp), _) => {
                    return Some(DemoMessage::LeftScroll(ScrollAction::ScrollBy(300.0)));
                }
                (Key::Named(NamedKey::PageDown), _) => {
                    return Some(DemoMessage::LeftScroll(ScrollAction::ScrollBy(-300.0)));
                }
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

    p.add_text("Context Menu", Point::new(x, y), colors::TEXT_SECONDARY, 14.0);

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
    p.add_text("Copy", Point::new(ix + 8.0, iy + 4.0), Color::WHITE, 14.0);
    p.add_text("\u{2318}C", Point::new(ix + iw - 30.0, iy + 4.0), colors::TEXT_MUTED, 14.0);

    // Paste
    let iy = iy + row_h;
    p.add_text("Paste", Point::new(ix + 8.0, iy + 4.0), colors::TEXT_PRIMARY, 14.0);
    p.add_text("\u{2318}V", Point::new(ix + iw - 30.0, iy + 4.0), colors::TEXT_MUTED, 14.0);

    // Separator
    let iy = iy + row_h;
    p.add_line(
        Point::new(ix, iy + sep_gap * 0.5),
        Point::new(ix + iw, iy + sep_gap * 0.5),
        1.0, Color::rgba(1.0, 1.0, 1.0, 0.08),
    );

    // Select All
    let iy = iy + sep_gap;
    p.add_text("Select All", Point::new(ix + 8.0, iy + 4.0), colors::TEXT_PRIMARY, 14.0);
    p.add_text("\u{2318}A", Point::new(ix + iw - 30.0, iy + 4.0), colors::TEXT_MUTED, 14.0);

    // Clear Selection (disabled)
    let iy = iy + row_h;
    p.add_text("Clear Selection", Point::new(ix + 8.0, iy + 4.0), colors::TEXT_MUTED, 14.0);

    // Separator
    let iy = iy + row_h;
    p.add_line(
        Point::new(ix, iy + sep_gap * 0.5),
        Point::new(ix + iw, iy + sep_gap * 0.5),
        1.0, Color::rgba(1.0, 1.0, 1.0, 0.08),
    );

    // Search
    let iy = iy + sep_gap;
    p.add_text("Search", Point::new(ix + 8.0, iy + 4.0), colors::TEXT_PRIMARY, 14.0);
    p.add_text("\u{2318}F", Point::new(ix + iw - 30.0, iy + 4.0), colors::TEXT_MUTED, 14.0);
}

// =========================================================================
// Overlay: Drawing Styles (lines, curves, polylines)
// =========================================================================

fn view_drawing_styles(snapshot: &mut LayoutSnapshot, x: f32, y: f32, width: f32, time: f32) {
    let p = snapshot.primitives_mut();

    p.add_rounded_rect(Rect::new(x, y, width, 180.0), 6.0, colors::BG_BLOCK);
    p.add_text("Drawing Styles", Point::new(x + 10.0, y + 6.0), colors::TEXT_SECONDARY, 14.0);

    let lx = x + 14.0;
    let lw = width - 28.0;

    // --- Solid lines (various thickness) ---
    let ly = y + 32.0;
    p.add_text("Solid", Point::new(lx, ly), colors::TEXT_MUTED, 14.0);
    p.add_line(Point::new(lx + 50.0, ly + 9.0), Point::new(lx + lw * 0.5, ly + 9.0), 1.0, colors::RUNNING);
    p.add_line(Point::new(lx + lw * 0.5 + 8.0, ly + 9.0), Point::new(lx + lw, ly + 9.0), 2.0, colors::SUCCESS);

    // --- Dashed lines ---
    let ly = ly + 24.0;
    p.add_text("Dashed", Point::new(lx, ly), colors::TEXT_MUTED, 14.0);
    p.add_line_styled(
        Point::new(lx + 50.0, ly + 9.0), Point::new(lx + lw, ly + 9.0),
        1.5, colors::WARNING, LineStyle::Dashed,
    );

    // --- Dotted lines ---
    let ly = ly + 24.0;
    p.add_text("Dotted", Point::new(lx, ly), colors::TEXT_MUTED, 14.0);
    p.add_line_styled(
        Point::new(lx + 50.0, ly + 9.0), Point::new(lx + lw, ly + 9.0),
        1.5, colors::ERROR, LineStyle::Dotted,
    );

    // --- Polyline (zigzag) ---
    let ly = ly + 24.0;
    p.add_text("Poly", Point::new(lx, ly), colors::TEXT_MUTED, 14.0);
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
    p.add_text("Curve", Point::new(lx, ly), colors::TEXT_MUTED, 14.0);
    let curve_w = lw - 50.0;
    let phase = time * 2.0;
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
    p.add_text("Wave", Point::new(lx, ly), colors::TEXT_MUTED, 14.0);
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
pub fn run() -> Result<(), crate::shell::Error> {
    crate::shell::run_with_config::<DemoApp>(AppConfig {
        title: String::from("Strata — Nexus Widget Demo"),
        window_size: (1050.0, 672.0),
        antialiasing: true,
        background_color: colors::BG_APP,
    })
}
