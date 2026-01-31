//! Nexus - The converged shell runtime.
//!
//! Main entry point for the GPU-accelerated Nexus UI.
//!
//! Flags:
//!   --demo  Launch the Strata demo/playground UI

use tracing_subscriber::EnvFilter;

fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--demo") {
        tracing::info!("Starting Strata demo");
        nexus_ui::strata::demo::run().map_err(strata_err)
    } else {
        tracing::info!("Starting Nexus");
        nexus_ui::strata::nexus_app::run().map_err(strata_err)
    }
}

fn strata_err(e: nexus_ui::strata::shell::Error) -> iced::Error {
    match e {
        nexus_ui::strata::shell::Error::Iced(ice) => ice,
    }
}
