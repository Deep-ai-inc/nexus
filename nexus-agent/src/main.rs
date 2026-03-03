//! Nexus Agent - Headless remote kernel with wire protocol.
//!
//! Reads `Request` from stdin, writes `Response` to stdout.
//! Deployed to remote machines and wrapped around a `nexus-kernel::Kernel`.

mod agent;
mod pty;
mod relay;
mod session;

use std::process;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

fn main() {
    // Set subreaper on Linux so orphaned grandchildren are reparented to us
    #[cfg(target_os = "linux")]
    unsafe {
        // Best-effort: fails gracefully in Docker PID 1 or restricted namespaces
        let _ = libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
    }

    // Parse CLI args
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--version") {
        println!(
            "nexus-agent {} (protocol v{})",
            env!("CARGO_PKG_VERSION"),
            nexus_protocol::PROTOCOL_VERSION
        );
        process::exit(0);
    }

    if args.iter().any(|a| a == "--protocol-version") {
        println!("{}", nexus_protocol::PROTOCOL_VERSION);
        process::exit(0);
    }

    // Parse idle timeout
    let idle_timeout_secs: u64 = args
        .iter()
        .position(|a| a == "--idle-timeout")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .or_else(|| std::env::var("NEXUS_AGENT_IDLE_TIMEOUT").ok()?.parse().ok())
        .unwrap_or(7 * 24 * 3600); // 7 days default

    // Ensure TERM is set — the agent is launched via `ssh host agent-path`
    // without a TTY, so TERM is unset. Child processes (top, vim, etc.) need it.
    if std::env::var("TERM").is_err() {
        // SAFETY: called before any threads are spawned (single-threaded at this point).
        unsafe { std::env::set_var("TERM", "xterm-256color") };
    }

    // Initialize tracing to stderr (stdout is the protocol channel)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    // Build and run the async runtime
    let rt = tokio::runtime::Runtime::new().expect("failed to create tokio runtime");
    if let Err(e) = rt.block_on(run(idle_timeout_secs)) {
        tracing::error!("agent exited with error: {e}");
        process::exit(1);
    }
}

async fn run(idle_timeout_secs: u64) -> Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let mut agent = agent::Agent::new(idle_timeout_secs)?;
    agent.run(stdin, stdout).await
}
