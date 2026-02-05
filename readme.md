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
│  strata            Custom GPU rendering engine (Iced, WGPU)   │
│    GPU pipeline, glyph atlas, text engine (cosmic-text)       │
│                                                                 │
│  nexus-ui          Nexus shell frontend (depends on strata)    │
│    ├─ nexus_app       App shell, state, subscriptions          │
│    ├─ nexus_widgets   Nexus-specific widget implementations    │
│    └─ blocks/         Command output blocks (structured + PTY) │
└────────────┬───────────────────────────┬────────────────────────┘
             │                           │
┌────────────▼──────────────┐ ┌──────────▼────────────────────────┐
│  SHELL INTERPRETER        │ │  AGENT SYSTEM                     │
│  nexus-kernel             │ │  nexus-agent                      │
│    ├─ Parser (tree-sitter)│ │    ├─ Agentic loop & tool system  │
│    ├─ Evaluator (AST)     │ │    ├─ Session management          │
│    ├─ 110+ native commands│ │    └─ ACP protocol integration    │
│    ├─ Job control         │ │                                   │
│    └─ SQLite history      │ │  Composed of:                     │
│                           │ │    nexus-llm       Multi-provider │
│  Execution paths:         │ │      (Anthropic, OpenAI, Ollama,  │
│    Kernel → Value output  │ │       Vertex, Groq, Mistral,      │
│    PTY    → raw terminal  │ │       Cerebras, OpenRouter, ...)   │
│                           │ │    nexus-executor  Cmd execution  │
│                           │ │    nexus-fs        File ops       │
│                           │ │    nexus-web       Web & search   │
│                           │ │    nexus-sandbox   Policy enforce │
└────────────┬──────────────┘ └──────────┬────────────────────────┘
             │                           │
┌────────────▼───────────────────────────▼────────────────────────┐
│  INFRASTRUCTURE                                                 │
│  nexus-api         Shared types: Value, ShellEvent, BlockMeta  │
│    Value = Bool | Int | Float | String | Bytes | Path          │
│            | List | Table | Record | FileEntry | Process       │
│            | GitStatus | GitCommit | Media | ...               │
│                                                                 │
│  nexus-term        Headless terminal emulation                 │
│    ANSI parsing via alacritty_terminal, virtual grid for TUIs  │
│                                                                 │
│  nexus-sandbox     macOS Seatbelt, read-only/workspace policies │
└─────────────────────────────────────────────────────────────────┘

Data flow:
  Input → Parser → CommandClassification
    ├─ Kernel path → Evaluator → Native cmd → Value → ShellEvent
    └─ PTY path    → Subprocess → nexus-term (ANSI) → Grid → ShellEvent
  All ShellEvents → tokio broadcast → UI subscription → GPU render
```

**Native commands** (ls, git, ps, etc.) return structured `Value` types.
**Legacy commands** (vim, htop, etc.) run in a PTY with full terminal emulation.
**The UI** renders both seamlessly through Strata, a custom GPU rendering engine built on Iced with its own pipeline, glyph atlas, and text shaping via cosmic-text.

## Status

**Working:**
- Bash-compatible parsing (pipes, loops, redirects, subshells)
- 110+ native commands with structured output
- Native git commands (status, log, branch, diff, add, commit, remote, stash)
- Session persistence (SQLite — history with full-text search, blocks)
- Tilde expansion, glob expansion
- PTY for external commands
- Terminal emulation for TUI apps (vim, htop)
- Output persistence and `|` continuation
- AI agent with 13 LLM providers, tool use, and ACP protocol

**In Progress:**
- Interactive table rendering (click to sort/filter)
- Semantic actions (right-click context menus)

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

# Run tests with coverage and open HTML report
cargo llvm-cov --open

# Coverage for a specific package
cargo llvm-cov -p nexus-kernel --open

# Generate lcov report (for CI integration)
cargo llvm-cov --lcov --output-path lcov.info
```

### Strata Demo

**Strata** is its own workspace crate — a custom GPU rendering engine with its own pipeline and text shaping, built on Iced's windowing infrastructure. To run its demo:

```bash
cargo run -p nexus-ui -- --demo
```

## License

MIT
