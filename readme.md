# Nexus

**A modern shell where the interpreter, data pipeline, and GUI are one unified system.**

Nexus inverts the Unix philosophy: instead of small text-processing tools composed via pipes, we absorb core utilities (git, curl, ls, ps, etc.) into the shell itself. Commands return structured data. The terminal understands that data. Everything clicks together.

## The Vision

### Interactive Data Exploration
- `ls -l` returns a Table → click column headers to sort, type to filter, resize columns
- `ps aux` → click a row to expand details, right-click PID → Kill
- Nested records expand/collapse like a JSON viewer
- Pagination that actually works (not `| less`)

### Semantic Actions
- File paths are clickable → Open, Reveal in Finder, Copy Path
- URLs open in browser
- Git commits → Checkout, Cherry-pick, Show Diff
- Error messages → Jump to file:line in editor

### Output Transformation Without Re-running
- Command finished? Re-sort, re-filter, export as JSON/CSV from the UI
- `| grep foo` at a new prompt pipes from the previous output instantly
- Copy a table → pastes as proper TSV/CSV

### Rich Visualizations
- `du | chart pie` → actual pie chart, not ASCII art
- `history | chart timeline` → visual command history
- Real progress bars, sparklines, inline images

### AI That Understands Structure
- "Find the largest files" operates on the actual Table, not parsing text
- AI sees `{name: "foo.rs", size: 1024, modified: ...}` not `-rw-r--r-- 1024 foo.rs`
- Smarter suggestions because it knows the data types

### Blocks as Objects
- Re-run a block with different args
- Collapse/expand output
- Name and reference outputs from previous commands
- Potentially undo destructive commands (we know what they did)

## Philosophy: Absorbed Utilities

Traditional shells spawn external processes and parse their text output. Nexus absorbs commonly-used tools as **native commands** that return structured data:

| Traditional | Nexus Native | Returns |
|-------------|--------------|---------|
| `/usr/bin/ls` | `ls` | `Table { name, size, modified, permissions, ... }` |
| `/usr/bin/git status` | `git status` | `Record { branch, staged, unstaged, untracked }` |
| `/usr/bin/curl` | `http GET url` | `Record { status, headers, body }` (parsed JSON if applicable) |
| `/usr/bin/ps` | `ps` | `Table { pid, name, cpu, memory, ... }` |
| `/usr/bin/find` | `find` | `List<Path>` with metadata |
| `/usr/bin/docker ps` | `docker ps` | `Table { id, image, status, ports, ... }` |

This isn't about reimplementing everything - it's about **owning the data**. When `git log` returns actual commit objects, the UI can render them richly, offer contextual actions, and pipe them meaningfully to other commands.

External commands still work. They just produce bytes/text like they always have.

## Syntax: POSIX-Compatible, Secretly Upgraded

Nexus looks and feels like bash. No new language to learn.

```bash
# Normal commands work
ls -la
cd ~/projects
echo "hello world"

# Pipes work, but data is structured underneath
ls | grep ".rs" | head -5

# Start a line with | to continue from previous output (no re-execution)
ls -la
| grep ".rs"
| sort size

# Everything else: variables, loops, redirects, globs, tilde expansion
for f in *.txt; do echo $f; done
cat ~/.config/nexus/config.toml > backup.toml
```

The structured data is invisible until you want it. The UI reveals it through sorting, filtering, clicking, and rich rendering.

## Quick Start

```bash
cargo build
cargo run -p nexus-ui --release
```

## Architecture

```
nexus/
├── nexus-kernel/     # Shell interpreter: parser, evaluator, process management
│   ├── parser/       # Bash-compatible parser (tree-sitter)
│   ├── eval/         # AST walker, expansion (tilde, glob, brace)
│   ├── commands/     # Native commands (ls, git, http, ps, ...)
│   └── process/      # PTY for external commands
├── nexus-ui/         # Terminal GUI (Iced framework)
│   ├── app.rs        # Main application, event loop
│   ├── block.rs      # Command blocks with rich output rendering
│   └── components/   # Tables, records, charts, semantic renderers
├── nexus-api/        # Shared types: Value, ShellEvent, etc.
└── nexus-term/       # ANSI parsing for legacy command output
```

## Current Status

**Working:**
- Bash-compatible parsing (pipes, lists, loops, subshells)
- 65+ native commands with structured output
- Tilde expansion (`~`, `~/path`, `~user`)
- Glob expansion (`*.rs`, `file?.txt`, `[a-z]*`)
- PTY for external/legacy commands
- Terminal emulation for TUI apps (vim, htop)

**Next:**
- Clickable/interactive table rendering
- Semantic actions (right-click paths, URLs, PIDs)
- `| continuation` syntax for instant re-filtering
- Native `git` commands with structured output
- Native `http` command

## Development

```bash
cargo test              # Run tests (170+)
cargo clippy            # Lint
cargo fmt               # Format
RUST_LOG=debug cargo run -p nexus-ui  # Debug logging
```

## License

MIT
