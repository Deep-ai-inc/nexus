//! Demo application exercising all Strata ubershader features.
//!
//! **Ubershader rendering features:**
//! - Solid rectangles (white pixel trick, zero-branch)
//! - Rounded rectangles (SDF with screen-space AA)
//! - Circles (SDF, same pipeline as rounded rects)
//! - Line segments (rotated quads, GPU-expanded)
//! - Polylines / charts (N-1 line segment instances)
//! - Glyph rendering (atlas-sampled, anti-aliased)
//! - Alpha blending (semi-transparent overlays)
//! - Selection highlighting (hit-tested, cross-source)
//! - Z-ordering via instance submission order
//!
//! **Layout system features:**
//! - Column/Row containers with flexbox semantics
//! - PrimitiveBatch for direct GPU instance access
//! - Declarative TextElement with auto-cached keys
//! - TerminalElement grid rendering
//! - Background/foreground decoration layers
//! - Pointer capture for drag selection
//!
//! Run with: `cargo run -p nexus-ui --example strata_demo`

use crate::strata::content_address::{ContentAddress, SourceId};
use crate::strata::event_context::{CaptureState, MouseButton, MouseEvent};
use crate::strata::layout::containers::{TextElement, TerminalElement};
use crate::strata::layout::primitives::LineStyle;
use crate::strata::primitives::{Color, Point, Rect};
use crate::strata::{
    AppConfig, Column, Command, Decoration, LayoutSnapshot,
    MouseResponse, Selection, StrataApp, Subscription,
};

/// Demo message type.
#[derive(Debug, Clone)]
pub enum DemoMessage {
    /// Selection started at an address.
    SelectionStart(ContentAddress),
    /// Selection extended to an address.
    SelectionExtend(ContentAddress),
    /// Selection ended.
    SelectionEnd,
}

/// Demo application state.
pub struct DemoState {
    /// Stable source IDs for demo content (created once, reused every frame).
    title_source: SourceId,
    subtitle_source: SourceId,
    pangram_source: SourceId,
    terminal_source: SourceId,
    footer_source: SourceId,
    /// Current selection.
    selection: Option<Selection>,
    /// Whether we're currently dragging to select.
    is_selecting: bool,
}

/// Demo application.
pub struct DemoApp;

impl StrataApp for DemoApp {
    type State = DemoState;
    type Message = DemoMessage;

    fn init() -> (Self::State, Command<Self::Message>) {
        let state = DemoState {
            title_source: SourceId::new(),
            subtitle_source: SourceId::new(),
            pangram_source: SourceId::new(),
            terminal_source: SourceId::new(),
            footer_source: SourceId::new(),
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
        // =====================================================================
        // PRODUCTION PATTERN: Direct Primitive Access
        // =====================================================================
        // For backgrounds and decorations, use primitives() directly.
        // This is the fastest path - maps 1:1 to GPU instances.
        snapshot
            .primitives_mut()
            // Panel background
            .add_solid_rect(Rect::new(10.0, 10.0, 500.0, 350.0), Color::rgb(0.12, 0.12, 0.15))
            // Accent bar
            .add_solid_rect(Rect::new(10.0, 10.0, 500.0, 4.0), Color::rgb(0.6, 0.4, 0.9))
            // Button backgrounds (rounded)
            .add_rounded_rect(Rect::new(530.0, 30.0, 100.0, 36.0), 8.0, Color::rgb(0.2, 0.6, 0.9))
            .add_rounded_rect(Rect::new(530.0, 80.0, 100.0, 36.0), 8.0, Color::rgb(0.9, 0.3, 0.4))
            .add_rounded_rect(Rect::new(530.0, 130.0, 100.0, 36.0), 8.0, Color::rgb(0.3, 0.8, 0.5))
            // Pill button
            .add_rounded_rect(Rect::new(530.0, 180.0, 140.0, 30.0), 15.0, Color::rgb(0.9, 0.7, 0.2))
            // Card shadow (drawn before card for correct layering)
            .add_shadow(
                Rect::new(534.0, 234.0, 180.0, 120.0),
                12.0,
                12.0,
                Color::rgba(0.0, 0.0, 0.0, 0.5),
            )
            // Card
            .add_rounded_rect(Rect::new(530.0, 230.0, 180.0, 120.0), 12.0, Color::rgb(0.15, 0.15, 0.2))
            // Card border
            .add_border(
                Rect::new(530.0, 230.0, 180.0, 120.0),
                12.0,
                1.0,
                Color::rgba(1.0, 1.0, 1.0, 0.1),
            )
            // Status circles
            .add_circle(Point::new(560.0, 260.0), 8.0, Color::rgb(0.3, 0.9, 0.5))
            .add_circle(Point::new(590.0, 260.0), 8.0, Color::rgb(0.9, 0.7, 0.2))
            .add_circle(Point::new(620.0, 260.0), 8.0, Color::rgb(0.9, 0.3, 0.3))
            // Semi-transparent overlay (demonstrates alpha blending)
            .add_rounded_rect(
                Rect::new(640.0, 250.0, 60.0, 30.0),
                6.0,
                Color::rgba(1.0, 1.0, 1.0, 0.15),
            )
            // =====================================================================
            // Lines and Polylines (rendered as rotated quads in the ubershader)
            // =====================================================================
            // Solid line
            .add_line(
                Point::new(530.0, 370.0),
                Point::new(700.0, 370.0),
                2.0,
                Color::rgb(0.5, 0.5, 0.6),
            )
            // Dashed line
            .add_line_styled(
                Point::new(530.0, 382.0),
                Point::new(700.0, 382.0),
                2.0,
                Color::rgb(0.9, 0.5, 0.3),
                LineStyle::Dashed,
            )
            // Dotted line
            .add_line_styled(
                Point::new(530.0, 394.0),
                Point::new(700.0, 394.0),
                2.0,
                Color::rgb(0.4, 0.8, 0.5),
                LineStyle::Dotted,
            )
            // Mini chart background
            .add_rounded_rect(
                Rect::new(525.0, 400.0, 190.0, 100.0),
                8.0,
                Color::rgb(0.1, 0.1, 0.14),
            )
            // Chart: sine-wave polyline (simulated data)
            // Curves sit in the lower 75px of the chart (y=425..490)
            .add_polyline(
                {
                    let mut points = Vec::with_capacity(30);
                    for i in 0..30 {
                        let t = i as f32 / 29.0;
                        let x = 535.0 + t * 170.0;
                        let y = 462.0 - (t * 6.0).sin() * 25.0;
                        points.push(Point::new(x, y));
                    }
                    points
                },
                2.0,
                Color::rgb(0.3, 0.8, 0.9),
            )
            // Chart: second polyline (offset data)
            .add_polyline(
                {
                    let mut points = Vec::with_capacity(30);
                    for i in 0..30 {
                        let t = i as f32 / 29.0;
                        let x = 535.0 + t * 170.0;
                        let y = 462.0 - (t * 6.0 + 2.0).sin() * 18.0 - 4.0;
                        points.push(Point::new(x, y));
                    }
                    points
                },
                1.5,
                Color::rgb(0.9, 0.4, 0.6),
            );

        // =====================================================================
        // PRODUCTION PATTERN: Declarative Layout with Containers
        // =====================================================================
        // For structured content, use Column/Row with declarative elements.
        // Layout is computed ONCE when layout() is called, not per-widget.

        // Main content column (left side)
        let content = Column::new()
            .spacing(12.0)
            .padding(20.0)
            // Title - uses stable source ID for hit-testing
            .text(
                TextElement::new("Strata Production API Demo")
                    .source(state.title_source)
                    .color(Color::WHITE),
            )
            // Subtitle
            .text(
                TextElement::new("Layout computed once per frame, not per widget")
                    .source(state.subtitle_source)
                    .color(Color::rgb(0.6, 0.6, 0.7)),
            )
            // Selectable text
            .text(
                TextElement::new("The quick brown fox jumps over the lazy dog.")
                    .source(state.pangram_source)
                    .color(Color::rgb(0.4, 0.9, 0.9)),
            )
            // Terminal grid
            .terminal(
                TerminalElement::new(state.terminal_source, 50, 4)
                    .cell_size(8.4, 18.0)
                    .row("$ cargo build --release", Color::rgb(0.4, 0.8, 0.4))
                    .row("   Compiling nexus-ui v0.1.0", Color::rgb(0.7, 0.7, 0.7))
                    .row("    Finished release [optimized]", Color::rgb(0.4, 0.8, 0.4))
                    .row("$ _", Color::rgb(0.8, 0.8, 0.8)),
            )
            // Footer with dynamic content (no cache key = always reshape)
            .text(
                TextElement::new(if state.selection.is_some() {
                    "Selection active! Drag to extend."
                } else {
                    "Click and drag on text to select."
                })
                .source(state.footer_source)
                .color(Color::rgb(0.9, 0.8, 0.3)),
            );

        // Compute layout and flush to snapshot
        content.layout(snapshot, Rect::new(10.0, 14.0, 480.0, 340.0));

        // Button labels (centered on top of rounded rects)
        // Buttons at y=30,80,130 (36px tall, 50px apart).
        // Center 18px text: start y=39, spacing=50-18=32.
        let buttons = Column::new()
            .spacing(32.0)
            .text(TextElement::new("Primary").color(Color::WHITE))
            .text(TextElement::new("Delete").color(Color::WHITE))
            .text(TextElement::new("Success").color(Color::WHITE));

        buttons.layout(snapshot, Rect::new(555.0, 39.0, 80.0, 150.0));

        // Pill button label
        snapshot.primitives_mut().add_text(
            "Pill Button",
            Point::new(560.0, 188.0),
            Color::BLACK,
        );

        // Card content
        let card = Column::new()
            .spacing(8.0)
            .text(TextElement::new("Card Title").color(Color::WHITE))
            .text(TextElement::new("With SDF corners").color(Color::rgb(0.7, 0.7, 0.8)));

        card.layout(snapshot, Rect::new(545.0, 288.0, 150.0, 50.0));

        // Chart label
        snapshot.primitives_mut().add_text(
            "GPU Chart (polylines)",
            Point::new(545.0, 405.0),
            Color::rgb(0.6, 0.6, 0.7),
        );

        // =====================================================================
        // Background decoration (rendered behind primitives)
        // =====================================================================
        // Subtle glow behind the card - demonstrates background layer ordering
        snapshot.add_background(Decoration::RoundedRect {
            rect: Rect::new(525.0, 225.0, 190.0, 130.0),
            corner_radius: 16.0,
            color: Color::rgba(0.4, 0.3, 0.8, 0.3),
        });

        // =====================================================================
        // Foreground decoration (rendered on top of everything)
        // =====================================================================
        snapshot.add_foreground(Decoration::Circle {
            center: Point::new(495.0, 25.0),
            radius: 6.0,
            color: Color::rgb(0.9, 0.3, 0.9),
        });
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
                        state.pangram_source,
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
        String::from("Strata Production API Demo")
    }
}

/// Run the demo application.
pub fn run() -> Result<(), crate::strata::shell::Error> {
    crate::strata::shell::run_with_config::<DemoApp>(AppConfig {
        title: String::from("Strata Production API Demo"),
        window_size: (750.0, 520.0),
        antialiasing: true,
        background_color: crate::strata::Color::rgb(0.08, 0.08, 0.1),
    })
}
