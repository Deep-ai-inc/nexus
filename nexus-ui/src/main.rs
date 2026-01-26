//! Nexus - The converged shell runtime.
//!
//! Main entry point for the Iced-based GUI.

use tracing_subscriber::EnvFilter;

fn main() -> iced::Result {
    // Set up logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    tracing::info!("Starting Nexus shell");

    nexus_ui::app::run()
}
