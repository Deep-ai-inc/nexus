You're right. Let me restart with a proper specification document.

---

# Nexus: The Converged Shell Runtime
## Complete Technical Specification v1.0

---

# Executive Summary

Nexus is a ground-up reimplementation of the terminal experience. It is not a terminal emulator wrapping a shell—it *is* the shell, with the GUI as a first-class participant in command execution, data flow, and user interaction.

We accept hard constraints: we cannot parse `.bashrc`, we cannot spy on arbitrary process I/O without being the middleman, and we cannot magically understand remote systems without cooperation. Within these constraints, we build the most ambitious command-line environment ever attempted.

The core insight: by owning the shell interpreter, the pipeline infrastructure, and the rendering layer, we can make the command line *legible* without sacrificing its power.

---

# Part I: The Shell Kernel

## 1. Language Specification

### 1.1 Target Language

Nexus implements a POSIX-compliant `sh` interpreter with strategic Bash extensions.

**POSIX sh compliance (mandatory):**
- All control structures: `if`/`then`/`elif`/`else`/`fi`, `for`/`do`/`done`, `while`/`until`, `case`/`esac`
- All redirections: `>`, `>>`, `<`, `<<`, `2>&1`, `>&2`, file descriptor manipulation up to fd 9
- Pipelines and lists: `|`, `&&`, `||`, `;`, `&`
- Full parameter expansion: `$var`, `${var}`, `${var:-default}`, `${var:=default}`, `${var:+alt}`, `${var:?error}`, `${#var}`, `${var%pattern}`, `${var%%pattern}`, `${var#pattern}`, `${var##pattern}`
- Command substitution: `$(command)` and legacy backticks
- Arithmetic expansion: `$((expression))`
- Here documents and here strings
- Subshells: `(commands)`
- Functions: `name() { commands; }`
- All POSIX-mandated builtins

**Bash extensions (the "Big Three"):**
- Indexed arrays: `arr=(a b c)`, `${arr[0]}`, `${arr[@]}`, `${#arr[@]}`
- Associative arrays: `declare -A map; map[key]=value`
- Extended test: `[[ ]]` with pattern matching, regex `=~`, and logical operators
- Process substitution: `<(command)` and `>(command)`

**Nexus-specific extensions:**
- Block references: `%1`, `%latest`, `%-1` expanding to previous command outputs
- Inline structured data: `@json{...}` literals for passing structured data to commands

**Explicit non-goals:**
- `shopt` options and Bash-specific shell option behaviors
- Bash 4+ features: `|&`, `coproc`, nameref variables
- Ksh compatibility features
- Zsh-specific syntax
- Any feature requiring `BASH_VERSION` checks in scripts

### 1.2 Parser Architecture

The parser uses Tree-sitter for incremental parsing.

**Rationale:**
- Incremental parsing enables real-time feedback on every keystroke without re-parsing the entire input
- Error recovery produces partial ASTs for incomplete or invalid input, enabling intelligent suggestions even mid-edit
- The query system enables structural pattern matching for syntax highlighting and semantic analysis
- Community-maintained grammars reduce maintenance burden

**Grammar definition:** A custom `tree-sitter-nexus-sh` grammar extending POSIX sh with the supported Bash extensions and Nexus-specific syntax.

**Parse outputs:**
- Concrete Syntax Tree (CST): Full fidelity including whitespace, comments, and error nodes
- Abstract Syntax Tree (AST): Simplified tree for interpretation, discarding formatting
- Error diagnostics: Source locations and descriptions for all syntax errors

### 1.3 Tiered Analysis System

Analysis occurs in three distinct tiers with explicit confidence levels. Each tier is independent—failures in higher tiers never affect lower tiers.

**Tier 1: Syntax Analysis**

- **Scope:** Grammar validation only
- **Mechanism:** Tree-sitter incremental parsing
- **Latency target:** <1ms for typical edits
- **Outputs:** Syntax errors (unclosed quotes, invalid redirections, malformed expansions)
- **Confidence:** 100%—we define the grammar
- **UI feedback:** Red squiggles under invalid syntax, error messages in tooltip

**Tier 2: Semantic Analysis**

- **Scope:** Name resolution against shell state
- **Mechanism:** Queries against live environment, alias table, function table, and PATH cache
- **Latency target:** <5ms
- **Outputs:** Command classification (builtin, alias, function, external, unknown), variable existence checks, alias expansion previews
- **Confidence:** High for shell state, medium for PATH (filesystem may change)
- **UI feedback:** Syntax highlighting by category, "unknown command" warnings, hover tooltips showing resolved values

**Tier 3: Intelligence Analysis**

- **Scope:** Argument semantics and documentation
- **Mechanism:** Provider plugins (see Part V)
- **Latency target:** <50ms (async, non-blocking)
- **Outputs:** Argument documentation, flag validation, completion candidates, "did you mean?" suggestions
- **Confidence:** Varies by provider—provenance is always displayed
- **UI feedback:** Inline argument hints, completion menus, warning annotations

**Failure isolation:** A provider crash displays an error toast but does not affect syntax highlighting or command execution.

### 1.4 Interpreter Design

The interpreter is an AST-walking evaluator implemented in Rust.

**Rationale for AST walking over bytecode/JIT:**
- Shell scripts are I/O bound; interpreter overhead is negligible compared to process spawning
- AST walking is simpler to implement correctly and debug
- Direct AST access enables better error messages with source locations
- No warm-up time or compilation pauses

**Execution model:**

1. Input text is parsed into an AST
2. The AST is walked recursively
3. Each node type has a corresponding evaluation function
4. Side effects (process spawning, redirections, variable assignments) occur during evaluation
5. Exit status propagates up the tree

**Word expansion order** (POSIX-mandated sequence):

1. Brace expansion: `{a,b,c}` → `a b c`
2. Tilde expansion: `~` → home directory
3. Parameter expansion: `$var`, `${var:-default}`
4. Command substitution: `$(cmd)`
5. Arithmetic expansion: `$((1+2))`
6. Field splitting: based on `$IFS`
7. Pathname expansion (globbing): `*.txt`
8. Quote removal

### 1.5 Shell State

The shell maintains the following canonical state:

**Environment variables:** Key-value string pairs that are exported to child processes.

**Shell variables:** Key-value pairs (scalar, indexed array, or associative array) that are not exported.

**Functions:** Named AST nodes that can be invoked as commands.

**Aliases:** String-to-string mappings expanded during parsing (not during execution).

**Job table:** Active jobs indexed by job ID, containing process group ID, constituent processes, status, and original command text.

**Working directory:** Canonical absolute path.

**Shell options:** Boolean flags controlling shell behavior (e.g., `errexit`, `nounset`).

**Exit status:** Integer result of the last command (0-255).

**Positional parameters:** Arguments to the current function or script.

**Special variables:** `$$` (shell PID), `$!` (last background PID), `$?` (last exit status), `$-` (current options).

### 1.6 Event Bus

All state mutations emit typed events to a central event bus.

**Event categories:**

- **Environment events:** Variable set, variable unset, variable changed
- **Directory events:** Working directory changed (with trigger: cd, pushd, popd, external)
- **Function events:** Function defined, function undefined
- **Alias events:** Alias set, alias unset
- **Job events:** Job created, job state changed (running, stopped, done), job removed
- **Execution events:** Command started (with block ID, command text, timestamp), command finished (with exit status, duration)
- **Signal events:** Signal received

**Subscriber model:**

- The event bus supports multiple subscribers
- Subscribers receive events via bounded channels
- Slow subscribers do not block the shell—events are dropped if the channel is full
- Each subscriber maintains its own view model derived from events

**Primary subscribers:**

- UI thread: Maintains rendering state
- History system: Records commands and outputs
- Plugin host: Notifies providers of state changes

### 1.7 Configuration

Configuration uses TOML files.

**Configuration file location:** `~/.config/nexus/config.toml`

**Configuration schema:**

- **shell:** History size, history deduplication, autocd, glob options
- **env:** Environment variable overrides
- **path:** Directories to prepend/append to PATH
- **aliases:** Alias definitions
- **prompt:** Prompt format string with expansion tokens (git status, CWD, etc.)
- **keys:** Keybinding overrides
- **ui:** Theme, font, cursor style, scrollback limit
- **providers:** List of providers to enable

**Rationale for TOML:**
- Human-readable and easy to edit
- Well-supported across editors
- No runtime evaluation needed—pure data
- Familiar format (used by Cargo, pyproject.toml, etc.)

**Environment migration ("Harvest"):**

On first run, Nexus offers to import environment variables from the user's existing shell:

1. Execute the user's login shell with `env` command
2. Compare output against a clean environment baseline
3. Extract user-added variables (especially PATH modifications)
4. Generate initial `config.toml` with these values
5. User can then customize and extend

This is a one-time migration. Users are encouraged to maintain configuration in `config.toml` thereafter.

---

# Part II: Pipeline Architecture

## 2. The Middleman Model

### 2.1 Design Principle

When Nexus executes a pipeline, it interposes itself between every stage. This is the only way to observe data flow without kernel-level mechanisms (eBPF, ptrace).

For a pipeline `A | B | C`:

- Traditional shell: A's stdout connects directly to B's stdin via an OS pipe
- Nexus: A's stdout connects to a Pump thread, which forwards to B's stdin while copying to an observable buffer

### 2.2 Pump Threads

Each connection between pipeline stages is managed by a dedicated Pump thread.

**Pump responsibilities:**

1. Read data from source file descriptor (non-blocking)
2. Write data to destination file descriptor (blocking with backpressure)
3. Copy data to the associated ring buffer (non-blocking, lossy)
4. Feed data to the stream sniffer for format detection
5. Track throughput metrics

**Critical path guarantee:** Writing to the destination is the critical path. The pump never delays writes to the destination for any reason. Buffer copies and sniffing happen after the write succeeds.

**Backpressure handling:**

- If the destination blocks (e.g., slow consumer), the pump blocks on write—this is correct POSIX behavior
- If the ring buffer is full, old data is overwritten—the UI may miss data, but the pipeline is never affected
- If the sniffer is slow, it receives sampled data—analysis quality degrades, but throughput is unaffected

### 2.3 Ring Buffers

Each command's output is captured in a memory-mapped ring buffer.

**Buffer properties:**

- Fixed capacity (configurable, default 1MB per stream)
- Lock-free single-producer writes
- Multiple consumers can read (UI thread, block reference reads)
- Overwrite-on-full semantics

**Metadata tracked:**

- Total bytes written (monotonic, for detecting overwrites)
- Write position (for consumers to detect new data)
- Block ID association
- Timestamp of first and last write

**Overflow indication:** When data is overwritten before the UI consumes it, the block displays a warning: "Output truncated: showing last 1MB of 4.7MB total."

### 2.4 Stream Sniffing

The Pump thread runs lightweight heuristics to detect output format.

**Detection methods:**

1. **Magic bytes:** First 512 bytes checked against known signatures (PNG, JPEG, GIF, PDF, gzip, etc.)
2. **Structure probing:** First complete line checked for JSON object/array, XML declaration
3. **Delimiter frequency:** Tab and comma frequency analyzed for TSV/CSV detection
4. **ANSI detection:** Presence of escape sequences indicates terminal-formatted output

**Detected formats:**

- JSON (single object or array)
- JSON Lines (newline-delimited JSON objects)
- CSV (comma-separated values)
- TSV (tab-separated values)
- XML
- Binary (with subtype: image, PDF, compressed, unknown)
- ANSI text (plain text with escape codes)
- Plain text

**Output:** Format hints are advisory only. The UI offers to enable an appropriate Lens but never forces a view. The raw stream is always accessible.

---

## 3. Block Reference System

### 3.1 Block Reference Syntax

Users can reference previous outputs using shorthand syntax.

**Reference formats:**

- `%N` → Block N's stdout (absolute block ID)
- `%latest` → Most recent block's stdout
- `%-N` → N blocks before current (relative reference)
- `%N:stderr` → Block N's stderr stream
- `%N:meta` → Block N's metadata

**Expansion:** References are expanded during word expansion, after tilde expansion and before parameter expansion. The shell internally retrieves the content from the block store.

**Usage examples:**

- `jq .foo %1` — Parse JSON from block 1
- `diff %1 %2` — Compare outputs of two commands
- `grep error %latest:stderr` — Search recent errors

### 3.2 Block Metadata

Each block tracks:

- **block_id:** Unique identifier
- **command:** Original command text
- **started_at:** ISO 8601 timestamp
- **finished_at:** ISO 8601 timestamp
- **duration_ms:** Execution duration in milliseconds
- **exit_code:** Process exit status
- **cwd:** Working directory at execution time
- **env_snapshot:** Environment variables at execution time (sensitive values redacted)
- **stdout_bytes:** Total bytes written to stdout
- **stderr_bytes:** Total bytes written to stderr
- **detected_format:** Sniffer's format classification
- **truncated:** Boolean indicating if ring buffer overflowed

### 3.3 Block Lifecycle

Blocks are managed by an LRU eviction policy.

**Lifecycle stages:**

1. **Active:** Currently executing; ring buffer in memory
2. **Hot:** Recently completed; ring buffer in memory
3. **Warm:** Older; spilled to disk, metadata in memory
4. **Evicted:** Deleted from disk; only block ID remains for reference error messages

**Eviction triggers:**

- Memory pressure: When total ring buffer memory exceeds limit (default 100MB)
- Disk pressure: When total spilled data exceeds limit (default 1GB)
- Count limit: When block count exceeds limit (default 10,000)

**Eviction order:** Least recently accessed blocks are evicted first. Access includes UI scrollback views and explicit references in commands.

**Spill format:** Blocks are written to `~/.local/share/nexus/blocks/{id}/` containing `stdout`, `stderr`, and `meta.json`.

---

# Part III: TTY and Job Control

## 4. Process Execution

### 4.1 PTY Allocation Rules

Pseudo-terminal allocation follows strict rules based on execution context.

| Context | PTY Allocated | Stdin | Stdout | Stderr |
|:--------|:--------------|:------|:-------|:-------|
| Interactive foreground | Yes | PTY slave | PTY slave | PTY slave |
| Last stage of foreground pipeline | Yes | Pipe from previous | PTY slave | PTY slave |
| Middle stage of pipeline | No | Pipe from previous | Pipe to next | Inherited or pipe |
| Background job (`&`) | No | /dev/null | Pipe to buffer | Pipe to buffer |
| Command substitution `$(...)` | No | Inherited | Pipe (captured) | Inherited |
| Subshell `(...)` | Inherited | Inherited | Inherited | Inherited |
| Process substitution `<(...)` | No | Inherited | Pipe (to FIFO) | Inherited |

**Rationale:** Only processes that need to interact with the user receive a PTY. This matches traditional shell behavior and ensures programs detect interactivity correctly via `isatty()`.

### 4.2 Process Group Management

Nexus implements full POSIX job control.

**Shell initialization:**

1. Shell places itself in its own process group
2. Shell takes control of the terminal (becomes foreground process group)
3. Shell ignores job control signals (SIGINT, SIGQUIT, SIGTSTP, SIGTTIN, SIGTTOU)

**Job creation:**

1. First process in a job becomes process group leader
2. All subsequent processes in the pipeline join that process group
3. Job is registered in the job table with a sequential job ID

**Foreground execution:**

1. Terminal control is transferred to the job's process group
2. Shell waits for the job to complete or stop
3. Terminal control returns to the shell

**Background execution:**

1. Job runs in its own process group without terminal control
2. Shell continues immediately without waiting
3. If background job attempts to read from terminal, it receives SIGTTIN and stops

**Job state transitions:**

- Running → Stopped: SIGTSTP (Ctrl+Z), SIGTTIN, or SIGTTOU
- Running → Done: All processes exited
- Stopped → Running: SIGCONT (via `fg` or `bg` command)
- Any → Terminated: SIGKILL, SIGTERM, etc.

### 4.3 Signal Handling

**Signals ignored by shell:**

- SIGINT: Delivered to foreground job, not shell
- SIGQUIT: Delivered to foreground job, not shell
- SIGTSTP: Delivered to foreground job, not shell
- SIGTTIN: Shell never reads from terminal while job is foreground
- SIGTTOU: Shell never writes to terminal while job is foreground

**Signals handled by shell:**

- SIGCHLD: Child status changed; shell reaps zombies and updates job table
- SIGWINCH: Terminal resized; shell propagates to foreground job and updates UI
- SIGHUP: Terminal closed; shell sends SIGHUP to all jobs and exits
- SIGTERM: Graceful shutdown requested; shell sends SIGTERM to all jobs and exits

**Signal forwarding:** When the user presses Ctrl+C, Ctrl+\, or Ctrl+Z, the terminal driver sends the corresponding signal to the foreground process group. The shell is not in that group, so it doesn't receive the signal.

### 4.4 TUI Detection (Stream Sentinel)

Nexus detects when a program enters full-screen or raw input mode by monitoring the PTY output stream for specific escape sequences.

**Monitored sequences:**

| Sequence | Meaning | Nexus Response |
|:---------|:--------|:---------------|
| `\e[?1049h` | Enter alternate screen buffer | Switch block to fullscreen mode |
| `\e[?1049l` | Leave alternate screen buffer | Return to normal block mode |
| `\e[?1h` | Application cursor keys | Route arrow keys directly to PTY |
| `\e[?1l` | Normal cursor keys | Handle arrow keys for line editing |
| `\e[?1000h` | Enable mouse tracking | Forward mouse events to PTY |
| `\e[?1000l` | Disable mouse tracking | Handle mouse events in UI |
| `\e[?2004h` | Enable bracketed paste | Wrap pastes in bracket sequences |
| `\e[?2004l` | Disable bracketed paste | Paste text directly |

**Fullscreen mode behavior:**

- Block expands to fill the terminal area
- All keyboard input routes directly to PTY (no shell line editing)
- Mouse events forward to PTY if mouse tracking is enabled
- Block output area becomes a pure terminal emulator
- Sidecars and overlays are disabled

**Fallback:** A manual "Raw Mode" toggle in the block header allows users to force fullscreen mode when automatic detection fails (e.g., connecting to systems with unusual terminfo).

### 4.5 Terminal Emulation

When a PTY is allocated, Nexus emulates a terminal for rendering purposes.

**Emulation target:** xterm-256color with common extensions

**Supported features:**

- All standard ANSI escape sequences (cursor movement, colors, clearing)
- 256-color palette and 24-bit true color
- Unicode with proper width handling (East Asian Wide, combining characters, emoji)
- Alternate screen buffer
- Scrollback regions
- Tab stops
- Character sets (UTF-8 primary, with G0/G1 switching for legacy compatibility)
- Window title setting (displayed in block header)
- Cursor styles (block, underline, bar)

**Terminal state tracked per block:**

- Cursor position (row, column)
- Saved cursor position
- Character attributes (foreground, background, bold, italic, underline, etc.)
- Scroll region
- Active character set
- Mode flags (origin mode, autowrap, etc.)
- Tab stops
- Screen buffer (primary and alternate)

---

# Part IV: The User Interface

## 5. Architecture

### 5.1 Rendering Engine

The UI is built on GPUI, a Rust-native GPU-accelerated UI framework.

**Rationale for GPUI over web technologies:**

- Native performance: 120fps rendering without jank
- Direct GPU access: Text rendering, scrolling, and animations are GPU-accelerated
- Memory efficiency: No JavaScript heap, no DOM overhead
- Single binary: No Electron, no bundled browser engine
- Rust integration: Direct access to shell kernel without IPC serialization

**Rendering pipeline:**

1. Shell events arrive via event bus subscription
2. UI thread updates view model (projection of shell state optimized for rendering)
3. View model changes trigger re-layout of affected components
4. GPU renders updated frames

**Target performance:**

- Input latency: <8ms from keypress to screen update
- Scrolling: 120fps with 100,000+ line scrollback
- Memory: <100MB baseline, <500MB with heavy scrollback

### 5.2 View Model

The UI thread maintains its own view model, updated by shell events.

**View model contents:**

- Block list with current scroll position
- Per-block state: content buffer, cursor position, selection, active Lens, collapsed state
- Input line state: text, cursor, selection, completion menu
- Global state: current prompt, working directory, active jobs
- UI state: focused block, panel visibility, theme

**Update model:** Events from the shell are applied to the view model in order. The view model is the single source of truth for rendering—the UI never reads directly from shell state.

**Consistency guarantee:** Because updates are event-driven and ordered, the UI always renders a consistent snapshot. There are no torn reads or race conditions.

### 5.3 Component Hierarchy

**Top-level layout:**

```
┌─────────────────────────────────────────────────────────────┐
│ Tab Bar                                                     │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  Block List (scrollable)                                    │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ Block 1: $ ls -la                                    │   │
│  │ [output...]                                          │   │
│  └─────────────────────────────────────────────────────┘   │
│  ┌─────────────────────────────────────────────────────┐   │
│  │ Block 2: $ git status                                │   │
│  │ [output...]                                          │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│ Input Line                                                  │
│ ┌─────────────────────────────────────────────────────────┐│
│ │ ~/project (main) $ _                                    ││
│ └─────────────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────────────┘
```

**Block structure:**

```
┌─────────────────────────────────────────────────────────────┐
│ [▼] $ command --flags argument          [Raw] [JSON] [⋯]   │  ← Header
├─────────────────────────────────────────────────────────────┤
│                                                             │
│ [Output content rendered according to active Lens]          │  ← Content
│                                                             │
├─────────────────────────────────────────────────────────────┤
│ ✓ exit 0 • 1.23s • 4.5KB                    [Rerun] [Copy] │  ← Footer
└─────────────────────────────────────────────────────────────┘
```

**Header elements:**

- Collapse toggle: Expand/collapse block output
- Command text: Original command (editable for rerun)
- Lens selector: Tabs for available views (Raw, JSON, Table, etc.)
- Actions menu: Block-specific actions

**Footer elements:**

- Exit status indicator: Checkmark (0) or X with code (non-zero)
- Duration: Execution time
- Output size: Bytes written
- Action buttons: Rerun, Copy, Pin, etc.

### 5.4 Block States

Blocks transition through defined states:

**Input:** User is composing a command. Block shows input line with syntax highlighting, completions, and analysis feedback.

**Running:** Command is executing. Block shows live output, progress indication, and stop button.

**Completed:** Command finished. Block shows final output with exit status, duration, and available Lenses.

**Collapsed:** Output hidden. Block shows only header with command text and summary.

**Fullscreen:** TUI application running. Block expands to fill terminal area, pure terminal emulation mode.

**Pinned:** Block persists at top of view, not scrolled away by new output.

## 6. The Lens System

### 6.1 Lens Concept

A Lens is a view transformation applied to block output. The raw byte stream is always preserved; Lenses provide alternative presentations.

**Core properties:**

- **Non-destructive:** Lenses never modify the underlying data
- **Switchable:** Users can change Lenses at any time
- **Fallible:** If a Lens fails to parse, it gracefully degrades to Raw view
- **Provenance-tracked:** Every Lens displays its data source

### 6.2 Built-in Lenses

**Raw Lens:**

- Default view for all output
- Full terminal emulation with ANSI rendering
- Source: stdout/stderr byte stream
- Always available, cannot fail

**JSON Lens:**

- Collapsible tree view of JSON data
- Syntax highlighting for values by type
- Path copying (click node to copy `$.foo.bar[0]`)
- Search within structure
- Source: stdout parsed as JSON
- Availability: When sniffer detects JSON or user forces

**Table Lens:**

- Spreadsheet-style grid view
- Column sorting and filtering
- Column resizing and reordering
- Export to CSV
- Source: stdout parsed as CSV/TSV, or JSON array of objects
- Availability: When sniffer detects tabular data or user forces

**Diff Lens:**

- Side-by-side or unified diff view
- Syntax highlighting for changed lines
- Navigation between hunks
- Source: stdout from diff commands, or comparison of two blocks
- Availability: When output matches diff format or user selects two blocks

**Image Lens:**

- Inline image rendering
- Zoom and pan controls
- Source: stdout detected as image format
- Availability: When sniffer detects image magic bytes

**Hex Lens:**

- Traditional hex dump view with ASCII sidebar
- Offset navigation
- Source: raw bytes
- Availability: Always (user-selectable)

### 6.3 Provenance Display

Every Lens displays its data source in the block header.

**Provenance format:** `Source: {origin} → {transformations}`

**Examples:**

- `Source: stdout` (Raw Lens)
- `Source: stdout → JSON parser` (JSON Lens on command output)
- `Source: Sidecar (git status --porcelain=v2) → git-status parser` (Smart overlay)
- `Source: stdout → JSON parser → Table projection ($.users[*])` (Table from JSON array)

**Rationale:** Users must always know whether they're seeing raw output or an interpretation. This builds trust and aids debugging when transformations produce unexpected results.

### 6.4 Error Handling

When a Lens fails to parse or render, it follows a strict recovery protocol.

**Failure scenarios:**

- JSON Lens receives invalid JSON
- Table Lens receives inconsistent column counts
- Image Lens receives corrupt image data
- Any Lens encounters out-of-memory

**Recovery protocol:**

1. Immediately switch to Raw Lens
2. Display error toast: "JSON parsing failed at line 47: unexpected token"
3. Mark the failed Lens as unavailable for this block
4. Log detailed error for debugging

**Partial failure:** If a streaming parse fails mid-output (e.g., JSON Lines with one malformed line), the Lens shows successfully parsed portions with inline error markers at failure points.

## 7. The Anchor System

### 7.1 Anchor Concept

Anchors are interactive hotspots detected within block output. They enable actions like "click to open file" or "hover to preview."

**Anchor properties:**

- **Position:** Row and column coordinates within the rendered output (not byte offsets)
- **Span:** Start and end columns on a single row
- **Type:** Classification (file path, URL, git ref, IP address, etc.)
- **Data:** Extracted value and any additional context
- **Actions:** Available operations for this anchor type
- **Confidence:** Detection confidence level (provider, structured, heuristic)

### 7.2 Detection Priority

Anchors are detected through three mechanisms with explicit priority:

**Priority 1 - Provider (High Confidence):**

Sidecar commands return explicit anchor definitions with exact positions.

Example: Git status sidecar returns `{"type": "file", "path": "src/main.rs", "row": 3, "col_start": 12, "col_end": 23, "status": "modified"}`

**Priority 2 - Structured (Medium Confidence):**

When output format is known (e.g., command run with `--porcelain` flag), format-specific parsers extract anchors.

Example: `ls -la` output parsed with knowledge of column positions.

**Priority 3 - Heuristic (Low Confidence):**

Regex patterns scan rendered text for common patterns.

Example: URL regex matches `https://...` anywhere in output.

**Display:** Anchor styling indicates confidence level. Provider anchors show solid underlines; heuristic anchors show dotted underlines.

### 7.3 Coordinate System

Anchors use cell coordinates, not byte offsets.

**Rationale:**

- Byte offsets break when ANSI escape sequences are stripped for analysis
- Byte offsets break when wide characters (CJK, emoji) are present
- Byte offsets break when terminal wraps long lines
- Cell coordinates match what the user sees

**Coordinate definition:**

- Row: 0-indexed line number in rendered output (after line wrapping)
- Column: 0-indexed cell position (accounting for character width)

**Coordinate stability:** Coordinates are computed against the final rendered state. If terminal width


changes and lines rewrap, anchors are recomputed.

### 7.4 Anchor Types and Actions

**File Path Anchor:**

- Detection: Absolute paths, relative paths, `~/` paths
- Validation: Optional filesystem existence check
- Actions: Open in editor, Open in finder, Copy path, Preview (if text file)
- Display: Underlined, file icon on hover

**URL Anchor:**

- Detection: http://, https://, ftp://, file:// schemes
- Validation: URL parsing
- Actions: Open in browser, Copy URL, Fetch preview
- Display: Underlined in link color

**Git Reference Anchor:**

- Detection: SHA hashes (7+ hex chars), branch names (from provider), tags
- Validation: Provider confirms ref exists
- Actions: Show commit, Copy SHA, Checkout, Diff against HEAD
- Display: Monospace, git icon on hover

**IP Address Anchor:**

- Detection: IPv4 dotted quad, IPv6 addresses
- Validation: Format parsing
- Actions: Copy, Ping, Lookup hostname, SSH to
- Display: Underlined

**Error Location Anchor:**

- Detection: Compiler error format (`file:line:col`), stack traces
- Validation: File existence
- Actions: Open at location in editor
- Display: Red underline, click navigates directly

**Docker/Kubernetes Resource Anchor:**

- Detection: Container IDs, pod names, image names (from provider)
- Validation: Provider confirms resource exists
- Actions: Logs, Exec, Stop, Describe
- Display: Resource-type icon on hover

### 7.5 Hover and Click Behavior

**Hover:**

- 200ms delay before hover card appears
- Hover card shows: anchor type, extracted value, available actions, confidence indicator
- Preview content loads asynchronously (file preview, URL fetch, git commit message)
- Hover card dismisses when cursor leaves anchor or card

**Click:**

- Single click: Primary action (configurable per anchor type, default is "most common")
- Right click: Context menu with all available actions
- Modifier+click: Secondary action (e.g., Cmd+click opens file in editor vs. finder)

**Keyboard navigation:**

- Tab/Shift+Tab: Move between anchors in block
- Enter: Activate primary action on focused anchor
- Space: Open context menu on focused anchor

---

# Part V: Provider System

## 8. Provider Architecture

### 8.1 Provider Concept

Providers are plugins that enhance Nexus with command-specific intelligence. They supply argument hints, completions, sidecars, and anchor definitions.

**Provider responsibilities:**

- Declare which commands they handle
- Provide argument documentation and validation
- Supply completion candidates
- Define sidecar commands for structured data
- Parse sidecar output into anchors and structured data
- Declare actions available for their resource types

**Provider isolation:**

- Providers run in separate processes
- Providers communicate through a defined API via IPC
- Provider crashes do not affect shell stability

### 8.2 Provider API

Providers implement a standard interface:

**Metadata:**

- `name`: Provider identifier (e.g., "nexus-provider-git")
- `version`: Semantic version
- `commands`: List of command patterns this provider handles (e.g., "git *", "docker *")
- `resource_types`: Anchor types this provider can create and handle

**Command Intelligence:**

- `get_completions(command_line, cursor_position) -> CompletionList`: Returns completion candidates for current input
- `get_documentation(command_line, cursor_position) -> Documentation`: Returns help text for argument at cursor
- `validate_arguments(command_line) -> ValidationResult`: Returns warnings/errors for invalid arguments

**Sidecar Definition:**

- `get_sidecar(command) -> SidecarSpec | null`: Returns sidecar command to run alongside user command
- `parse_sidecar_output(output) -> StructuredResult`: Parses sidecar output into anchors and data

**Actions:**

- `get_actions(anchor) -> ActionList`: Returns available actions for an anchor
- `execute_action(action, anchor) -> ActionResult`: Executes an action, returns command to run or result

### 8.3 Provider Discovery and Loading

**Discovery locations:**

1. Built-in providers (compiled into Nexus)
2. User providers: `~/.config/nexus/providers/`
3. Project providers: `.nexus/providers/` in current directory or ancestors
4. Installed packages: From npm-style registry (future)

**Loading sequence:**

1. At shell startup, scan discovery locations
2. Load provider manifests (metadata only)
3. Build command routing table (command pattern → provider)
4. Lazy-load providers when first needed
5. Cache loaded providers for session duration

**Conflict resolution:** When multiple providers claim the same command, priority is: project > user > installed > built-in. Users can override in config.

### 8.4 Built-in Providers

**Git Provider:**

- Commands: `git *`
- Completions: Branches, tags, remotes, files, subcommands
- Sidecar: `git status --porcelain=v2` for status, `git log --format=...` for log
- Anchors: File paths, commit SHAs, branch names
- Actions: Stage, unstage, diff, commit, checkout, blame

**Docker Provider:**

- Commands: `docker *`, `docker-compose *`
- Completions: Container names/IDs, image names, networks, volumes
- Sidecar: `docker ps --format json`, `docker images --format json`
- Anchors: Container IDs, image names, port mappings
- Actions: Logs, exec, stop, start, remove, inspect

**Kubernetes Provider:**

- Commands: `kubectl *`, `k *` (if aliased)
- Completions: Resource types, resource names, namespaces, contexts
- Sidecar: `kubectl get ... -o json`
- Anchors: Pod names, service names, deployment names
- Actions: Logs, exec, describe, delete, port-forward

**Filesystem Provider:**

- Commands: `ls`, `find`, `fd`, `tree`, `exa`
- Completions: File paths with type-aware filtering
- Sidecar: None (uses native output parsing)
- Anchors: File paths with type detection
- Actions: Open, edit, copy path, preview, delete

**SSH Provider:**

- Commands: `ssh`, `scp`, `rsync`
- Completions: Hosts from `~/.ssh/config`, known_hosts
- Sidecar: None
- Anchors: Hostnames
- Actions: Connect, copy host, edit config

### 8.5 Provider Runtime

Providers run as separate processes communicating via IPC.

**Runtime capabilities:**

- Execution time limits (100ms for completions, 1s for parsing)
- Data passed via structured IPC messages
- Shell executes commands on provider's behalf

**IPC API available to providers:**

- `log(level, message)`: Write to debug log
- `cache_get(key) -> value`: Read from provider-specific cache
- `cache_set(key, value, ttl)`: Write to provider-specific cache
- `request_command(argv) -> request_id`: Request shell execute a command (async)
- `get_command_result(request_id) -> result`: Get result of requested command

**Rationale for process isolation:**

- Simple: Standard IPC, no complex runtime
- Debuggable: Providers can be tested independently
- Language-agnostic: Any language that can do IPC works

---

# Part VI: Sidecar Execution

## 9. Sidecar System

### 9.1 Sidecar Concept

A sidecar is a command executed alongside the user's command to obtain structured data. The user sees their original command's output; the sidecar output is parsed and used for overlays and anchors.

**Sidecar properties:**

- **Parallel execution:** Sidecar runs concurrently with user command
- **Hidden output:** Sidecar stdout/stderr not shown to user
- **Structured parsing:** Sidecar output parsed by provider
- **Non-blocking:** Sidecar failures don't affect user command
- **Cached:** Results cached to prevent redundant execution

### 9.2 Sidecar Execution Flow

1. User types command (e.g., `git status`)
2. Shell queries providers for sidecar specification
3. Git provider returns sidecar: `git status --porcelain=v2`
4. Shell executes both commands:
   - User command: `git status` with PTY (user sees colorized output)
   - Sidecar command: `git status --porcelain=v2` with pipe (hidden)
5. Sidecar output sent to provider's parser
6. Parser returns structured data: file statuses, anchors, metadata
7. UI renders overlay on top of user command output

### 9.3 Overlay Rendering

Overlays add interactive elements to the raw output without modifying it.

**Overlay types:**

- **Anchor highlights:** Underlines and hover targets on detected elements
- **Inline buttons:** Action buttons rendered at end of relevant lines
- **Margin icons:** Status icons in left margin (e.g., git status indicators)
- **Floating panels:** Expandable detail panels attached to anchors

**Overlay positioning:**

- Overlays are positioned relative to rendered output using cell coordinates
- When output scrolls, overlays scroll with their anchor positions
- When terminal resizes, overlays reposition based on recomputed coordinates

**Overlay interaction:**

- Overlays capture mouse events within their bounds
- Keyboard focus can move to overlay elements
- Overlay actions invoke provider action handlers

### 9.4 Sidecar Caching

Sidecar results are cached to prevent redundant execution.

**Cache key:** `(working_directory, command_text, environment_hash)`

**Cache invalidation:**

- Time-based: Configurable TTL per provider (default 5 seconds)
- Event-based: Invalidate on relevant filesystem changes (if watching)
- Manual: User can force refresh

**Cache storage:**

- In-memory LRU cache
- Maximum entries configurable (default 1000)
- Maximum memory configurable (default 50MB)

### 9.5 Action Execution Safety

Actions triggered from UI (buttons, context menus) execute via direct `execve`, not shell parsing.

**Rationale:**

A malicious filename or container ID could contain shell metacharacters. If UI actions constructed shell command strings, injection attacks would be possible.

**Example attack prevented:**

- Container named `foo; rm -rf /`
- UI "Stop Container" button clicked
- Unsafe: `docker stop foo; rm -rf /` executed via shell
- Safe: `execve("docker", ["docker", "stop", "foo; rm -rf /"])` - metacharacters are literal

**Implementation:**

- Provider actions return argv arrays, not command strings
- Shell executes via direct `execve` with argument array
- No shell parsing, expansion, or interpretation occurs
- Arguments are passed exactly as specified

---

# Part VII: Remote Connectivity

## 10. SSH Integration

### 10.1 Design Philosophy

SSH is treated as a hostile environment by default. Nexus provides a solid baseline experience without requiring any remote installation, with optional enhancement when cooperation is available.

**Baseline (no agent):**

- Standard SSH connection via libssh2
- PTY allocated on remote system
- Full terminal emulation locally
- Heuristic parsing of output stream
- SFTP-assisted path completion

**Enhanced (with agent):**

- All baseline features
- Plus: Structured events from remote shell
- Plus: Accurate CWD tracking
- Plus: Remote file previews
- Plus: Provider intelligence for remote commands

### 10.2 Connection Management

**Connection establishment:**

1. Parse SSH destination (user@host:port or alias from config)
2. Load credentials (key file, agent, or password prompt)
3. Establish SSH session via libssh2
4. Open PTY channel with user's default shell
5. Optionally open SFTP channel for file operations
6. Probe for Nexus agent presence

**Session state:**

- Connection status (connecting, connected, disconnected, error)
- Remote user and hostname
- Remote shell type (detected from `$SHELL` or behavior)
- Agent availability
- SFTP channel status
- Latency estimate (from keepalive roundtrips)

**Multiplexing:**

- Single SSH connection supports multiple channels
- PTY channel for interactive shell
- SFTP channel for file operations
- Subsystem channel for agent communication (if available)

### 10.3 Baseline Remote Features

**Terminal emulation:**

- Full terminal emulation of remote PTY output
- Local echo disabled (remote shell handles echo)
- Window size synchronized on resize
- Keyboard input forwarded to remote PTY

**Heuristic parsing:**

- Stream sentinel watches for mode-switching escape sequences
- Anchor detection runs on rendered output (same as local)
- Confidence is lower (no provider verification)

**SFTP-assisted completion:**

When user presses Tab in a remote session:

1. Check local SFTP cache for current directory listing
2. If cache hit and fresh (<30s): Return completions immediately
3. If cache miss or stale:
   a. Send Tab character to remote PTY (let remote shell handle it)
   b. Simultaneously request SFTP directory listing
   c. Update cache for future completions

**Result:** Completion feels instant when cache is warm, degrades gracefully to remote shell completion when cold.

### 10.4 Agent Protocol

The Nexus agent is an optional static binary users can install on remote systems.

**Agent installation:**

- Single static binary, no dependencies
- User places in `~/.local/bin/nexus-agent` or anywhere in PATH
- User adds to shell rc: `eval "$(nexus-agent init)"`
- Agent sets `NEXUS_AGENT=1` environment variable

**Agent detection:**

Upon SSH connection, Nexus checks for `NEXUS_AGENT` in remote environment. If present, it opens a subsystem channel for structured communication.

**Agent capabilities:**

- **CWD tracking:** Reports working directory changes with full path
- **Command events:** Reports command start/finish with timing
- **Environment sync:** Reports environment variable changes
- **File watching:** Reports filesystem changes in watched directories
- **Structured output:** Runs sidecar commands and returns parsed results

**Agent protocol:**

- JSON-RPC over SSH subsystem channel
- Bidirectional: Nexus can request actions, agent can push events
- Versioned: Protocol version negotiated at connection

**Privacy consideration:**

The agent only reports information that would be visible to anyone with shell access. It does not provide elevated privileges or access to other users' data.

### 10.5 Graceful Degradation

Feature availability matrix by connection type:

| Feature | Local | SSH (no agent) | SSH (with agent) |
|:--------|:------|:---------------|:-----------------|
| Command execution | ✓ | ✓ | ✓ |
| Terminal emulation | ✓ | ✓ | ✓ |
| Syntax highlighting | ✓ | ✓ | ✓ |
| Heuristic anchors | ✓ | ✓ | ✓ |
| Provider anchors | ✓ | ✗ | ✓ |
| Accurate CWD | ✓ | Heuristic | ✓ |
| File completion | ✓ | SFTP (async) | ✓ |
| File preview | ✓ | SFTP (on demand) | ✓ |
| Sidecars | ✓ | ✗ | ✓ |
| Block references | ✓ | ✓ (local blocks) | ✓ (local blocks) |

**Principle:** Every feature either works fully or is clearly unavailable. No feature silently produces wrong results.

---

# Part VIII: Session Management

## 11. Sessions and Tabs

### 11.1 Session Concept

A session is an independent shell instance with its own state.

**Session state includes:**

- Shell interpreter state (env, vars, functions, aliases)
- Working directory
- Job table
- Block history
- Undo/redo stack
- UI state (scroll position, collapsed blocks, pinned blocks)

**Session isolation:**

- Sessions do not share environment changes
- Sessions do not share function/alias definitions
- Sessions do not share working directory

### 11.2 Tab Management

Each tab contains one session.

**Tab operations:**

- New tab: Creates fresh session with default environment


- Duplicate tab: Creates new session with copied environment snapshot
- Close tab: Terminates session (with confirmation if jobs running)
- Reorder tabs: Drag-and-drop rearrangement
- Split tab: Divide tab into panes (horizontal or vertical)

**Tab persistence:**

- Tab state serialized on close
- Tabs restored on application restart (configurable)
- Restoration includes: working directory, environment, block history (not running processes)

### 11.3 Branching

Users can create a new session branched from a specific point in history.

**Branch operation:**

1. User right-clicks a block and selects "Branch from here"
2. New tab created with new session
3. Session initialized with environment snapshot from that block's `meta.json`
4. Working directory set to that block's CWD
5. Block history is NOT copied (clean slate)

**Use case:** "What if I had done something different after this point?"

**Limitations clearly communicated:**

- Filesystem state is not restored
- Network conditions are not restored
- Running processes are not restored
- Only environment and CWD are restored

### 11.4 History Re-execution

Two explicit modes for re-running historical commands:

**Rerun Here (default):**

- Executes command text in current shell state
- Uses current working directory
- Uses current environment
- Equivalent to pressing up-arrow and enter in traditional terminal

**Rerun in Context:**

- Creates temporary sub-shell
- Restores environment variables from block's snapshot
- Changes to block's original working directory
- Executes command
- Returns to original state after completion
- Warning displayed: "Filesystem and network state cannot be restored"

**UI presentation:**

- "Rerun" button defaults to "Rerun Here"
- Dropdown arrow reveals "Rerun in Context" option
- Tooltip explains the difference

---

# Part IX: Input Handling

## 12. Line Editing

### 12.1 Editing Model

Input uses a rich text editing model with syntax-aware behavior.

**Cursor types:**

- Point cursor: Single position between characters
- Selection: Range of characters (start, end)
- Multi-cursor: Multiple independent cursors (future)

**Editing operations:**

- Insert: Add characters at cursor
- Delete: Remove characters (backspace, delete, kill-word, kill-line)
- Move: Reposition cursor (character, word, line, home, end)
- Select: Extend selection (shift+move operations)
- Clipboard: Cut, copy, paste (with bracketed paste support)
- Transpose: Swap characters or words
- Case change: Uppercase, lowercase, title case selection

### 12.2 Keybindings

Default keybindings follow common conventions with Emacs-style alternatives.

**Movement:**

| Key | Action |
|:----|:-------|
| Left/Right | Move by character |
| Ctrl+Left/Right | Move by word |
| Home/Ctrl+A | Move to line start |
| End/Ctrl+E | Move to line end |
| Ctrl+F/B | Move forward/backward character (Emacs) |
| Alt+F/B | Move forward/backward word (Emacs) |

**Editing:**

| Key | Action |
|:----|:-------|
| Backspace | Delete character before cursor |
| Delete/Ctrl+D | Delete character after cursor |
| Ctrl+W | Delete word before cursor |
| Alt+D | Delete word after cursor |
| Ctrl+U | Delete to line start |
| Ctrl+K | Delete to line end |
| Ctrl+Y | Paste last deleted text (yank) |
| Ctrl+T | Transpose characters |

**History:**

| Key | Action |
|:----|:-------|
| Up/Ctrl+P | Previous history entry |
| Down/Ctrl+N | Next history entry |
| Ctrl+R | Reverse history search |
| Ctrl+S | Forward history search |
| Alt+. | Insert last argument of previous command |

**Completion:**

| Key | Action |
|:----|:-------|
| Tab | Trigger completion |
| Shift+Tab | Previous completion candidate |
| Ctrl+Space | Show all completions |
| Escape | Dismiss completion menu |

**Execution:**

| Key | Action |
|:----|:-------|
| Enter | Execute command (or newline if incomplete) |
| Shift+Enter | Insert literal newline |
| Ctrl+C | Cancel current input / interrupt running command |
| Ctrl+D | Exit shell (if input empty) |

**Customization:** All keybindings configurable in `config.toml`. Users can define custom bindings and override defaults.

### 12.3 Multi-line Input

Commands can span multiple lines.

**Automatic continuation:**

- Unclosed quotes: `"...` continues to next line
- Backslash continuation: `\` at end of line
- Unclosed brackets: `(`, `{`, `[`
- Pipeline continuation: `|` at end of line
- Logical operators: `&&`, `||` at end of line

**Visual indication:**

- Continuation lines indented
- Continuation prompt (e.g., `> `) shown in margin
- Bracket matching highlights paired delimiters

**Editing behavior:**

- Up/Down navigate within multi-line input before accessing history
- Enter executes only when input is syntactically complete
- Shift+Enter always inserts newline regardless of completeness

### 12.4 Syntax Highlighting

Input is highlighted in real-time based on Tier 1 and Tier 2 analysis.

**Token categories and default colors:**

| Category | Example | Default Color |
|:---------|:--------|:--------------|
| Command (builtin) | `cd`, `echo` | Cyan |
| Command (external) | `git`, `docker` | Blue |
| Command (alias) | User-defined | Purple |
| Command (function) | User-defined | Purple |
| Command (unknown) | Typos | Orange with underline |
| Argument | Positional args | Default |
| Flag (short) | `-v`, `-la` | Green |
| Flag (long) | `--verbose` | Green |
| String (single-quoted) | `'text'` | Yellow |
| String (double-quoted) | `"text"` | Yellow |
| Variable | `$PATH` | Orange |
| Variable (braced) | `${PATH}` | Orange |
| Expansion | `$(cmd)` | Magenta |
| Glob | `*.txt` | Magenta |
| Redirection | `>`, `2>&1` | Red |
| Pipe | `\|` | Red |
| Operator | `&&`, `\|\|`, `;` | Red |
| Comment | `# text` | Gray |
| Error | Syntax errors | Red background |

**Customization:** Colors configurable in theme section of `config.toml`.

## 13. Completion System

### 13.1 Completion Architecture

Completion combines multiple sources with intelligent ranking.

**Completion sources (in priority order):**

1. **Provider completions:** Command-specific intelligence from providers
2. **Path completions:** Filesystem paths from current directory or absolute
3. **History completions:** Previous commands and arguments
4. **Variable completions:** Environment and shell variables
5. **Alias/function completions:** User-defined names

**Triggering:**

- Explicit: Tab key pressed
- Implicit: After typing trigger characters (configurable, default: `/`, `-`)
- Continuous: While completion menu is open, updates with each keystroke

### 13.2 Completion Context

The completion system determines context from cursor position.

**Context types:**

- **Command position:** First word of simple command, after `|`, `&&`, `||`, `;`
- **Argument position:** After command, completing arguments
- **Flag value position:** After `--flag=` or `-f `
- **Variable position:** After `$` or `${`
- **Path position:** Detected by `/` or `./` or `~/` prefix
- **Redirect position:** After `>`, `>>`, `<`

**Context detection:**

1. Parse input up to cursor using Tree-sitter
2. Examine AST node at cursor position
3. Walk up tree to determine syntactic context
4. Query appropriate completion sources for context

### 13.3 Completion Ranking

Candidates are ranked by relevance score.

**Ranking factors:**

- **Prefix match:** Candidates starting with typed text score higher
- **Fuzzy match:** Candidates matching typed characters in order score lower than prefix
- **Recency:** Recently used commands/paths score higher
- **Frequency:** Frequently used items score higher
- **Provider confidence:** Provider-supplied confidence affects ranking
- **Type match:** Items matching expected type (file, directory, executable) score higher

**Score combination:**

```
score = (prefix_match * 1000) + (recency * 100) + (frequency * 10) + (provider_confidence * 5) + (fuzzy_score)
```

**Tie-breaking:** Alphabetical order when scores are equal.

### 13.4 Completion UI

**Menu appearance:**

- Appears below cursor (or above if near bottom of screen)
- Shows 10 candidates by default (configurable)
- Each candidate shows: icon, text, type annotation, source indicator
- Selected candidate highlighted
- Scrollbar if more candidates available

**Menu interaction:**

- Tab/Down: Select next candidate
- Shift+Tab/Up: Select previous candidate
- Enter/Right: Accept selected candidate
- Escape: Dismiss menu
- Continue typing: Filter candidates
- Any non-matching key: Accept and continue

**Preview:**

- File completions show file type icon and size
- Directory completions show item count
- Command completions show brief description (from provider)
- Variable completions show current value

**Documentation panel:**

- When enabled, shows extended documentation for selected candidate
- Appears to the right of completion menu
- Shows: full description, usage examples, source (man page, provider, etc.)

---

# Part X: Persistence and History

## 14. History System

### 14.1 Command History

All executed commands are recorded with metadata.

**History entry contents:**

- Command text (full, including multi-line)
- Timestamp (start time)
- Duration (execution time)
- Exit status
- Working directory at execution
- Session ID
- Block ID (for cross-reference to output)

**History storage:**

- SQLite database at `~/.local/share/nexus/history.db`
- Indexed by timestamp, command text, working directory
- Full-text search enabled

**History limits:**

- Maximum entries: Configurable (default 100,000)
- Maximum age: Configurable (default unlimited)
- Eviction: Oldest entries removed when limit exceeded

### 14.2 History Search

**Incremental search (Ctrl+R):**

1. Search prompt appears in input area
2. User types search pattern
3. Matching history entries shown in reverse chronological order
4. Up/Down navigate through matches
5. Enter accepts and places command in input
6. Escape cancels search

**Search modes:**

- Substring: Match anywhere in command (default)
- Prefix: Match at start of command
- Regex: Full regular expression matching
- Fuzzy: Fuzzy character matching

**Search scope:**

- Current session: Only commands from this session
- All sessions: Commands from all sessions
- Directory-local: Commands executed in current directory or children

### 14.3 History Deduplication

**Deduplication options (configurable):**

- None: All commands recorded
- Consecutive: Ignore immediately repeated commands
- Global: Store each unique command once, update timestamp on repeat

**Ignore patterns:**

- Commands starting with space (configurable)
- Commands matching user-defined patterns
- Commands containing sensitive patterns (passwords, tokens)

### 14.4 Output History

Block outputs are persisted separately from command history.

**Output storage:**

- Ring buffers spilled to `~/.local/share/nexus/blocks/`
- Directory structure: `{block_id}/stdout`, `{block_id}/stderr`, `{block_id}/meta.json`
- Compressed with zstd after completion

**Output limits:**

- Maximum disk usage: Configurable (default 1GB)
- Maximum age: Configurable (default 30 days)
- Eviction: LRU based on last access time

**Output indexing:**

- Full-text search index built on output content
- Enables searching across all historical output
- Index updated asynchronously after command completion

---

# Part XI: Theming and Appearance

## 15. Visual Design

### 15.1 Color System

**Semantic colors:**

Colors are defined semantically, not by literal values.

| Semantic Name | Usage |
|:--------------|:------|
| `background.primary` | Main background |
| `background.secondary` | Block backgrounds, panels |
| `background.tertiary` | Hover states, subtle highlights |
| `foreground.primary` | Main text |
| `foreground.secondary` | Dimmed text, annotations |
| `foreground.muted` | Very dim text, disabled states |
| `accent.primary` | Primary actions, links |
| `accent.secondary` | Secondary highlights |
| `border.default` | Default borders |
| `border.focused` | Focused element borders |
| `status.success` | Success states, exit code 0 |
| `status.error` | Error states, non-zero exit |
| `status.warning` | Warning states |
| `status.info` | Informational states |

**Terminal colors:**

Standard 16-color ANSI palette plus 256-color and 24-bit support.

| Index | Name | Default Dark | Default Light |
|:------|:-----|:-------------|:--------------|
| 0 | Black | #1a1a1a | #f5f5f5 |
| 1 | Red | #ff6b6b | #d32f2f |
| 2 | Green | #69db7c | #388e3c |
| 3 | Yellow | #ffd93d | #fbc02d |
| 4 | Blue | #4dabf7 | #1976d2 |
| 5 | Magenta | #da77f2 | #7b1fa2 |
| 6 | Cyan | #66d9e8 | #0097a7 |
| 7 | White | #e9ecef | #424242 |
| 8-15 | Bright variants | Lighter | Darker |

### 15.2 Typography

**Font stack:**

- Primary: User-configured monospace font
- Fallback chain: JetBrains Mono, Fira Code, SF Mono, Menlo, Consolas, monospace
- UI elements: System sans-serif font

**Font features:**

- Ligatures: Configurable (default enabled for supported fonts)
- Character variants: Configurable (e.g., slashed zero, dotted zero)

**Size scale:**

| Element | Default Size |
|:--------|:-------------|
| Terminal text | 14px |
| UI labels | 13px |
| Block headers | 12px |
| Tooltips | 12px |
| Tab titles | 13px |

**Line height:** 1.4 for terminal text (configurable)

**Letter spacing:** 0 (configurable)

### 15.3 Theme Definition

Themes are defined in TOML.

**Theme structure:**

```toml
[theme]
name = "My Theme"
appearance = "dark"  # or "light"

[theme.colors.background]
primary = "#1a1a1a"
secondary = "#2d2d2d"
tertiary = "#3d3d3d"

[theme.colors.foreground]
primary = "#ffffff"
secondary = "#a0a0a0"
muted = "#606060"

[theme.colors.accent]
primary = "#007acc"
secondary = "#3794ff"

[theme.colors.status]
success = "#4ec9b0"
error = "#f14c4c"
warning = "#cca700"
info = "#3794ff"

[theme.colors.ansi]
black = "#000000"
red = "#cd3131"
green = "#0dbc79"
# ... etc

[theme.syntax]
command = "#dcdcaa"
argument = "#ce9178"
flag = "#569cd6"
string = "#ce9178"
variable = "#9cdcfe"
```

**Built-in themes:**

- Nexus Dark (default)
- Nexus Light
- High Contrast Dark
- High Contrast Light

**Custom themes:**

Users can define custom themes in `config.toml` or load from theme files.

### 15.4 Layout Customization

**Configurable layout options:**

- Tab bar position: Top, bottom, hidden
- Block spacing: Compact, comfortable, spacious
- Block borders: Visible, subtle, hidden
- Prompt position: Inline (after last block), fixed bottom, floating
- Sidebar: File browser, git panel, job list (all optional)
- Status bar: Visible, hidden, auto-hide

**Block density options:**

- Compact: Minimal padding, smaller headers
- Comfortable: Default spacing (recommended)
-


Spacious: Extra padding, larger touch targets

**Responsive behavior:**

- Narrow windows: Collapse sidebars, simplify headers
- Wide windows: Optional split panes, expanded panels
- Minimum width: 400px (below this, horizontal scroll)

---

# Part XII: Development Roadmap

## 16. Implementation Phases

### Phase 1: Contract Kernel (Months 1-4)

**Objective:** A stable, correct POSIX shell implementation with the foundational architecture.

**Milestone 1.1: Parser and AST (Weeks 1-4)**

- Implement Tree-sitter grammar for Nexus shell language
- Build AST extraction from Tree-sitter CST
- Implement Tier 1 syntax analysis with error recovery
- Unit tests for all grammar constructs
- Benchmark: Parse 10,000 lines in <100ms

**Milestone 1.2: Interpreter Core (Weeks 5-8)**

- Implement word expansion (all POSIX expansions)
- Implement simple command execution
- Implement redirections and file descriptor manipulation
- Implement pipelines with pump threads
- Implement control structures (if, for, while, case)
- Implement functions and local variables
- Unit tests against POSIX test suite subset

**Milestone 1.3: Job Control (Weeks 9-12)**

- Implement process group management
- Implement foreground/background job handling
- Implement signal handling (SIGCHLD, SIGTSTP, SIGCONT)
- Implement job builtins (jobs, fg, bg, wait)
- Integration tests for job control scenarios

**Milestone 1.4: State and Events (Weeks 13-16)**

- Implement shell state structures
- Implement event bus with typed events
- Implement block storage and lifecycle
- Implement ring buffers with overflow handling
- Implement block reference expansion
- Integration tests for state synchronization

**Exit criteria:**

- Passes 95% of POSIX sh compliance test suite
- Passes custom test suite for supported Bash extensions
- Can execute common shell scripts (configure, build scripts)
- Event bus delivers all state changes to subscribers
- Block references resolve correctly

### Phase 2: Honest UI (Months 5-7)

**Objective:** A functional graphical interface with reliable rendering and honest data presentation.

**Milestone 2.1: GPUI Foundation (Weeks 17-20)**

- Set up GPUI application scaffold
- Implement window management and tab system
- Implement basic block list rendering
- Implement input line with cursor
- Implement event bus subscription and view model
- Benchmark: 60fps scrolling with 1000 blocks

**Milestone 2.2: Terminal Emulation (Weeks 21-24)**

- Implement ANSI escape sequence parser
- Implement terminal state machine (cursor, attributes, modes)
- Implement text rendering with attributes
- Implement alternate screen buffer
- Implement stream sentinel for TUI detection
- Test: vim, htop, top, less work correctly

**Milestone 2.3: Block System (Weeks 25-28)**

- Implement block lifecycle (input, running, completed, collapsed)
- Implement block header with command display
- Implement block footer with status and actions
- Implement block selection and focus
- Implement rerun functionality (both modes)
- Implement block reference syntax expansion

**Milestone 2.4: Input and Completion (Weeks 29-32)**

- Implement line editor with keybindings
- Implement syntax highlighting in input
- Implement completion menu
- Implement history search
- Implement multi-line input handling
- Test: Completion latency <50ms for filesystem paths

**Exit criteria:**

- Full terminal emulation passes vttest subset
- TUI applications work correctly (vim, htop, etc.)
- Input editing feels responsive (<8ms input latency)
- Completion works for paths, commands, history
- Rerun functionality works correctly in both modes

### Phase 3: Lens and Provider SDK (Months 8-10)

**Objective:** Extensible augmentation system with providers.

**Milestone 3.1: Lens System (Weeks 33-36)**

- Implement Lens abstraction and switching
- Implement Raw Lens (default)
- Implement JSON Lens with tree view
- Implement Table Lens with sorting/filtering
- Implement Hex Lens
- Implement provenance display for all Lenses
- Implement error fallback protocol

**Milestone 3.2: Anchor System (Weeks 37-40)**

- Implement anchor detection framework
- Implement cell coordinate system
- Implement heuristic detectors (URL, path, IP)
- Implement anchor rendering (underlines, hover cards)
- Implement anchor actions (click, context menu)
- Implement keyboard navigation between anchors

**Milestone 3.3: Provider Runtime (Weeks 41-44)**

- Implement provider process management
- Implement provider IPC API
- Implement provider loading and lifecycle
- Implement provider-to-shell communication
- Implement provider caching

**Milestone 3.4: Built-in Providers (Weeks 45-48)**

- Implement Git provider (completions, sidecars, anchors, actions)
- Implement Docker provider
- Implement Kubernetes provider
- Implement Filesystem provider
- Documentation for provider development
- Example providers in Rust and Python

**Exit criteria:**

- Lenses switch correctly and display provenance
- Lens failures gracefully degrade to Raw view
- Anchors detected and interactive
- Provider SDK documented and usable
- Built-in providers functional for common workflows
- Provider crashes do not affect shell stability

### Phase 4: Remote Intelligence (Months 11-13)

**Objective:** Full SSH integration with graceful degradation.

**Milestone 4.1: SSH Baseline (Weeks 49-52)**

- Implement libssh2 integration
- Implement PTY channel management
- Implement terminal emulation for remote sessions
- Implement connection management (reconnect, timeout)
- Implement credential handling (keys, agent, password)
- Test: SSH to various server types (Linux, macOS, BSD)

**Milestone 4.2: SFTP Integration (Weeks 53-56)**

- Implement SFTP channel management
- Implement directory listing cache
- Implement SFTP-assisted completion
- Implement remote file preview
- Implement remote file editing workflow
- Test: Completion latency acceptable on high-latency connections

**Milestone 4.3: Agent Protocol (Weeks 57-60)**

- Design and document agent protocol
- Implement agent binary (static, minimal dependencies)
- Implement agent initialization and shell integration
- Implement subsystem channel communication
- Implement structured event streaming
- Test: Agent works on major Linux distributions

**Milestone 4.4: Remote Providers (Weeks 61-64)**

- Implement remote sidecar execution via agent
- Implement remote anchor detection
- Implement remote-aware providers
- Implement connection status UI
- Documentation for remote workflows
- Test: Full provider functionality over SSH with agent

**Exit criteria:**

- SSH connections work reliably
- SFTP completion provides good UX on reasonable latency
- Agent installation is simple and well-documented
- Remote sessions with agent match local functionality
- Remote sessions without agent provide solid baseline
- Graceful degradation clearly communicated to users

### Phase 5: Polish and Launch (Months 14-16)

**Objective:** Production-ready release with documentation and community infrastructure.

**Milestone 5.1: Performance Optimization (Weeks 65-68)**

- Profile and optimize rendering hot paths
- Optimize memory usage for large scrollback
- Optimize startup time
- Optimize completion latency
- Benchmark suite with regression detection

**Milestone 5.2: Platform Support (Weeks 69-72)**

- macOS build and testing
- Linux build and testing (Ubuntu, Fedora, Arch)
- Windows build and testing (with WSL2 integration)
- Platform-specific installers
- Auto-update mechanism

**Milestone 5.3: Documentation (Weeks 73-76)**

- User guide (getting started, configuration, workflows)
- Reference manual (all commands, options, APIs)
- Provider development guide
- Theme development guide
- Migration guide from Bash/Zsh
- Video tutorials for key features

**Milestone 5.4: Community Launch (Weeks 77-80)**

- Public repository with contribution guidelines
- Issue templates and triage process
- Discussion forum or Discord
- Provider registry (discovery and installation)
- Theme gallery
- Launch blog post and demonstrations

**Exit criteria:**

- Performance meets all benchmarks
- Works on all target platforms
- Documentation comprehensive and accurate
- Community infrastructure operational
- Provider ecosystem has initial contributions
- Positive reception from early adopters

---

# Part XIII: Technical Constraints and Decisions

## 17. Explicit Constraints

### 17.1 What We Cannot Do

**We cannot parse `.bashrc`:**

Bash configuration files use Bash-specific syntax, conditionals, and features. Attempting to parse them would require implementing full Bash, which is explicitly a non-goal. Users must migrate their configuration to Nexus format.

**We cannot spy on arbitrary process I/O:**

Without kernel-level access (eBPF, ptrace), we cannot observe data flowing between processes we didn't create. Our solution is to be the middleman—by owning the pipeline, we can observe all data that flows through it.

**We cannot know what commands do:**

We cannot introspect the behavior of arbitrary executables. Our solution is the provider system—explicit, opt-in intelligence for known commands.

**We cannot restore filesystem state:**

When re-running historical commands, we can restore environment variables and working directory, but we cannot restore the filesystem to its previous state. This limitation is clearly communicated to users.

**We cannot guarantee remote functionality:**

SSH connections to systems we don't control may have limited functionality. Our solution is graceful degradation—baseline functionality works everywhere, enhanced functionality requires cooperation (agent installation).

### 17.2 What We Choose Not To Do

**We do not implement full Bash compatibility:**

Full Bash compatibility is a tar pit. We implement POSIX plus the most valuable Bash extensions. Scripts requiring full Bash should be run with `/bin/bash`.

**We do not use web technologies for the UI:**

Electron and web-based terminals introduce latency, memory overhead, and complexity. GPUI provides native performance with Rust's safety guarantees.

**We do not scrape man pages for documentation:**

Man page parsing is fragile and produces low-quality results. Provider-supplied documentation is explicit, structured, and reliable.

**We do not auto-upload binaries to remote systems:**

Automatically installing software on systems we don't own is a security and trust violation. Agent installation is always explicit and user-initiated.

**We do not hide the raw output:**

Structured views (Lenses) are conveniences, not replacements. The raw byte stream is always accessible and is always the source of truth.

### 17.3 Key Technical Decisions

| Decision | Choice | Rationale |
|:---------|:-------|:----------|
| Shell language | POSIX + Big Three Bashisms | Maximum compatibility without full Bash complexity |
| Parser | Tree-sitter | Incremental parsing, error recovery, community grammars |
| Interpreter | AST-walking | Simplicity, correctness over marginal performance gains |
| State sync | Event bus | Enables undo and debugging; prevents race conditions |
| Pipeline observation | Pump threads + ring buffers | Only way to observe without kernel access |
| Output references | Block references (`%N`) | Shell-integrated, works with pipelines and redirections |
| Anchor coordinates | Cell-based | Survives ANSI stripping, wide characters, line wrapping |
| Provider isolation | Process isolation | Simple, debuggable, language-agnostic |
| Configuration | TOML | Human-readable, no runtime evaluation, familiar format |
| UI framework | GPUI (Rust) | Native performance, single binary, no web overhead |
| Remote baseline | Standard SSH | Works everywhere without requiring installation |
| Remote enhancement | Opt-in agent | Full functionality when user chooses to install |
| Action execution | Direct execve | Prevents shell injection attacks |
| Error handling | Fallback to raw | Truth over convenience; never show broken structured views |
| Provenance | Always displayed | Builds trust, aids debugging |

---

# Part XIV: Security Considerations

## 18. Security Model

### 18.1 Threat Model

**Trusted:**

- The Nexus binary itself
- User's configuration files
- Built-in providers

**Partially trusted:**

- Third-party providers (process-isolated)
- Remote systems (limited trust, graceful degradation)
- Command output (displayed but not executed)

**Untrusted:**

- Command arguments (may contain malicious strings)
- File contents (may contain malicious content)
- Network responses (may be malicious)

### 18.2 Provider Isolation

Providers run in separate processes with limited IPC capabilities.

**Provider capabilities:**

- Communicate with shell via structured IPC messages
- Request command execution (shell executes on their behalf)
- Access provider-specific cache
- Log messages

**Resource limits:**

- CPU: 100ms for completions, 1s for parsing, 10s for actions
- Cache: 10MB per provider

**Violation handling:**

- Time exceeded: Provider terminated, error displayed
- Repeated failures: Provider disabled with user notification

### 18.3 Action Safety

UI-triggered actions never construct shell command strings.

**Safe pattern:**

```
User clicks "Stop Container" on container "foo"
→ Provider returns: { argv: ["docker", "stop", "foo"] }
→ Shell executes: execve("docker", ["docker", "stop", "foo"])
```

**Prevented attack:**

```
Malicious container named: "foo; rm -rf /"
→ Provider returns: { argv: ["docker", "stop", "foo; rm -rf /"] }
→ Shell executes: execve("docker", ["docker", "stop", "foo; rm -rf /"])
→ Docker receives literal string "foo; rm -rf /" as container name
→ Docker reports: "No such container: foo; rm -rf /"
→ No shell interpretation occurs
```

### 18.4 Sensitive Data Handling

**Environment variables:**

- Variables matching sensitive patterns (PASSWORD, SECRET, TOKEN, KEY, CREDENTIAL) are redacted in:
  - Block metadata snapshots
  - History exports
  - Error reports
  - Debug logs
- Full values remain in live shell state (necessary for functionality)

**Command history:**

- Commands starting with space are not recorded (configurable)
- Commands matching user-defined patterns are not recorded
- History file permissions are 0600 (user read/write only)

**Clipboard:**

- Paste operations are logged in debug mode only
- Sensitive content detection warns before paste (configurable)

**Network:**

- No telemetry without explicit opt-in
- No automatic update checks without explicit opt-in
- Provider registry uses HTTPS with certificate pinning

### 18.5 File Permissions

**Configuration files:**

- `~/.config/nexus/config.toml`: 0644 (world-readable, user-writable)
- `~/.config/nexus/providers/`: 0755 (world-readable, user-writable)

**Data files:**

- `~/.local/share/nexus/history.db`: 0600 (user only)
- `~/.local/share/nexus/blocks/`: 0700 (user only)

**Runtime files:**

- Socket files (if any): 0600 (user only)

---

# Part XV: Compatibility and Migration

## 19. Migration Path

### 19.1 From Bash/Zsh

**Automatic migration (Harvest):**

On first run, Nexus offers to import:

- Environment variables (PATH, EDITOR, etc.)
- Exported functions (converted if POSIX-compatible)

Not imported (requires manual migration):

- Aliases (syntax differs)
- Shell functions (may use unsupported features)
- Prompt configuration (different API)
- Keybindings (different API)
- Plugin configurations (incompatible)

**Migration guide topics:**

- Converting aliases to Nexus format
- Converting prompt functions to Nexus prompt format
- Replacing Bash-specific features with Nexus equivalents
- Setting up providers for tool-specific intelligence
- Configuring keybindings

### 19.2 Script Compatibility

**Running existing scripts:**

- POSIX sh scripts: Run directly with Nexus
- Bash scripts: Run with `/bin/bash` shebang (


Nexus respects shebangs and executes with the specified interpreter)
- Zsh scripts: Run with `/bin/zsh` shebang

**Compatibility checking:**

Nexus can analyze scripts for compatibility:

```bash
nexus --check-compat script.sh
```

Output indicates:
- POSIX-compatible: Will run in Nexus
- Requires Bash: Lists Bash-specific features used
- Requires Zsh: Lists Zsh-specific features used

**Gradual migration:**

Users can continue using Bash/Zsh scripts while adopting Nexus as their interactive shell. Scripts are executed by their designated interpreter; only interactive use requires Nexus compatibility.

### 19.3 Terminal Emulator Compatibility

**terminfo entry:**

Nexus provides a terminfo entry (`nexus-256color`) based on `xterm-256color` with accurate capability descriptions.

**TERM environment variable:**

- Default: `TERM=xterm-256color` (maximum compatibility)
- Optional: `TERM=nexus-256color` (accurate but may not be installed on remote systems)

**Compatibility with terminal applications:**

Tested and verified working:
- Editors: vim, neovim, emacs, nano, micro
- Pagers: less, more, most
- Multiplexers: tmux, screen
- System monitors: htop, top, btop, glances
- File managers: ranger, nnn, lf, mc
- Git interfaces: lazygit, tig
- Other TUIs: k9s, docker-compose, npm/yarn

### 19.4 Shell Feature Comparison

| Feature | POSIX sh | Bash | Zsh | Nexus |
|:--------|:---------|:-----|:----|:------|
| Basic syntax | ✓ | ✓ | ✓ | ✓ |
| Pipelines | ✓ | ✓ | ✓ | ✓ |
| Redirections | ✓ | ✓ | ✓ | ✓ |
| Variables | ✓ | ✓ | ✓ | ✓ |
| Functions | ✓ | ✓ | ✓ | ✓ |
| Control flow | ✓ | ✓ | ✓ | ✓ |
| Indexed arrays | ✗ | ✓ | ✓ | ✓ |
| Associative arrays | ✗ | ✓ | ✓ | ✓ |
| `[[ ]]` extended test | ✗ | ✓ | ✓ | ✓ |
| Process substitution | ✗ | ✓ | ✓ | ✓ |
| Brace expansion | ✗ | ✓ | ✓ | ✓ |
| `shopt` options | ✗ | ✓ | ✗ | ✗ |
| `setopt` options | ✗ | ✗ | ✓ | ✗ |
| Coprocesses | ✗ | ✓ | ✓ | ✗ |
| Loadable builtins | ✗ | ✓ | ✓ | ✗ |
| Block references | ✗ | ✗ | ✗ | ✓ |
| Structured data | ✗ | ✗ | ✗ | ✓ |
| GUI integration | ✗ | ✗ | ✗ | ✓ |

---

# Part XVI: Appendices

## Appendix A: Grammar Specification

### A.1 Lexical Elements

**Tokens:**

```
WORD        = [^|&;<>()$`\"' \t\n]+
NAME        = [a-zA-Z_][a-zA-Z0-9_]*
NUMBER      = [0-9]+
ASSIGNMENT  = NAME '=' WORD?

DQUOTE      = '"' ... '"'
SQUOTE      = "'" ... "'"
BQUOTE      = '`' ... '`'

NEWLINE     = '\n'
SEMI        = ';'
AMP         = '&'
PIPE        = '|'
AND_IF      = '&&'
OR_IF       = '||'

LESS        = '<'
GREAT       = '>'
DLESS       = '<<'
DGREAT      = '>>'
LESSAND     = '<&'
GREATAND    = '>&'
LESSGREAT   = '<>'
CLOBBER     = '>|'

LPAREN      = '('
RPAREN      = ')'
LBRACE      = '{'
RBRACE      = '}'

IF          = 'if'
THEN        = 'then'
ELSE        = 'else'
ELIF        = 'elif'
FI          = 'fi'
FOR         = 'for'
WHILE       = 'while'
UNTIL       = 'until'
DO          = 'do'
DONE        = 'done'
CASE        = 'case'
ESAC        = 'esac'
IN          = 'in'
FUNCTION    = 'function'
```

### A.2 Grammar Rules

```
program         : linebreak complete_commands linebreak
                | linebreak
                ;

complete_commands: complete_commands newline_list complete_command
                | complete_command
                ;

complete_command: list separator_op
                | list
                ;

list            : list separator_op and_or
                | and_or
                ;

and_or          : pipeline
                | and_or AND_IF linebreak pipeline
                | and_or OR_IF linebreak pipeline
                ;

pipeline        : pipe_sequence
                | '!' pipe_sequence
                ;

pipe_sequence   : command
                | pipe_sequence PIPE linebreak command
                ;

command         : simple_command
                | compound_command
                | compound_command redirect_list
                | function_definition
                ;

simple_command  : cmd_prefix cmd_word cmd_suffix
                | cmd_prefix cmd_word
                | cmd_prefix
                | cmd_name cmd_suffix
                | cmd_name
                ;

compound_command: brace_group
                | subshell
                | for_clause
                | case_clause
                | if_clause
                | while_clause
                | until_clause
                ;

subshell        : LPAREN compound_list RPAREN
                ;

brace_group     : LBRACE compound_list RBRACE
                ;

for_clause      : FOR name do_group
                | FOR name sequential_sep do_group
                | FOR name linebreak IN sequential_sep do_group
                | FOR name linebreak IN wordlist sequential_sep do_group
                ;

case_clause     : CASE word linebreak IN linebreak case_list ESAC
                | CASE word linebreak IN linebreak ESAC
                ;

if_clause       : IF compound_list THEN compound_list else_part FI
                | IF compound_list THEN compound_list FI
                ;

while_clause    : WHILE compound_list do_group
                ;

until_clause    : UNTIL compound_list do_group
                ;

function_definition: name LPAREN RPAREN linebreak function_body
                   | FUNCTION name linebreak function_body
                   | FUNCTION name LPAREN RPAREN linebreak function_body
                   ;
```

### A.3 Nexus Extensions

**Block references:**

```
block_ref       : '%' NUMBER
                | '%' NUMBER ':' stream_name
                | '%' 'latest'
                | '%' '-' NUMBER
                ;

stream_name     : 'stdout'
                | 'stderr'
                | 'meta'
                ;
```

**Extended test (Bash-compatible):**

```
extended_test   : '[[' test_expr ']]'
                ;

test_expr       : test_expr '&&' test_expr
                | test_expr '||' test_expr
                | '!' test_expr
                | '(' test_expr ')'
                | test_primary
                ;

test_primary    : word '=~' word          /* regex match */
                | word '==' word          /* pattern match */
                | word '!=' word
                | word '<' word
                | word '>' word
                | '-' test_op word
                | word '-' test_op word
                ;
```
