//! Demo application to test Strata shell and shader features.
//!
//! Tests all ubershader capabilities in a single draw call:
//! - Glyphs (textured quads from atlas)
//! - Solid rectangles (white pixel trick)
//! - Rounded rectangles (SDF-based)
//! - Circles (rounded rect where radius = size/2)
//! - Selection highlighting
//!
//! Run with: `cargo run -p nexus-ui --example strata_demo`

use crate::strata::content_address::{ContentAddress, SourceId};
use crate::strata::event_context::{CaptureState, MouseButton, MouseEvent};
use crate::strata::primitives::{Color, Point, Rect};
use crate::strata::widget::StrataWidget;
use crate::strata::widgets::{TerminalWidget, TextWidget};
use crate::strata::{
    AppConfig, Command, Decoration, LayoutSnapshot, MouseResponse, Selection, StrataApp,
    Subscription,
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
    /// Stable source IDs for demo content.
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
        // TEST: Solid Rectangles (white pixel trick)
        // =====================================================================
        // Panel background (dark gray)
        snapshot.add_solid_rect(
            Rect::new(10.0, 10.0, 500.0, 350.0),
            Color::rgb(0.12, 0.12, 0.15),
        );

        // Colored bar at top (purple accent)
        snapshot.add_solid_rect(
            Rect::new(10.0, 10.0, 500.0, 4.0),
            Color::rgb(0.6, 0.4, 0.9),
        );

        // =====================================================================
        // TEST: Rounded Rectangles (SDF-based)
        // =====================================================================
        // Button-like rounded rects
        snapshot.add_rounded_rect(
            Rect::new(530.0, 30.0, 100.0, 36.0),
            8.0, // corner radius
            Color::rgb(0.2, 0.6, 0.9),
        );
        snapshot.add_rounded_rect(
            Rect::new(530.0, 80.0, 100.0, 36.0),
            8.0,
            Color::rgb(0.9, 0.3, 0.4),
        );
        snapshot.add_rounded_rect(
            Rect::new(530.0, 130.0, 100.0, 36.0),
            8.0,
            Color::rgb(0.3, 0.8, 0.5),
        );

        // Pill-shaped rect (radius = height/2)
        snapshot.add_rounded_rect(
            Rect::new(530.0, 180.0, 140.0, 30.0),
            15.0,
            Color::rgb(0.9, 0.7, 0.2),
        );

        // Large rounded card
        snapshot.add_rounded_rect(
            Rect::new(530.0, 230.0, 180.0, 120.0),
            12.0,
            Color::rgb(0.15, 0.15, 0.2),
        );

        // =====================================================================
        // TEST: Circles (SDF-based)
        // =====================================================================
        // Status indicators (small circles)
        snapshot.add_circle(Point::new(560.0, 260.0), 8.0, Color::rgb(0.3, 0.9, 0.5)); // green
        snapshot.add_circle(Point::new(590.0, 260.0), 8.0, Color::rgb(0.9, 0.7, 0.2)); // yellow
        snapshot.add_circle(Point::new(620.0, 260.0), 8.0, Color::rgb(0.9, 0.3, 0.3)); // red

        // Larger decorative circle
        snapshot.add_circle(Point::new(660.0, 310.0), 20.0, Color::rgba(0.5, 0.5, 0.9, 0.5));

        // =====================================================================
        // TEST: Glyphs (textured quads from atlas)
        // =====================================================================
        // Title text (white)
        let mut title = TextWidget::with_source_id(state.title_source, "Strata Ubershader Demo")
            .color(Color::WHITE);
        title.layout(snapshot, Rect::new(30.0, 30.0, 400.0, 24.0));

        // Subtitle text (gray)
        let mut subtitle = TextWidget::with_source_id(
            state.subtitle_source,
            "All primitives rendered in a single draw call",
        )
        .color(Color::rgb(0.6, 0.6, 0.7));
        subtitle.layout(snapshot, Rect::new(30.0, 60.0, 400.0, 18.0));

        // Pangram text (cyan) - selectable
        let mut pangram = TextWidget::with_source_id(
            state.pangram_source,
            "The quick brown fox jumps over the lazy dog.",
        )
        .color(Color::rgb(0.4, 0.9, 0.9));
        pangram.layout(snapshot, Rect::new(30.0, 100.0, 450.0, 18.0));

        // Button labels (on top of rounded rects)
        let mut btn1 = TextWidget::new("Primary").color(Color::WHITE);
        btn1.layout(snapshot, Rect::new(555.0, 42.0, 80.0, 18.0));
        let mut btn2 = TextWidget::new("Delete").color(Color::WHITE);
        btn2.layout(snapshot, Rect::new(555.0, 92.0, 80.0, 18.0));
        let mut btn3 = TextWidget::new("Success").color(Color::WHITE);
        btn3.layout(snapshot, Rect::new(555.0, 142.0, 80.0, 18.0));
        let mut btn_pill = TextWidget::new("Pill Button").color(Color::BLACK);
        btn_pill.layout(snapshot, Rect::new(555.0, 190.0, 110.0, 18.0));

        // Card content
        let mut card_title = TextWidget::new("Card Title").color(Color::WHITE);
        card_title.layout(snapshot, Rect::new(545.0, 290.0, 100.0, 18.0));
        let mut card_body = TextWidget::new("With SDF corners").color(Color::rgb(0.7, 0.7, 0.8));
        card_body.layout(snapshot, Rect::new(545.0, 315.0, 120.0, 18.0));

        // =====================================================================
        // TEST: Terminal Grid (grid layout)
        // =====================================================================
        let mut terminal = TerminalWidget::with_source_id(
            state.terminal_source,
            50,
            8, // cols, rows
            8.4,
            18.0, // cell_width, cell_height
        );
        terminal.write_str(0, 0, "$ cargo run --example strata_demo", Color::rgb(0.5, 0.8, 1.0), Color::TRANSPARENT);
        terminal.write_str(0, 1, "   Compiling nexus-ui v0.1.0", Color::rgb(0.3, 0.9, 0.4), Color::TRANSPARENT);
        terminal.write_str(0, 2, "    Finished dev [unoptimized + debuginfo]", Color::rgb(0.3, 0.9, 0.4), Color::TRANSPARENT);
        terminal.write_str(0, 3, "     Running `target/debug/examples/strata_demo`", Color::rgb(0.3, 0.9, 0.4), Color::TRANSPARENT);
        terminal.write_str(0, 4, "", Color::WHITE, Color::TRANSPARENT);
        terminal.write_str(0, 5, "Shader features tested:", Color::WHITE, Color::TRANSPARENT);
        terminal.write_str(0, 6, "  [x] Glyphs  [x] Solid rects  [x] Rounded rects", Color::rgb(0.7, 0.7, 0.8), Color::TRANSPARENT);
        terminal.write_str(0, 7, "  [x] Circles [x] Selection    [x] Anti-aliasing", Color::rgb(0.7, 0.7, 0.8), Color::TRANSPARENT);
        terminal.layout(snapshot, Rect::new(30.0, 140.0, 420.0, 144.0));

        // Footer text with selection instructions
        let footer_text = if state.selection.is_some() {
            "Selection active! Drag to extend, release to finish."
        } else {
            "Click and drag on text to test selection highlighting."
        };
        let mut footer =
            TextWidget::with_source_id(state.footer_source, footer_text).color(Color::rgb(0.9, 0.8, 0.3));
        footer.layout(snapshot, Rect::new(30.0, 310.0, 450.0, 18.0));

        // =====================================================================
        // TEST: Foreground Decorations
        // =====================================================================
        // Add some foreground decorations (rendered on top of text)
        // Small decorative circle in corner
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
                    // Start selection and capture the pointer for drag
                    return MouseResponse::message_and_capture(
                        DemoMessage::SelectionStart(addr),
                        state.pangram_source, // Capture to the selectable source
                    );
                }
            }
            MouseEvent::CursorMoved { .. } => {
                // Extend selection while dragging (captured)
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
                // Release capture and end selection
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
        String::from("Strata Ubershader Demo")
    }
}

/// Run the demo application.
pub fn run() -> Result<(), crate::strata::shell::Error> {
    crate::strata::shell::run_with_config::<DemoApp>(AppConfig {
        title: String::from("Strata Ubershader Demo"),
        window_size: (750.0, 400.0),
        antialiasing: true,
        background_color: crate::strata::Color::rgb(0.08, 0.08, 0.1),
    })
}
