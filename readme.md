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
nexus/
├── nexus-kernel/     # Shell interpreter (bash-compatible parser, evaluator)
├── nexus-ui/         # Native GUI (Iced framework)
├── nexus-api/        # Structured data types (Value, Events)
└── nexus-term/       # Legacy command support (PTY, ANSI parsing)
```

**Native commands** (ls, git, ps, etc.) return structured data (`Value` types).
**Legacy commands** run in a PTY with proper terminal emulation.
**The UI** renders both seamlessly.

## Status

**Working:**
- Bash-compatible parsing (pipes, loops, redirects, subshells)
- 65+ native commands with structured output
- Tilde expansion, glob expansion
- PTY for external commands
- Terminal emulation for TUI apps (vim, htop)
- Output persistence and `|` continuation

**In Progress:**
- Interactive table rendering (click to sort/filter)
- Semantic actions (right-click context menus)
- Native git commands
- Session persistence (SQLite)

## Quick Start

```bash
cargo build --release
cargo run -p nexus-ui --release
```

### Strata GUI Library Demo

Nexus includes **Strata**, a custom GPU-accelerated GUI library. To run the demo:

```bash
cargo run -p nexus-ui --example strata_demo
```

## License

MIT
