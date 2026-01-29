//! Demo application to test Strata shell.
//!
//! Run with: `cargo run -p nexus-ui --example strata_demo`
//! Or temporarily replace main() to call `strata::demo::run()`

use crate::strata::{
    AppConfig, Command, LayoutSnapshot, Selection, StrataApp, Subscription,
};

/// Demo message type.
#[derive(Debug, Clone)]
pub enum DemoMessage {
    Tick,
}

/// Demo application state.
pub struct DemoState {
    tick_count: u64,
}

/// Demo application.
pub struct DemoApp;

impl StrataApp for DemoApp {
    type State = DemoState;
    type Message = DemoMessage;

    fn init() -> (Self::State, Command<Self::Message>) {
        let state = DemoState { tick_count: 0 };
        (state, Command::none())
    }

    fn update(state: &mut Self::State, message: Self::Message) -> Command<Self::Message> {
        match message {
            DemoMessage::Tick => {
                state.tick_count += 1;
            }
        }
        Command::none()
    }

    fn view(_state: &Self::State, _snapshot: &mut LayoutSnapshot) {
        // For now, nothing to render - just testing the shell
        // TODO: Register demo content with snapshot
    }

    fn selection(_state: &Self::State) -> Option<&Selection> {
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
