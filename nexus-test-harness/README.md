# nexus-test-harness

Integration test suite for the nexus remote agent. Tests SSH connectivity,
session persistence, resume, ring buffer replay, and network chaos scenarios
against a real SSH server running in Docker.

## Prerequisites

- Docker Desktop running
- Agent binary built: `./scripts/build-agent.sh x86_64`

## Quick Start

```bash
# Run all tests
cargo test -p nexus-test-harness

# Run a specific slice
cargo test -p nexus-test-harness --test slice2_happy_path -- --nocapture

# Run a single test with debug output
RUST_LOG=info cargo test -p nexus-test-harness --test slice2_happy_path connect_and_execute_echo -- --nocapture
```

## Docker Image

The test image (`nexus-test-sshd`) is built automatically on first test run.
To force rebuild after changing the Dockerfile:

```bash
docker rmi nexus-test-sshd
# Next test run rebuilds it
```

### Manual rebuild

```bash
docker build --no-cache -t nexus-test-sshd nexus-test-harness/docker/
```

## SSH Key

The test SSH key is at `docker/id_test` (private) and `docker/id_test.pub`.
If you regenerate it, you MUST rebuild the Docker image:

```bash
ssh-keygen -t ed25519 -f nexus-test-harness/docker/id_test -N ""
docker rmi nexus-test-sshd
```

## Manual SSH Testing

These are the exact commands that work with the test container:

```bash
# Start a test container (ephemeral port)
CID=$(docker run -d --cap-add NET_ADMIN -p 0:22 nexus-test-sshd)
PORT=$(docker port $CID 22 | head -1 | awk -F: '{print $NF}')

# SSH into the container
ssh -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null \
    -o LogLevel=ERROR \
    -o BatchMode=yes \
    -p $PORT \
    -i nexus-test-harness/docker/id_test \
    testuser@127.0.0.1 "echo hello"

# Deploy and run the agent manually
docker cp ~/.nexus/agents/nexus-agent-x86_64-unknown-linux-musl \
    $CID:/home/testuser/.nexus/nexus-agent
docker exec $CID chmod +x /home/testuser/.nexus/nexus-agent
docker exec $CID chown testuser:testuser /home/testuser/.nexus/nexus-agent

# Run agent via SSH (protocol on stdin/stdout, logs to stderr)
ssh -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null \
    -o LogLevel=ERROR \
    -o BatchMode=yes \
    -p $PORT \
    -i nexus-test-harness/docker/id_test \
    testuser@127.0.0.1 \
    "RUST_LOG=info /home/testuser/.nexus/nexus-agent 2>>/tmp/nexus-agent.log"

# Check agent version
ssh ... testuser@127.0.0.1 "/home/testuser/.nexus/nexus-agent --protocol-version"

# View agent logs inside container
docker exec $CID cat /tmp/nexus-agent.log

# Check running processes
docker exec $CID ps aux

# Check UDS sockets
docker exec $CID ls -la /home/testuser/.nexus/agent-*.sock

# Network chaos commands (require --cap-add NET_ADMIN)
docker exec $CID iptables -A INPUT -p tcp --dport 22 -j DROP   # blackhole inbound
docker exec $CID iptables -A OUTPUT -p tcp --sport 22 -j DROP  # blackhole outbound
docker exec $CID iptables -F                                    # restore network

# Shrink TCP timeouts for fast failure detection
docker exec $CID sysctl -w net.ipv4.tcp_retries2=3
docker exec $CID sysctl -w net.ipv4.tcp_keepalive_time=1
docker exec $CID sysctl -w net.ipv4.tcp_keepalive_intvl=1
docker exec $CID sysctl -w net.ipv4.tcp_keepalive_probes=2

# Kill sshd session (server-side disconnect)
# Alpine sshd process names: "sshd: testuser [priv]", "sshd: testuser@notty"
docker exec $CID pkill -f "testuser@"

# Freeze/unfreeze sshd (half-open TCP simulation)
docker exec $CID pkill -STOP -f "testuser@"
docker exec $CID pkill -CONT -f "testuser@"

# Add latency + packet loss
docker exec $CID tc qdisc add dev eth0 root netem delay 200ms loss 5%
docker exec $CID tc qdisc del dev eth0 root   # remove

# Cleanup
docker rm -f $CID
```

## Alpine Gotchas

- **Locked accounts**: Alpine's `adduser -D` creates a locked account. sshd rejects
  locked accounts even with valid pubkey auth. Fix: `passwd -u testuser` in Dockerfile.
- **SGID bit on home dirs**: Alpine sets SGID on `/home/testuser`. sshd is strict about
  `.ssh/` permissions. Fix: explicit `chmod 700 /home/testuser/.ssh` in Dockerfile.
- **Process names**: sshd child processes show as `sshd: testuser@notty` (not
  `sshd: testuser` like Ubuntu). Use `pkill -f "testuser@"` to target them.
- **No host keys by default**: Unlike Ubuntu, Alpine doesn't auto-generate SSH host keys.
  Fix: `ssh-keygen -A` in Dockerfile.

## Test Slices

| Slice | File | What it tests |
|-------|------|---------------|
| 1 | `slice1_infra.rs` | Docker lifecycle, SSH reachability, agent deploy |
| 2 | `slice2_happy_path.rs` | Connect, execute commands, verify pong |
| 3 | `slice3_chaos.rs` | Blackhole, kill sshd, agent persistence |
| 4 | `slice4_resume.rs` | Resume after disconnect, PTY survival, ring buffer replay, CWD sync |

## Known Issues / TODO

- `slice3_chaos::kill_sshd_session_triggers_disconnect`: `pkill` pattern needs
  adjustment for Alpine's sshd process naming — use `pkill -f "testuser@"` instead
  of `pkill -f 'sshd:.*testuser'`
- Agent builtins (like `echo`) emit `CommandOutput` events, not `StdoutChunk`.
  The test client handles both, but use `/bin/echo` if you need raw byte output.
- RTT may be 0ms for local Docker — don't assert `rtt_ms > 0`.
- Tests run in parallel (each gets its own container with ephemeral port).
  Don't hardcode ports.

## Architecture

```
TestEnv::start()
  ├── Build Docker image (cached)
  ├── Start container (ephemeral port, NET_ADMIN cap)
  ├── Wait for sshd ready
  ├── Deploy agent binary
  └── Shrink TCP timeouts

TestClient::connect(env)
  ├── SSH to container, launch agent
  ├── Hello handshake
  ├── Spawn event bridge + ping loop
  └── Return connected client

TestClient::resume(env, instance_id, token, last_seq)
  ├── SSH with --attach {instance_id}
  ├── Resume handshake (via UDS to persisting agent)
  └── Ring buffer replay + terminal snapshots

NetworkControl
  ├── blackhole() — iptables DROP
  ├── restore() — iptables flush
  ├── degrade(delay_ms, loss_pct) — tc netem
  ├── kill_sshd_session() — pkill
  ├── freeze/unfreeze_sshd_session() — SIGSTOP/SIGCONT
  └── is_agent_alive() / agent_socket_exists()
```
