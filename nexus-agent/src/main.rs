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
    // === Process isolation ===
    // Detach from the SSH session entirely so no signals from the parent
    // session (SIGHUP, SIGTERM, etc.) reach us. setsid() makes this process
    // its own session leader. Ignore SIGHUP/SIGTERM as a belt-and-suspenders
    // defense (setsid can fail if we're already a session leader).
    #[cfg(unix)]
    unsafe {
        let _ = libc::setsid(); // new session — detach from SSH process group
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
        libc::signal(libc::SIGPIPE, libc::SIG_IGN); // prevent death on write to dead SSH pipe
    }

    // Linux-specific hardening
    #[cfg(target_os = "linux")]
    unsafe {
        // Set subreaper so orphaned grandchildren are reparented to us
        // Best-effort: fails gracefully in Docker PID 1 or restricted namespaces
        let _ = libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0);
        // Clear "kill me when parent dies" flag (usually unset, but defensive)
        let _ = libc::prctl(libc::PR_SET_PDEATHSIG, 0, 0, 0, 0);
    }

    // Install panic handler that logs instead of aborting.
    // The agent must survive panics in spawned tasks to keep relays/PTYs alive.
    std::panic::set_hook(Box::new(|info| {
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown".to_string()
        };
        let location = info.location().map_or_else(
            || "unknown location".to_string(),
            |loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()),
        );
        eprintln!("nexus-agent panic at {location}: {payload}");
    }));

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

    // Parse read timeout (for dead connection detection)
    let read_timeout_secs: u64 = std::env::var("NEXUS_AGENT_READ_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);

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
    } else if let Err(e) = rt.block_on(run(idle_timeout_secs, read_timeout_secs)) {
        tracing::error!("agent exited with error: {e}");
        process::exit(1);
    }
}

async fn run(idle_timeout_secs: u64, read_timeout_secs: u64) -> Result<()> {
    let mut agent = agent::Agent::new(idle_timeout_secs, read_timeout_secs)?;

    // Ensure ~/.nexus/ directory exists with restrictive permissions (0700).
    // UDS sockets grant unauthenticated access to terminal sessions — the
    // directory must not be readable by other users on shared machines.
    let sock_path = uds_path(agent.instance_id());
    if let Some(parent) = std::path::Path::new(&sock_path).parent() {
        std::fs::create_dir_all(parent).ok();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }

    // Sweep all stale agent-*.sock files from previous crashed agents
    cleanup_stale_sockets(&sock_path).await;

    let listener = tokio::net::UnixListener::bind(&sock_path)?;
    tracing::info!("UDS listener bound at {sock_path}");

    // Set socket file permissions to 0600 (owner only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600));
    }

    // Spawn zombie reaper (needed because we set PR_SET_CHILD_SUBREAPER)
    let reaper_interval_secs: u64 = std::env::var("NEXUS_AGENT_REAPER_INTERVAL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    #[cfg(target_os = "linux")]
    let _reaper = tokio::spawn(zombie_reaper(agent.tokio_pids(), agent.reaped_statuses(), reaper_interval_secs));

    // First connection: stdin/stdout (SSH pipe).
    // Concurrently accept UDS connections — if a new client arrives on the
    // UDS while the SSH pipe is still technically alive (dead TCP, no FIN),
    // we immediately swap to the UDS connection.
    run_with_uds_takeover(&mut agent, tokio::io::stdin(), tokio::io::stdout(), &listener).await;

    // Persistence loop: accept reconnections on UDS.
    while agent.should_persist() {
        tracing::info!("persisting, waiting for reconnection via UDS");

        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, _)) => {
                        let (read_half, write_half) = stream.into_split();
                        run_with_uds_takeover(&mut agent, read_half, write_half, &listener).await;
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
    std::fs::remove_file(&sock_path).ok();
    Ok(())
}

/// Run the agent on the given transport, but also listen for incoming UDS
/// connections. If a new UDS client connects, cancel the current `run()` and
/// immediately start a new one on the UDS stream. This handles the case where
/// the old TCP connection is dead but the agent hasn't timed out yet — the
/// reconnecting client triggers an instant swap.
///
/// IMPORTANT: `run_cancellable` must always run to completion so it can clean
/// up spawned tasks and save the ring buffer / seq counter. We use `select!`
/// only to detect the UDS arrival, then always `await` the agent future so
/// its cleanup runs.
async fn run_with_uds_takeover<R, W>(
    agent: &mut agent::Agent,
    reader: R,
    writer: W,
    listener: &tokio::net::UnixListener,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let cancel = tokio_util::sync::CancellationToken::new();

    let uds_cancel = cancel.clone();
    let uds_accept = async {
        match listener.accept().await {
            Ok((stream, _)) => {
                tracing::info!("UDS takeover: new client connected, dropping current transport");
                uds_cancel.cancel();
                Some(stream)
            }
            Err(e) => {
                tracing::error!("UDS accept error during takeover watch: {e}");
                None
            }
        }
    };

    // Run agent and UDS accept concurrently. We must ensure run_cancellable
    // always completes (for ring buffer/task cleanup), so we can't just drop
    // the future when UDS wins the select.
    let pending_stream = {
        let agent_fut = agent.run_cancellable(reader, writer, cancel);
        tokio::pin!(agent_fut);
        tokio::pin!(uds_accept);

        let mut pending_stream = None;

        tokio::select! {
            _ = &mut agent_fut => {
                // Normal exit — cleanup already ran inside run_cancellable.
            }
            stream = &mut uds_accept => {
                pending_stream = stream;
                // Cancel was fired; now let run_cancellable finish its cleanup
                // (abort spawned tasks, save ring buffer & seq counter).
                // Timeout: if the old transport is blocked on a dead TCP write,
                // don't let it hang the new connection forever.
                let _ = tokio::time::timeout(
                    Duration::from_secs(5),
                    agent_fut,
                ).await;
            }
        }

        pending_stream
        // agent_fut and uds_accept are dropped here, releasing &mut agent
    };

    if let Some(stream) = pending_stream {
        let (read_half, write_half) = stream.into_split();
        agent.run(read_half, write_half).await.ok();
    }
}

/// Sweep all stale agent-*.sock files in the ~/.nexus/ directory.
/// Probes each socket — if a live agent responds, leaves it alone.
/// This cleans up sockets from agents that died ungracefully (SIGKILL, OOM).
async fn cleanup_stale_sockets(own_sock_path: &str) {
    let parent = match std::path::Path::new(own_sock_path).parent() {
        Some(p) => p,
        None => return,
    };

    let entries = match std::fs::read_dir(parent) {
        Ok(e) => e,
        Err(_) => return,
    };

    // Collect all candidate sockets, then probe concurrently to avoid
    // O(n × 1s) startup delay when many stale sockets exist.
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if name.starts_with("agent-") && name.ends_with(".sock") {
            candidates.push(path);
        }
    }

    let mut set = tokio::task::JoinSet::new();
    for path in candidates {
        set.spawn(async move {
            let is_alive = tokio::time::timeout(
                Duration::from_secs(1),
                tokio::net::UnixStream::connect(&path),
            ).await.map_or(false, |r| r.is_ok());

            if is_alive {
                tracing::debug!("socket {} is live — skipping", path.display());
            } else {
                tracing::info!("removing stale socket {}", path.display());
                std::fs::remove_file(&path).ok();
            }
        });
    }
    while set.join_next().await.is_some() {}
}

/// Periodically reap orphaned zombie processes without stealing exit statuses
/// from PIDs that Tokio is actively waiting on (PTY children, relay children).
///
/// Strategy: attempt `waitpid(-1, WNOHANG)` to find any dead child. If the
/// reaped PID is Tokio-managed, we stole its exit status — save it to the
/// shared `reaped_statuses` map so the PTY waiter can recover it on ECHILD.
/// This is a very rare race (reaper runs every 30s, child exit is instant).
#[cfg(target_os = "linux")]
async fn zombie_reaper(
    tokio_pids: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<u32>>>,
    reaped_statuses: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<u32, i32>>>,
    interval_secs: u64,
) {
    loop {
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
        loop {
            let mut status: i32 = 0;
            let result = unsafe { libc::waitpid(-1, &mut status, libc::WNOHANG) };
            if result <= 0 {
                break; // No more zombies (0) or error (-1 ECHILD)
            }

            let pid = result as u32;
            let exit_code = libc::WEXITSTATUS(status);
            if tokio_pids.lock().unwrap().contains(&pid) {
                // Rare race: we stole a Tokio-managed PID's exit status.
                // Save it so the PTY waiter can recover the real exit code.
                tracing::warn!(
                    "zombie reaper accidentally reaped Tokio-managed pid {pid} \
                     (exit code {exit_code}); saving for PTY waiter"
                );
                reaped_statuses.lock().unwrap().insert(pid, exit_code);
            } else {
                tracing::debug!("reaped orphan zombie pid {pid}");
            }
        }
    }
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
