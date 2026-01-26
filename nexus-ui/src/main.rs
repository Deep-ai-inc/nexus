//! Nexus - The converged shell runtime.
//!
//! Main entry point for the Iced-based GUI.

mod agent_adapter;
mod agent_block;
mod agent_widgets;
mod app;
mod glyph_cache;
mod pty;
mod shell_context;
mod theme;
mod widgets;

use tracing_subscriber::EnvFilter;

fn main() -> iced::Result {
    // Set up logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting Nexus shell");

    app::run()
}
