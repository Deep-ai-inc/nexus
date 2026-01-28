//! Nexus Application Entry Point
//!
//! This module is a thin coordinator that:
//! - Configures the Iced application (window, theme)
//! - Delegates update logic to the orchestrator
//! - Delegates view rendering to the view module
//!
//! # Module Structure
//! - `orchestrator`: Message routing and cross-domain logic
//! - `view`: UI composition and rendering

use iced::{Subscription, Task, Theme};

use crate::msg::Message;
use crate::state::Nexus;

mod orchestrator;
mod view;

// Re-exports for backwards compatibility and public API
pub use crate::blocks::{Block, PtyEvent, UnifiedBlock};
pub use crate::constants::{CHAR_WIDTH_RATIO, LINE_HEIGHT_FACTOR};
pub use crate::msg::{GlobalShortcut, ZoomDirection};

// Re-export perform_buffer_search for Action processing
pub use orchestrator::perform_buffer_search;

/// Run the Nexus application.
pub fn run() -> iced::Result {
    iced::application("Nexus", update, view::view)
        .subscription(subscription)
        .theme(|_| Theme::Dark)
        .window_size(iced::Size::new(1200.0, 800.0))
        .antialiasing(true)
        .run_with(|| {
            let focus_task = iced::widget::focus_next();
            (Nexus::default(), focus_task)
        })
}

/// The update function - delegates to orchestrator.
fn update(state: &mut Nexus, message: Message) -> Task<Message> {
    orchestrator::update(state, message)
}

/// The subscription function - delegates to orchestrator.
fn subscription(state: &Nexus) -> Subscription<Message> {
    orchestrator::subscription(state)
}
