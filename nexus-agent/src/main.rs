// Binary-only modules (not part of the library)
mod app;
mod cli;
mod logging;

// Re-export from library for use in binary modules
pub use nexus_agent::{
    acp, agent, config, mcp, permissions, persistence, session, tools, types, ui, utils,
};

use crate::cli::{Args, Mode};
use crate::logging::setup_logging;
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Handle list commands first
    if args.handle_list_commands()? {
        return Ok(());
    }

    match args.mode {
        Some(Mode::Server { verbose }) => app::server::run(verbose).await,
        Some(Mode::Acp {
            verbose,
            path,
            model,
            tool_syntax,
            use_diff_format,
            sandbox_mode,
            sandbox_network,
        }) => {
            // Ensure the path exists and is a directory
            if !path.is_dir() {
                anyhow::bail!("Path '{}' is not a directory", path.display());
            }

            let model_name = Args::resolve_model_name(model)?;

            let config = app::AgentRunConfig {
                path,
                task: None,
                continue_task: false,
                model: model_name.clone(),
                tool_syntax,
                use_diff_format,
                record: None,
                playback: None,
                fast_playback: false,
                sandbox_policy: sandbox_mode.to_policy(sandbox_network),
            };

            app::acp::run(verbose, config).await
        }
        None => {
            // TODO: Integrate with Nexus Iced UI
            // For now, just run in ACP mode with default settings
            setup_logging(args.verbose, false);

            if !args.path.is_dir() {
                anyhow::bail!("Path '{}' is not a directory", args.path.display());
            }

            let model_name = args.get_model_name()?;
            let sandbox_policy = args.sandbox_policy();

            let config = app::AgentRunConfig {
                path: args.path,
                task: args.task,
                continue_task: args.continue_task,
                model: model_name,
                tool_syntax: args.tool_syntax,
                use_diff_format: args.use_diff_format,
                record: args.record,
                playback: args.playback,
                fast_playback: args.fast_playback,
                sandbox_policy,
            };

            // Placeholder - will integrate with Iced UI
            eprintln!("Note: UI modes (gpui/terminal) removed. Use --mode acp or integrate with nexus-ui.");
            app::acp::run(args.verbose > 0, config).await
        }
    }
}
