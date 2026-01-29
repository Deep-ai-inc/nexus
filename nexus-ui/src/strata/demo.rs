//! Demo application exercising all Strata rendering features needed for Nexus.
//!
//! Uses composed widget structs (from `demo_widgets`) for the main UI blocks,
//! proving that the layout engine handles real nexus layouts. Overlay elements
//! (context menu, completion popup, table) remain as free functions since
//! they are absolutely positioned and don't participate in flex layout.
//!
//! Run with: `cargo run -p nexus-ui --example strata_demo`

use crate::strata::content_address::{ContentAddress, SourceId};
use crate::strata::demo_widgets::{
    AgentBlock, InputBar, JobPanel, JobPill, PermissionDialog, ShellBlock, StatusIndicator,
    StatusPanel, ToolInvocation,
};
use crate::strata::event_context::{CaptureState, MouseButton, MouseEvent};
use crate::strata::layout::containers::Length;
use crate::strata::layout::primitives::LineStyle;
use crate::strata::primitives::{Color, Point, Rect};
use crate::strata::{
    AppConfig, Column, Command, LayoutSnapshot, MouseResponse, Row, Selection, StrataApp,
    Subscription,
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

/// Demo message type.
#[derive(Debug, Clone)]
pub enum DemoMessage {
    SelectionStart(ContentAddress),
    SelectionExtend(ContentAddress),
    SelectionEnd,
}

/// Demo application state.
pub struct DemoState {
    // Stable source IDs for selectable content
    query_source: SourceId,
    response_source: SourceId,
    terminal_source: SourceId,
    tool_output_source: SourceId,
    // Selection state
    selection: Option<Selection>,
    is_selecting: bool,
}

/// Demo application.
pub struct DemoApp;

impl StrataApp for DemoApp {
    type State = DemoState;
    type Message = DemoMessage;

    fn init() -> (Self::State, Command<Self::Message>) {
        let state = DemoState {
            query_source: SourceId::new(),
            response_source: SourceId::new(),
            terminal_source: SourceId::new(),
            tool_output_source: SourceId::new(),
            selection: None,
            is_selecting: false,
        };
        (state, Command::none())
    }

    fn update(state: &mut Self::State, message: Self::Message) -> Command<Self::Message> {
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
        }
        Command::none()
    }

    fn view(state: &Self::State, snapshot: &mut LayoutSnapshot) {
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
            // LEFT COLUMN: Nexus App Mockup
            // =============================================================
            .column(
                Column::new()
                    .spacing(16.0)
                    .width(Length::Fill)
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
                    .column(PermissionDialog { command: "rm -rf /tmp/cache" }.build())
                    // Input Bar
                    .row(
                        InputBar {
                            cwd: "~/Desktop/nexus",
                            mode: "SH",
                            mode_color: colors::SUCCESS,
                            mode_bg: colors::BTN_MODE_SH,
                        }
                        .build(),
                    ),
            )
            // =============================================================
            // RIGHT COLUMN: Component Catalog
            // =============================================================
            .column(
                Column::new()
                    .spacing(16.0)
                    .width(Length::Fixed(right_col_width))
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
                        }
                        .build(),
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
                        .build(),
                    ),
            )
            .layout(snapshot, Rect::new(0.0, 0.0, vw, vh));

        // =================================================================
        // OVERLAYS (absolute positioned, not in flow layout)
        // =================================================================
        // Right column x: viewport - outer padding - right column width
        let rx = vw - outer_padding - right_col_width;
        // Right column panels end at ~y=160 (StatusPanel ~62 + gap 16 + JobPanel ~66 + top padding 16)
        view_context_menu(snapshot, rx, 166.0);
        view_drawing_styles(snapshot, rx, 344.0, right_col_width);
        view_table(snapshot, rx, 540.0, right_col_width);
    }

    fn selection(state: &Self::State) -> Option<&Selection> {
        state.selection.as_ref()
    }

    fn on_mouse(
        state: &Self::State,
        event: MouseEvent,
        hit: Option<ContentAddress>,
        capture: &CaptureState,
    ) -> MouseResponse<Self::Message> {
        match event {
            MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                ..
            } => {
                if let Some(addr) = hit {
                    return MouseResponse::message_and_capture(
                        DemoMessage::SelectionStart(addr),
                        state.query_source,
                    );
                }
            }
            MouseEvent::CursorMoved { .. } => {
                if capture.is_captured() {
                    if let Some(addr) = hit {
                        return MouseResponse::message(DemoMessage::SelectionExtend(addr));
                    }
                }
            }
            MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            } => {
                if capture.is_captured() {
                    return MouseResponse::message_and_release(DemoMessage::SelectionEnd);
                }
            }
            _ => {}
        }
        MouseResponse::none()
    }

    fn subscription(_state: &Self::State) -> Subscription<Self::Message> {
        Subscription::none()
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
// Overlay: Table (absolute positioned)
// =========================================================================

fn view_table(snapshot: &mut LayoutSnapshot, x: f32, y: f32, width: f32) {
    let p = snapshot.primitives_mut();

    p.add_rounded_rect(Rect::new(x, y, width, 160.0), 6.0, colors::BG_BLOCK);
    p.add_text("Table", Point::new(x + 10.0, y + 6.0), colors::TEXT_SECONDARY);

    let tx = x + 14.0;
    let ty = y + 30.0;
    let col1 = tx;
    let col2 = tx + 140.0;
    let col3 = tx + 210.0;

    p.add_text("NAME", Point::new(col1, ty), colors::TEXT_SECONDARY);
    p.add_text("SIZE \u{25BC}", Point::new(col2, ty), colors::TEXT_PATH);
    p.add_text("TYPE", Point::new(col3, ty), colors::TEXT_SECONDARY);

    p.add_line(
        Point::new(tx, ty + 20.0),
        Point::new(tx + width - 28.0, ty + 20.0),
        1.0, Color::rgba(1.0, 1.0, 1.0, 0.12),
    );

    let rows: &[(&str, &str, &str, Color)] = &[
        ("src/", "256B", "dir", colors::TEXT_PATH),
        ("main.rs", "420B", "rust", colors::TEXT_PRIMARY),
        ("lib.rs", "1.2K", "rust", colors::TEXT_PRIMARY),
        ("Cargo.toml", "890B", "toml", colors::TEXT_PRIMARY),
        ("README.md", "2.4K", "md", colors::TEXT_PRIMARY),
    ];

    for (i, (name, size, kind, color)) in rows.iter().enumerate() {
        let ry = ty + 26.0 + i as f32 * 22.0;

        if i == 0 {
            p.add_rounded_rect(
                Rect::new(tx - 4.0, ry - 2.0, width - 20.0, 20.0),
                3.0, Color::rgba(1.0, 1.0, 1.0, 0.04),
            );
        }

        p.add_text(*name, Point::new(col1, ry), *color);
        p.add_text(*size, Point::new(col2, ry), colors::TEXT_SECONDARY);
        p.add_text(*kind, Point::new(col3, ry), colors::TEXT_MUTED);
    }
}

// =========================================================================
// Overlay: Drawing Styles (lines, curves, polylines)
// =========================================================================

fn view_drawing_styles(snapshot: &mut LayoutSnapshot, x: f32, y: f32, width: f32) {
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

    // --- Polyline (smooth curve approximation) ---
    let ly = ly + 28.0;
    p.add_text("Curve", Point::new(lx, ly), colors::TEXT_MUTED);
    let curve_w = lw - 50.0;
    let curve: Vec<Point> = (0..30)
        .map(|i| {
            let t = i as f32 / 29.0;
            let px = lx + 50.0 + t * curve_w;
            let py = ly + 8.0 - (t * std::f32::consts::PI * 2.0).sin() * 8.0;
            Point::new(px, py)
        })
        .collect();
    p.add_polyline(curve, 1.5, colors::RUNNING);

    // --- Dashed polyline (wave) ---
    let ly = ly + 28.0;
    p.add_text("Wave", Point::new(lx, ly), colors::TEXT_MUTED);
    let wave: Vec<Point> = (0..30)
        .map(|i| {
            let t = i as f32 / 29.0;
            let px = lx + 50.0 + t * curve_w;
            let py = ly + 8.0 - (t * std::f32::consts::PI * 3.0).sin() * 6.0;
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
