//! Demo application to test Strata shell.
//!
//! Run with: `cargo run -p nexus-ui --example strata_demo`
//! Or temporarily replace main() to call `strata::demo::run()`

use crate::strata::content_address::{ContentAddress, SourceId};
use crate::strata::event_context::{MouseButton, MouseEvent};
use crate::strata::layout_snapshot::{SourceLayout, TextLayout};
use crate::strata::primitives::Color;
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
        // Register demo content with snapshot.
        // Positions are in logical coordinates - the shell adapter scales them.
        // We use a fixed char_width estimate; proper text shaping comes in Phase 2.
        let char_width = 8.4; // Approximate for 14pt monospace
        let line_height = 20.0;

        // Title text (white)
        let title = TextLayout::simple(
            "Strata GPU Text Rendering",
            Color::WHITE.pack(),
            20.0,
            20.0,
            char_width,
            line_height,
        );
        snapshot.register_source(state.title_source, SourceLayout::text(title));

        // Hello text (green)
        let hello = TextLayout::simple(
            "Hello, World!",
            Color::rgb(0.3, 0.9, 0.4).pack(),
            20.0,
            50.0,
            char_width,
            line_height,
        );
        snapshot.register_source(state.hello_source, SourceLayout::text(hello));

        // Pangram text (white)
        let pangram = TextLayout::simple(
            "The quick brown fox jumps over the lazy dog.",
            Color::WHITE.pack(),
            20.0,
            80.0,
            char_width,
            line_height,
        );
        snapshot.register_source(state.pangram_source, SourceLayout::text(pangram));

        // Status line showing last click (yellow)
        let status_text = if let Some(addr) = &state.last_click {
            format!("Clicked: offset {} in source {:?}", addr.content_offset, addr.source_id)
        } else {
            "Click on text to test hit-testing".to_string()
        };
        let status = TextLayout::simple(
            status_text,
            Color::rgb(1.0, 0.9, 0.3).pack(),
            20.0,
            120.0,
            char_width,
            line_height,
        );
        snapshot.register_source(state.status_source, SourceLayout::text(status));
    }

    fn selection(_state: &Self::State) -> Option<&Selection> {
        None
    }

    fn on_mouse(
        _state: &Self::State,
        event: MouseEvent,
        hit: Option<ContentAddress>,
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
