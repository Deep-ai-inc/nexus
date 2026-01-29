//! Demo application to test Strata shell.
//!
//! Run with: `cargo run -p nexus-ui --example strata_demo`
//! Or temporarily replace main() to call `strata::demo::run()`

use crate::strata::content_address::{ContentAddress, SourceId};
use crate::strata::event_context::{CaptureState, MouseButton, MouseEvent};
use crate::strata::primitives::{Color, Rect};
use crate::strata::widget::StrataWidget;
use crate::strata::widgets::{TextWidget, TerminalWidget};
use crate::strata::{
    AppConfig, Command, LayoutSnapshot, Selection, StrataApp, Subscription,
};

/// Demo message type.
#[derive(Debug, Clone)]
pub enum DemoMessage {
    Tick,
    /// User clicked at a content address.
    Clicked(ContentAddress),
}

/// Demo application state.
pub struct DemoState {
    tick_count: u64,
    /// Stable source IDs for our demo content.
    title_source: SourceId,
    hello_source: SourceId,
    pangram_source: SourceId,
    terminal_source: SourceId,
    status_source: SourceId,
    /// Last clicked position (for displaying hit-test results).
    last_click: Option<ContentAddress>,
}

/// Demo application.
pub struct DemoApp;

impl StrataApp for DemoApp {
    type State = DemoState;
    type Message = DemoMessage;

    fn init() -> (Self::State, Command<Self::Message>) {
        let state = DemoState {
            tick_count: 0,
            title_source: SourceId::new(),
            hello_source: SourceId::new(),
            pangram_source: SourceId::new(),
            terminal_source: SourceId::new(),
            status_source: SourceId::new(),
            last_click: None,
        };
        (state, Command::none())
    }

    fn update(state: &mut Self::State, message: Self::Message) -> Command<Self::Message> {
        match message {
            DemoMessage::Tick => {
                state.tick_count += 1;
            }
            DemoMessage::Clicked(addr) => {
                state.last_click = Some(addr);
            }
        }
        Command::none()
    }

    fn view(state: &Self::State, snapshot: &mut LayoutSnapshot) {
        // Demo content using the new widget system.
        // Widgets handle text shaping and layout registration automatically.

        // Title text (white)
        let mut title = TextWidget::with_source_id(state.title_source, "Strata Widget Demo")
            .color(Color::WHITE);
        title.layout(snapshot, Rect::new(20.0, 20.0, 400.0, 20.0));

        // Hello text (green)
        let mut hello = TextWidget::with_source_id(state.hello_source, "Hello from TextWidget!")
            .color(Color::rgb(0.3, 0.9, 0.4));
        hello.layout(snapshot, Rect::new(20.0, 50.0, 400.0, 20.0));

        // Pangram text (white)
        let mut pangram = TextWidget::with_source_id(
            state.pangram_source,
            "The quick brown fox jumps over the lazy dog.",
        )
        .color(Color::WHITE);
        pangram.layout(snapshot, Rect::new(20.0, 80.0, 400.0, 20.0));

        // Terminal widget demo (small 20x3 grid)
        let mut terminal = TerminalWidget::with_source_id(
            state.terminal_source,
            20, 3,  // cols, rows
            8.4, 20.0,  // cell_width, cell_height
        );
        terminal.write_str(0, 0, "Terminal Grid:", Color::rgb(0.5, 0.8, 1.0), Color::TRANSPARENT);
        terminal.write_str(0, 1, "Row 1: Hello", Color::WHITE, Color::TRANSPARENT);
        terminal.write_str(0, 2, "Row 2: World", Color::WHITE, Color::TRANSPARENT);
        terminal.layout(snapshot, Rect::new(20.0, 120.0, 200.0, 60.0));

        // Status line showing last click (yellow)
        let status_text = if let Some(addr) = &state.last_click {
            format!(
                "Clicked: pos {} in source {:?}",
                addr.content_offset, addr.source_id
            )
        } else {
            "Click on text or terminal to test hit-testing".to_string()
        };
        let mut status = TextWidget::with_source_id(state.status_source, status_text)
            .color(Color::rgb(1.0, 0.9, 0.3));
        status.layout(snapshot, Rect::new(20.0, 200.0, 600.0, 20.0));
    }

    fn selection(_state: &Self::State) -> Option<&Selection> {
        None
    }

    fn on_mouse(
        _state: &Self::State,
        event: MouseEvent,
        hit: Option<ContentAddress>,
        _capture: &CaptureState,
    ) -> Option<Self::Message> {
        // When user clicks, send the hit result as a message
        if let MouseEvent::ButtonPressed { button: MouseButton::Left, .. } = event {
            if let Some(addr) = hit {
                return Some(DemoMessage::Clicked(addr));
            }
        }
        None
    }

    fn subscription(_state: &Self::State) -> Subscription<Self::Message> {
        Subscription::none()
    }

    fn title(_state: &Self::State) -> String {
        String::from("Strata Demo")
    }
}

/// Run the demo application.
pub fn run() -> Result<(), crate::strata::shell::Error> {
    crate::strata::shell::run_with_config::<DemoApp>(AppConfig {
        title: String::from("Strata Demo"),
        window_size: (800.0, 600.0),
        antialiasing: true,
        background_color: crate::strata::Color::rgb(0.15, 0.15, 0.2),
    })
}
