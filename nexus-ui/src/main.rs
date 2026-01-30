//! Nexus - The converged shell runtime.
//!
//! Main entry point for the Iced-based GUI.
//!
//! Flags:
//!   --strata       Launch the Strata-based Nexus UI
//!   --strata-demo  Launch the Strata demo/playground UI

use tracing_subscriber::EnvFilter;

fn main() -> iced::Result {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--strata") {
        tracing::info!("Starting Nexus (Strata)");
        nexus_ui::strata::nexus_app::run().map_err(strata_err)
    } else if args.iter().any(|a| a == "--strata-demo") {
        tracing::info!("Starting Strata demo");
        nexus_ui::strata::demo::run().map_err(strata_err)
    } else {
        tracing::info!("Starting Nexus shell");
        nexus_ui::app::run()
    }
}

fn strata_err(e: nexus_ui::strata::shell::Error) -> iced::Error {
    match e {
        nexus_ui::strata::shell::Error::Iced(ice) => ice,
    }
}
