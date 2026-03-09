# Nexus

**A terminal that fixes what's broken.**

The command line is the bedrock of modern computing, yet it relies on protocols designed for hardware that hasn't existed for decades. Escape codes from the 1970s. Keybindings that conflict with every other application. Copy/paste that's a security risk. No images, no discoverability, no progress bars.

Nexus removes decades of accumulated friction by unifying the shell, terminal, and GUI into one system.

## The Problems We're Solving

### Legacy Baggage

| Problem | How Nexus Fixes It |
|---------|-------------------|
| **Escape Sequence Hell** — Parsing ANSI codes feels like 1970s programming | Commands return structured data. The UI renders it directly. No escape codes to parse or generate. |
| **Inconsistent Keybindings** — Ctrl+C kills here, copies there | Native GUI app. Standard keybindings work. Ctrl+C copies text. Process interruption is separate. |
| **The TERM Variable** — Apps break if this magic string is wrong | We control the terminal emulation. It's always correct. You never think about it. |
| **Unicode Chaos** — Emojis break layouts, cursors drift | Modern text layout engine. Character widths calculated correctly. No drift. |
| **Copy/Paste Friction** — Shift-click, invisible newlines, security risks | Native clipboard. Tables copy as TSV. Pasted newlines don't auto-execute. |

### User Experience

| Problem | How Nexus Fixes It |
|---------|-------------------|
| **No Discoverability** — Staring at a blank void | Command palette (Cmd+K). Inline help. Contextual suggestions. Browsable command list. |
| **Poor Mouse Support** — Which layer am I scrolling? | It's a GUI. Click to select. Right-click for actions. Drag to resize. Scroll works everywhere. |
| **Text Reflow Chaos** — Resize window, text scrambles | We control layout. Tables resize intelligently. Text wraps correctly. |
| **No Images or Media** — Leave terminal to see a chart | Images render inline. Charts are native. Media is a first-class data type. |
| **Config File Theming** — Edit text files to change fonts | Native settings. Pick fonts from a list. Live preview. Drag and drop. |

### Shell Interaction

| Problem | How Nexus Fixes It |
|---------|-------------------|
| **Dangerous Globs** — Typo deletes wrong files, no undo | Preview matches before executing: "These 15 files will be affected." |
| **The Sudo Trap** — Permission denied, retype everything | Detect the error, offer "Re-run with sudo?" One click. |
| **Argument Anarchy** — `-f`, `--file`, `f`, all different | Native commands use consistent `--flag` style. Help shown inline. |
| **Silent Commands** — Is `cp` frozen or working? | Built-in progress indicators. Spinners. Status in the UI. |
| **History Amnesia** — Limited, unsearchable, lost between tabs | SQLite-backed. Infinite. Full-text search. Synced across all sessions. |

### Modern Workflows

| Problem | How Nexus Fixes It |
|---------|-------------------|
| **Buffer Limits** — 10,000 lines of logs, terminal remembers 1,000 | Everything persisted. Scroll back forever. Search all output. |
| **Dumb Autocomplete** — Suggests files when you need git branches | Context-aware. Knows the command, suggests relevant completions. |
| **Accessibility Gaps** — Screen readers can't parse ncurses | Native GUI with proper accessibility APIs. Works with assistive tech. |

## How It Works

Nexus looks like a normal terminal. Bash-compatible syntax. Your muscle memory works.

```bash
# Everything you know still works
ls -la
cd ~/projects
grep -r "TODO" .
git status

# But data is structured underneath
ls | sort size | head 5    # Actually sorting by size, not text

# Previous output is remembered
ls -la
| grep ".rs"               # Pipes from previous output instantly
| wc -l                    # No re-execution needed
```

The difference is invisible until you need it:
- Click a column header to sort
- Right-click a file path → Open, Copy, Reveal in Finder
- Right-click a PID → Kill Process
- Resize the window, tables reflow properly
- Scroll back through yesterday's session

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  PRESENTATION                                                   │
│  strata            Custom GPU rendering engine (Metal)          │
│    GPU pipeline, glyph atlas, text engine (cosmic-text)       │
│                                                                 │
│  nexus-ui          Nexus shell frontend (depends on strata)    │
│    ├─ nexus_app       App shell, state, subscriptions          │
│    ├─ nexus_widgets   Nexus-specific widget implementations    │
│    └─ blocks/         Command output blocks (structured + PTY) │
└────────────┬───────────────────────────┬────────────────────────┘
             │                           │
┌────────────▼──────────────┐ ┌──────────▼────────────────────────┐
│  SHELL INTERPRETER        │ │  AI & REMOTE                      │
│  nexus-kernel             │ │                                   │
│    ├─ Parser (tree-sitter)│ │  AI agent (Claude Code CLI)       │
│    ├─ Evaluator (AST)     │ │    ├─ NDJSON event streaming      │
│    ├─ 100+ native commands│ │    ├─ Permission prompt routing   │
│    ├─ Job control         │ │    └─ Tool result rendering       │
│    └─ SQLite history      │ │                                   │
│                           │ │  nexus-agent       Remote shell   │
│  Execution paths:         │ │    ├─ Auto-deployed via SSH       │
│    Kernel → Value output  │ │    ├─ Runs nexus-kernel remotely  │
│    PTY    → raw terminal  │ │    └─ Binary protocol (msgpack)   │
│    Remote → nexus-agent   │ │                                   │
└────────────┬──────────────┘ └──────────┬────────────────────────┘
             │                           │
┌────────────▼───────────────────────────▼────────────────────────┐
│  INFRASTRUCTURE                                                 │
│  nexus-api         Shared types: Value, ShellEvent, BlockMeta  │
│    Value = Bool | Int | Float | String | Bytes | Path          │
│            | List | Table | Record | FileEntry | Process       │
│            | GitStatus | GitCommit | Media | ...               │
│                                                                 │
│  nexus-protocol    Binary wire protocol (client ↔ remote agent)│
│    Framed msgpack, flow control, session resume, priority lanes │
│                                                                 │
│  nexus-term        Headless terminal emulation                 │
│    ANSI parsing via alacritty_terminal, virtual grid for TUIs  │
└─────────────────────────────────────────────────────────────────┘

Data flow:
  Input → Parser → CommandClassification
    ├─ Kernel path → Evaluator → Native cmd → Value → ShellEvent
    ├─ PTY path    → Subprocess → nexus-term (ANSI) → Grid → ShellEvent
    └─ Remote path → SSH/Docker/kubectl → nexus-agent → nexus-protocol → ShellEvent
  All ShellEvents → tokio broadcast → UI subscription → GPU render
```

**Native commands** (ls, ps, etc.) return structured `Value` types.
**Legacy commands** (vim, htop, etc.) run in a PTY with full terminal emulation.
**Remote commands** run on a deployed agent via `nexus-protocol`, returning the same structured types over SSH/Docker/kubectl.
**The UI** renders all three seamlessly through Strata, a custom GPU rendering engine with its own Metal pipeline, glyph atlas, and text shaping via cosmic-text.

## Status

**Working:**
- Bash-compatible parsing (pipes, loops, redirects, subshells)
- 100+ native commands with structured output
- Session persistence (SQLite — history with full-text search, blocks)
- Tilde expansion, glob expansion
- PTY for external commands
- Terminal emulation for TUI apps (vim, htop)
- Output persistence and `|` continuation
- AI agent via Claude Code CLI with tool use and permissions
- Remote shells via SSH, Docker, kubectl with auto-deployed agent
- Connection progress overlay with real-time upload tracking
- Interactive table rendering (click to sort/filter)
- Semantic actions (right-click context menus)

## Remote Shells — `ssh`, `docker exec`, `kubectl exec`

Nexus extends its structured shell to remote machines. Type `ssh user@host` and Nexus automatically deploys a lightweight agent binary, establishing a full structured session on the remote — not a dumb PTY pipe.

```bash
ssh root@gpu-server          # deploys agent, connects with progress overlay
docker exec -it my-container # same structured shell inside Docker
kubectl exec my-pod          # and Kubernetes pods
```

**What happens under the hood:**

1. **Agent deployment** — Nexus detects the remote architecture (`uname -m`), finds the matching agent binary, and uploads it over SSH stdin in 64KB chunks with a real-time progress bar. Version-keyed by protocol hash so multiple Nexus versions coexist.
2. **Structured protocol** — The agent runs the same kernel as the local shell. Commands return typed `Value` objects (tables, file entries, git status) over a binary protocol — not escape codes. Native commands work identically on the remote.
3. **Connection progress** — A native GUI overlay shows each stage: checking agent → detecting architecture → uploading (with progress bar) → connecting → connected. Ctrl+C cancels at any point.
4. **Nesting** — Run `ssh` inside an SSH session. Each level gets its own agent. Breadcrumb bar shows the full chain. `exit` pops one level.
5. **Reconnection** — If the connection drops, Nexus detects it and can re-establish the session. The agent persists on the remote with a 7-day idle timeout.

```
┌─────────────────────────────────────────────┐
│ ● $ ssh root@gpu-server                     │
│                                             │
│   ⠹ Uploading agent...                     │
│     12.5 MB                                 │
│   ████████████░░░░░░░░  62%                │
└─────────────────────────────────────────────┘
```

**Agent binary locations:**
- Local: `~/.nexus/agents/nexus-agent-{target}` (e.g. `nexus-agent-x86_64-unknown-linux-musl`)
- Remote: `~/.nexus/agent-{protocol_version}`

Cross-compile the agent:
```bash
docker run --rm -v "$(pwd)":/src -w /src messense/rust-musl-cross:x86_64-musl \
  cargo build --release -p nexus-agent --target x86_64-unknown-linux-musl
cp target/x86_64-unknown-linux-musl/release/nexus-agent \
  ~/.nexus/agents/nexus-agent-x86_64-unknown-linux-musl
```

Supported targets: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `armv7-unknown-linux-musleabihf`. Use the matching `messense/rust-musl-cross:{arch}-musl` Docker image.

**After modifying agent or protocol code**, you must cross-compile, install, and rebuild the client:
```bash
# 1. If the wire format changed (header, message schema), bump PROTOCOL_VERSION
#    in nexus-protocol/src/lib.rs — otherwise the deploy skips upload
# 2. Cross-compile
docker run --rm -v "$(pwd)":/src -w /src messense/rust-musl-cross:x86_64-musl \
  cargo build --release -p nexus-agent --target x86_64-unknown-linux-musl
# 3. Install to deploy location
cp target/x86_64-unknown-linux-musl/release/nexus-agent \
  ~/.nexus/agents/nexus-agent-x86_64-unknown-linux-musl
# 4. Rebuild client
cargo build -p nexus-ui
```

The deploy system checks the remote agent's protocol version via `--protocol-version`. If it matches the client's `PROTOCOL_VERSION`, the upload is skipped. Forgetting to bump the version after a breaking wire change causes "failed to read HelloOk: connection closed" — the old agent can't parse the new frame format.

## Reactive Streaming Pipelines — `watch`

`watch` is a shell keyword that re-executes a typed pipeline on an interval, streaming live updates to the UI.

```bash
watch ps aux | sort -r --by %cpu | head 5   # live top-5 by CPU, structured table
watch -n 1 df                                # 1s disk usage refresh
watch -n 500ms ps aux | sort -r --by %mem | head 10   # 500ms memory monitor
watch docker ps                              # external commands work too (raw text)
watch -n 1 -- ls -la                         # -- separates watch flags from command
```

This is not Unix `watch`. Unix `watch` reruns a string and repaints characters. Nexus `watch` operates inside the typed execution engine:

- **Typed value graph** — `watch ps aux | sort -r --by %cpu | head 5` flows `Table<Process>` between stages, not bytes. The UI renders a native table with sortable, clickable columns.
- **Zero-copy structured handoff** — Native stages pass `Value` objects directly. No serialize/deserialize round-trip.
- **Coalesced streaming** — `StreamingUpdate(coalesce=true)` replaces the previous state atomically. The UI re-renders a structured widget, not a screen repaint.
- **Pipeline-aware** — The kernel knows the semantics of each stage. External commands (`watch docker ps`) fall back to stdout capture with ANSI preservation.

Interval syntax: bare number = seconds (`-n 1`), suffix = explicit (`-n 500ms`, `-n 2s`). Default is 2 seconds. Cancel with Ctrl+C.

## AppleScript / Automation

Nexus exposes its window and session state via Cocoa Scripting, so external tools (dashboards, window managers, scripts) can query and control it — the same way iTerm2 does.

**Requirements:** Nexus must be running as a `.app` bundle (not a bare binary) for macOS to load the scripting dictionary.

```bash
# Build and package
./scripts/package-app.sh          # debug build
./scripts/package-app.sh --release  # release build
open target/Nexus.app
```

The hierarchy matches iTerm2: **window → tab → session**. Nexus currently has one tab per window, so the tab layer is always `tab 1`.

### Reading state

```applescript
tell application "Nexus"
    get every window                                  -- {window id 1, ...}
    get id of window 1                                -- 1
    get name of window 1                              -- "Nexus — ~/projects"
    get bounds of window 1                            -- {x1, y1, x2, y2}
    get index of window 1                             -- 1 (frontmost)

    get every tab of window 1                         -- {tab 1}
    get every session of tab 1 of window 1            -- sessions
    get tty of session 1 of tab 1 of window 1         -- "/dev/ttys003"
    get cwd of session 1 of tab 1 of window 1         -- "/Users/kevin/projects"
    get columns of session 1 of tab 1 of window 1     -- 120
    get rows of session 1 of tab 1 of window 1        -- 36
    get command of session 1 of tab 1 of window 1           -- "cargo build" or ""
    get is busy of session 1 of tab 1 of window 1     -- true/false
end tell
```

### Writing state

```applescript
tell application "Nexus"
    set bounds of window 1 to {100, 100, 900, 700}   -- {x1, y1, x2, y2}
    set index of window 1 to 1                        -- raise to front
    activate                                          -- bring app to foreground
end tell
```

Bounds use iTerm2's `{x1, y1, x2, y2}` convention (origin + opposite corner), not `{x, y, width, height}`.

### Session properties

| Property | Type | Description |
|----------|------|-------------|
| `id` | text | Stable session identifier (e.g. `"window-1"`) |
| `tty` | text | PTY device path (e.g. `/dev/ttys003`), empty for builtins |
| `name` | text | Session display name (OSC title or command) |
| `cwd` | text | Current working directory |
| `columns` | integer | Terminal column count |
| `rows` | integer | Terminal row count |
| `command` | text | Currently executing command, or empty |
| `is busy` | boolean | Whether a command is actively running |
| `profile name` | text | Reserved for future use |

## Quick Start

```bash
cargo build --release
cargo run -p nexus-ui --release
```

## Development

### Tests

```bash
cargo test                              # run all tests
cargo test -p nexus-kernel              # run kernel tests only
cargo test -p nexus-kernel -- watch     # run tests matching "watch"
```

### Code Coverage

We use `cargo-llvm-cov` for code coverage. It uses native LLVM instrumentation built into the Rust compiler — fast and accurate on Apple Silicon.

```bash
# Install (one-time)
cargo install cargo-llvm-cov

# Quick summary of all crates
./coverage-summary.sh

# Detailed per-file coverage for all crates
./coverage-by-file.sh

# Detailed coverage for a specific crate
./coverage-by-file.sh nexus-kernel

# Run tests with coverage and open HTML report
cargo llvm-cov --open

# Coverage for a specific package
cargo llvm-cov -p nexus-kernel --open

# Generate lcov report (for CI integration)
cargo llvm-cov --lcov --output-path lcov.info
```

**Current coverage targets:** 25% minimum for `nexus-ui` and `strata`.

### Strata Demo

**Strata** is its own workspace crate — a custom GPU rendering engine with its own Metal pipeline, text shaping, and native macOS windowing. To run its demo:

```bash
cargo run -p nexus-ui -- --demo
```

## License

MIT
