//! Nexus - The converged shell runtime.
//!
//! Main entry point for the GPU-accelerated Nexus UI.
//!
//! Flags:
//!   --demo  Launch the Strata demo/playground UI

use tracing_subscriber::EnvFilter;

fn main() -> strata::shell::Result {
    let args: Vec<String> = std::env::args().collect();

    // Hidden subcommand: `nexus mcp-proxy --port <PORT>`
    // Spawned by the Claude CLI as an MCP stdio server for permission prompts.
    if args.iter().any(|a| a == "mcp-proxy") {
        let port = args
            .windows(2)
            .find(|w| w[0] == "--port")
            .and_then(|w| w[1].parse::<u16>().ok())
            .expect("usage: nexus mcp-proxy --port <PORT>");
        nexus_ui::features::agent::mcp::run(port);
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    if args.iter().any(|a| a == "--demo") {
        tracing::info!("Starting Strata demo");
        strata::demo::run()?;
    } else {
        tracing::info!("Starting Nexus");
        nexus_ui::app::run()?;
    }
    Ok(())
}
