//! Demo application to test Strata shell.
//!
//! Run with: `cargo run -p nexus-ui --example strata_demo`
//! Or temporarily replace main() to call `strata::demo::run()`

use std::cell::RefCell;

use crate::strata::content_address::{ContentAddress, SourceId};
use crate::strata::event_context::{MouseButton, MouseEvent};
use crate::strata::layout_snapshot::{SourceLayout, TextLayout};
use crate::strata::primitives::Color;
use crate::strata::text_engine::{TextAttrs, TextEngine};
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
    /// Text engine for cosmic-text shaping (RefCell for interior mutability in view()).
    text_engine: RefCell<TextEngine>,
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
            text_engine: RefCell::new(TextEngine::new()),
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
        // Use cosmic-text for accurate character positioning.
        let mut engine = state.text_engine.borrow_mut();

        // Title text (white)
        let title_attrs = TextAttrs {
            color: Color::WHITE,
            ..Default::default()
        };
        let title_shaped = engine.shape("Strata GPU Text Rendering", &title_attrs);
        let title = TextLayout::from_shaped(&title_shaped, 20.0, 20.0);
        snapshot.register_source(state.title_source, SourceLayout::text(title));

        // Hello text (green)
        let hello_attrs = TextAttrs {
            color: Color::rgb(0.3, 0.9, 0.4),
            ..Default::default()
        };
        let hello_shaped = engine.shape("Hello, World!", &hello_attrs);
        let hello = TextLayout::from_shaped(&hello_shaped, 20.0, 50.0);
        snapshot.register_source(state.hello_source, SourceLayout::text(hello));

        // Pangram text (white)
        let pangram_attrs = TextAttrs {
            color: Color::WHITE,
            ..Default::default()
        };
        let pangram_shaped = engine.shape("The quick brown fox jumps over the lazy dog.", &pangram_attrs);
        let pangram = TextLayout::from_shaped(&pangram_shaped, 20.0, 80.0);
        snapshot.register_source(state.pangram_source, SourceLayout::text(pangram));

        // Status line showing last click (yellow)
        // content_offset is a cursor position (0 to N), not a character index
        let status_text = if let Some(addr) = &state.last_click {
            format!("Cursor position: {} (between chars {} and {})",
                addr.content_offset,
                addr.content_offset.saturating_sub(1),
                addr.content_offset)
        } else {
            "Click on text to test hit-testing".to_string()
        };
        let status_attrs = TextAttrs {
            color: Color::rgb(1.0, 0.9, 0.3),
            ..Default::default()
        };
        let status_shaped = engine.shape(status_text, &status_attrs);
        let status = TextLayout::from_shaped(&status_shaped, 20.0, 120.0);
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
