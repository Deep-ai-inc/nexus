//! Nexus Agent - Headless remote kernel with wire protocol.
//!
//! Reads `Request` from stdin, writes `Response` to stdout.
//! Deployed to remote machines and wrapped around a `nexus-kernel::Kernel`.

mod agent;
mod pty;
mod relay;
mod session;

use std::process;
use std::time::Duration;

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

    // --attach mode: raw byte pipe to a persisting agent via UDS
    let attach_id = args
        .iter()
        .position(|a| a == "--attach")
        .and_then(|i| args.get(i + 1).cloned());

    // Parse idle timeout
    let idle_timeout_secs: u64 = args
        .iter()
        .position(|a| a == "--idle-timeout")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .or_else(|| {
            std::env::var("NEXUS_AGENT_IDLE_TIMEOUT")
                .ok()?
                .parse()
                .ok()
        })
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

    if let Some(instance_id) = attach_id {
        if let Err(e) = rt.block_on(attach(&instance_id)) {
            tracing::error!("attach failed: {e}");
            process::exit(1);
        }
    } else if let Err(e) = rt.block_on(run(idle_timeout_secs)) {
        tracing::error!("agent exited with error: {e}");
        process::exit(1);
    }
}

async fn run(idle_timeout_secs: u64) -> Result<()> {
    let mut agent = agent::Agent::new(idle_timeout_secs)?;

    // First connection: stdin/stdout (SSH pipe)
    agent
        .run(tokio::io::stdin(), tokio::io::stdout())
        .await?;

    // If we have an active relay or running PTY sessions, persist via UDS
    while agent.should_persist() {
        let sock_path = uds_path(agent.instance_id());

        // Clean up stale socket from a previous SIGKILL'd agent
        std::fs::remove_file(&sock_path).ok();

        let listener = tokio::net::UnixListener::bind(&sock_path)?;
        tracing::info!("persisting via UDS at {sock_path}");

        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _)) => {
                        // Socket file stays on disk — do NOT remove it here.
                        // If this connection drops, the loop re-binds (after remove_file above).
                        drop(listener);
                        let (read_half, write_half) = stream.into_split();
                        agent.run(read_half, write_half).await?;
                    }
                    Err(e) => {
                        tracing::error!("UDS accept failed: {e}");
                        break;
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(idle_timeout_secs)) => {
                tracing::info!("idle timeout, shutting down");
                break;
            }
        }
    }

    // Graceful exit: clean up socket file
    std::fs::remove_file(uds_path(agent.instance_id())).ok();
    Ok(())
}

/// --attach mode: transparent byte pipe between stdin/stdout and a persisting
/// agent's UDS socket. No FrameCodec — the protocol is already being spoken
/// between the client and the running agent.
async fn attach(instance_id: &str) -> Result<()> {
    let sock_path = uds_path(instance_id);
    let stream = tokio::net::UnixStream::connect(&sock_path).await?;
    let (mut stream_read, mut stream_write) = stream.into_split();
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    tokio::select! {
        r = tokio::io::copy(&mut stdin, &mut stream_write) => {
            r.map(|_| ()).map_err(Into::into)
        }
        r = tokio::io::copy(&mut stream_read, &mut stdout) => {
            r.map(|_| ()).map_err(Into::into)
        }
    }
}

/// Compute the UDS socket path for a given agent instance.
fn uds_path(instance_id: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    format!("{home}/.nexus/agent-{instance_id}.sock")
}
