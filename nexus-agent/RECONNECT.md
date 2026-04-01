# Agent Persistence & Reconnection Architecture

## Problem

When a user's laptop sleeps, changes IP, roams between networks, or has a flaky
connection, the SSH tunnel carrying the Nexus protocol dies. Without persistence,
the remote agent dies with it — all running PTYs, shell state, and environment
are lost. The user sees "Connection lost — remote agent restarted".

## Goal

Make SSH disruptions invisible. Running commands and PTY sessions survive any
network interruption. When the client reconnects, it resumes exactly where it
left off — no lost output, no restarted processes.

## Architecture Overview

```
┌─────────┐    SSH tunnel     ┌─────────────┐
│  Client  │◄────────────────►│ nexus-agent  │
│ (nexus-ui)│                  │  (remote)    │
└─────────┘                   └──────┬───────┘
                                     │ owns
                              ┌──────┴───────┐
                              │  PTY sessions │
                              │  Kernel state │
                              │  Ring buffer  │
                              │  UDS listener │
                              └──────────────┘
```

The key insight: the agent process outlives the SSH connection. The UDS listener
is bound **immediately on startup** — not after disconnect. A new SSH session
runs `nexus-agent --attach <id>` which connects to the UDS and acts as a
transparent byte pipe. If the old SSH connection is still technically alive
(dead TCP, no FIN), the UDS connection triggers an instant hot-swap via
cancellation token.

## Agent Lifecycle

### 1. Startup & Process Isolation

The agent detaches from the SSH session at startup to survive its death:

```
setsid()                    — new session leader, no process group signals from SSH
signal(SIGHUP, SIG_IGN)    — belt-and-suspenders, some SSHd variants send this
signal(SIGTERM, SIG_IGN)    — some SSHd send SIGTERM on KillTimeout
PR_SET_CHILD_SUBREAPER      — (Linux) reparent orphaned grandchildren to us
PR_SET_PDEATHSIG(0)         — (Linux) clear "kill when parent dies" flag
panic hook                  — log to stderr instead of aborting
```

### 2. UDS Socket Binding (Immediate)

Before the first connection, the agent:
1. Creates `~/.nexus/` with mode 0700 (owner-only access)
2. Sweeps all `agent-*.sock` files — probes each with a 1-second connect
   timeout, unlinks dead ones (from previously SIGKILL'd agents)
3. Binds its own socket at `~/.nexus/agent-{instance_id}.sock` with mode 0600

This happens **before** the first `run()` call, so reconnecting clients can
reach the agent even while the old SSH connection hasn't timed out yet.

### 3. First Connection (stdin/stdout over SSH)

```
ssh remote nexus-agent
    └─ agent reads Request from stdin, writes Response to stdout
    └─ Hello handshake → session_token issued, instance_id generated
    └─ Main request loop with 120s read timeout + CancellationToken
    └─ Concurrent: UDS accept() watches for takeover
```

The agent runs a `select!` between `run_cancellable()` and UDS `accept()`.
If a new client connects on the UDS while the SSH pipe is still open, the
cancellation token fires, `run_cancellable()` breaks immediately, and the
agent swaps to serving the UDS client. The old stdin/stdout handles are
dropped (closing the SSH pipe), and the sender/collector tasks are `.abort()`ed
and `.await`ed to ensure prompt cleanup.

### 4. Disconnection Detection

The agent detects dead connections through three mechanisms:

- **UDS takeover (instant)**: A reconnecting client connects to the UDS socket.
  The agent immediately cancels the current connection and switches. This is
  the fastest path — no timeout needed.

- **Read timeout (120s)**: If no request arrives within 120 seconds, the agent
  assumes the connection is dead and breaks out of the main loop. Fallback for
  when no client reconnects before the timeout.

- **Write failure**: All response writes use `is_err() { break }` instead of
  `?` propagation. A broken pipe on write immediately exits the loop.

The 120s timeout is calibrated against the client's ping interval (~30s). Four
missed pings = definitely dead.

### 5. Persistence Loop

After `run()` returns, the agent enters a persistence loop:

```
while should_persist() {
    select! {
        accept UDS connection → run_with_uds_takeover() again
        idle timeout (7 days) → exit
    }
}
```

`should_persist()` returns true if:
- There's an active relay to a nested child agent, OR
- There are running PTY sessions, OR
- A session token exists (i.e., at least one client successfully connected)

Clean shutdown (`Request::Shutdown`) and unnesting (`Request::Unnest`) clear the
session token so the agent doesn't persist unnecessarily.

### 6. Reconnection (--attach mode)

When the client reconnects, it runs:
```
ssh remote nexus-agent --attach {instance_id}
```

The `--attach` process is a minimal byte pipe:
```
UDS socket ◄──► stdin/stdout (SSH tunnel)
```

No protocol parsing — just `tokio::io::copy` in both directions. The real
protocol conversation happens between the client and the persisting agent
through this pipe.

### 7. Client-Side Reconnection

The client uses a two-phase approach per attempt:

**Phase 1 — Resume**: Send `Request::Resume { session_token, last_seen_seq }`
through the `--attach` pipe. If the persisting agent validates the token, it
replays missed events from its ring buffer and sends terminal snapshots for
active PTYs. The client seamlessly continues — no "connection lost" message.

**Phase 2 — Fresh Hello**: If Resume fails (agent died, token invalid), start a
fresh agent with `Request::Hello`. Orphaned blocks get a "Connection lost"
message. This is the fallback, not the normal path.

Retry schedule: 20 attempts with delays [1, 2, 4, 8, 15, 15, 15, ...] seconds.
Total window: ~5.5 minutes.

## SSH Hardening

The client's SSH commands include:

```
-o ConnectTimeout=10        — fail fast if server unreachable (not 60s+ default)
-o ServerAliveInterval=15   — client-side keepalive every 15s
-o ServerAliveCountMax=3    — 3 missed = dead (45s detection)
```

`ServerAliveInterval` is the client-side complement to the agent's read timeout.
It ensures the SSH process itself dies quickly on network loss, rather than
waiting for TCP keepalive.

## Security

- `~/.nexus/` directory: mode 0700 (owner-only)
- Socket files: mode 0600 (owner-only)
- Session token: 128-bit random, validated on Resume

UDS sockets grant unauthenticated access to terminal sessions. Directory and
file permissions prevent other users on shared machines from connecting.

## Ring Buffer & Overrun Detection

The agent maintains a 1 MB ring buffer of recent outbound events. On Resume,
the client sends `last_seen_seq` and the agent replays everything after it.

If the ring buffer has wrapped (e.g., a noisy build process output 5 MB while
the laptop was asleep), the agent detects the gap:
`last_seen_seq < oldest_buffered_seq`. It sends
`SessionState { events_lost: true }` so the client can warn the user that
some scrollback output was lost.

Terminal snapshots are always sent after replay, correcting the visual state
of interactive PTYs regardless of buffer overrun.

## Zombie Process Reaping

Because `PR_SET_CHILD_SUBREAPER` is set, orphaned grandchildren are reparented
to the agent. A background task calls `waitpid(-1, WNOHANG)` every 30 seconds
to reap them.

This can race with Tokio's own child waiter (for PTY processes). The PTY waiter
handles `ECHILD` gracefully — if the zombie reaper happens to reap a PTY child
first, the waiter treats it as exit code 0 rather than a fatal error.

## Transport Handle Cleanup

When a UDS takeover or disconnection occurs, the agent:
1. Cancels the token → `run_cancellable()` breaks immediately
2. Aborts collector and sender tasks
3. Awaits both tasks to ensure they drop their `Arc<FrameWriter>` references
4. Function returns, dropping `reader` (which owns the old transport)

This ensures the old SSH pipe's stdin/stdout are closed promptly, preventing
leaked SSH daemon processes on the remote host.

## Failure Scenarios

### Laptop sleep (minutes to hours)
1. TCP connection goes dead silently
2. Laptop wakes → client starts reconnection attempts
3. `--attach` connects to UDS → UDS takeover cancels old dead connection
4. Resume succeeds → ring buffer replay + terminal snapshots
5. **Instant reconnection** — no waiting for 120s timeout

### IP address change (WiFi roaming, VPN toggle)
Same as laptop sleep — UDS takeover handles it instantly.

### Brief network blip (< 45s)
TCP retransmits may save the connection. If the blip exceeds
ServerAliveInterval × ServerAliveCountMax (45s), the client's SSH dies,
triggering reconnection via UDS takeover.

### SSH server kills connection (ClientAliveInterval)
Agent's `read()` returns `ConnectionClosed` immediately. No timeout needed.

### Agent process killed (SIGKILL, OOM)
Unrecoverable. The stale socket is cleaned up by the next agent that starts
(socket sweep on startup). Phase 2 starts a fresh agent.

### Ring buffer overrun
Client warned via `events_lost: true`. PTY visual state restored via snapshots.
Only scrollback text output has a gap.

### Poor/lossy connection
TCP handles retransmission. Credit-based flow control prevents overwhelming
slow links. Severe loss degrades to the "brief network blip" scenario.

### Nested sessions (agent → agent → agent)
Each agent in the chain has the same persistence behavior. If a middle agent
loses its parent connection, it persists via UDS while maintaining its child
relay. When the parent reconnects, the entire chain resumes.

## What This Doesn't Handle

- **Agent machine reboots**: The agent process dies. Fresh start required.
- **Disk full**: UDS bind may fail. Agent exits.
- **Firewall blocks reconnection**: Client retries exhaust. Fresh start required.
- **Clock skew**: Not relevant — timeouts use monotonic clocks.
