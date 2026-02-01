# Nexus: Solving Terminal Friction

This document tracks the core problems with modern terminals and how Nexus addresses each one.

---

## Core Architecture

These foundational design choices enable everything else:

### Block-Based Output
Commands and their output are organized into discrete blocks you can navigate, re-run, copy, or reference. Each block is a first-class UI element with its own state, not just text in a scrollback buffer.

**Status:** ✅ Working. Blocks track command, output, exit code, duration, and can render structured data.

**Implementation:**
- [x] Commands produce discrete Block objects
- [x] Blocks can be collapsed/expanded
- [x] Status indicators (running/success/failed)
- [x] Kill button for running blocks
- [ ] Re-run button for completed blocks
- [ ] Copy block output (text, JSON, or TSV for tables)
- [ ] Block permalinks/sharing

---

### AI Integration
Built-in AI that can explain commands, suggest fixes, and act as a coding agent that can edit files while you steer it mid-execution.

**Status:** 🔨 In progress. Agent panel working, tool use implemented.

**Implementation:**
- [x] Agent panel with streaming responses
- [x] Tool use (file read/write, bash execution)
- [x] Interrupt/cancel agent mid-execution
- [ ] "Explain this error" contextual action
- [ ] "Fix this" suggestions on command failure
- [ ] Steer agent while running (persistent input)
- [ ] Project-specific agent instructions (NEXUS.md)

---

### Modern Input
Text editor-style input with proper cursor movement, selections, and multi-line editing. Not the traditional readline approach.

**Status:** 🔨 In progress. Basic editing works, multi-line needs work.

**Implementation:**
- [x] Standard cursor movement (arrows, Cmd+Left/Right)
- [x] Text selection (Shift+arrows)
- [ ] Multi-line input with Shift+Enter
- [ ] Syntax highlighting in input
- [ ] Bracket matching
- [ ] History search inline (not modal popup)

---

## The 20 Problems

### 1. Escape Sequence Hell
**Pain:** Parsing ANSI codes, stripping colors, writing cursor movements. Feels like 1970s programming.

**Solution:** Native commands return structured `Value` types. The UI renders them directly. No escape codes involved.

**Status:** ✅ Working for native commands. Legacy commands still produce escape codes (handled by terminal emulator).

---

### 2. Inconsistent Keybindings
**Pain:** Ctrl+C kills processes here, copies text everywhere else. Alt+Backspace works in one terminal, prints garbage in another.

**Solution:** Native GUI application. We control all keybindings. Standard CUA keybindings work (Ctrl+C copies, Ctrl+V pastes). Process interruption via dedicated key or button.

**Status:** 🔨 In progress. Need to implement proper keybinding system.

**Implementation:**
- [ ] Ctrl+C → copy selected text
- [ ] Ctrl+Shift+C or Cmd+. → interrupt process
- [ ] Standard navigation keys work everywhere
- [ ] Configurable keybinding preferences

---

### 3. The TERM Variable
**Pain:** Apps break, colors vanish, backspace stops working if TERM is wrong.

**Solution:** We control the PTY. TERM is always set correctly. User never sees or configures it.

**Status:** ✅ Working. PTY sets `TERM=xterm-256color` automatically.

---

### 4. Unicode/Wide Character Chaos
**Pain:** Emojis break layouts. CJK characters cause cursor drift. Text overlaps.

**Solution:** Native GUI with proper text shaping (using system text layout engine). Character widths calculated correctly using Unicode width properties.

**Status:** ⚠️ Partial. Iced handles basic text well. Need to verify emoji and CJK rendering in tables.

**Implementation:**
- [ ] Test emoji rendering in output
- [ ] Test CJK characters in tables
- [ ] Ensure cursor positioning is correct
- [ ] Use unicode-width crate for alignment calculations

---

### 5. Copy/Paste Friction
**Pain:** Shift-click to select, invisible newlines, pasted commands execute immediately (security risk). Copy a table and get misaligned spaces. Copy across commands and prompts leak in. No format awareness.

**Solution:** Native clipboard with structured data awareness. Text selection works across blocks like a traditional terminal, but what lands on the clipboard is intelligent. Paste is always safe.

**Status:** 🔨 In progress.

#### Selection Model

Text selection works the way you expect — click and drag across output, across block boundaries. The familiar terminal "paint across everything" behavior stays.

**Chrome stripping.** Copied text never includes block status indicators, timestamps, or prompt decorations (the `➜ ~` prefix). You get just content. When a selection spans across a command line, the command text itself is included (e.g., `grep "foo"`) but the prompt chrome is not — so the result reads like a clean shell transcript.

**WYSIWYG for truncated output.** If a block was paginated or visually truncated (e.g., `less`, a long `cat`), selection copies exactly what is rendered on screen. "Copy Full Output" lives in the right-click menu for when you want everything.

#### Structured Copy

**Tables produce multi-format clipboard entries.** When a selection lands entirely within a table, the clipboard gets three simultaneous representations: plain text (space-aligned), TSV (for spreadsheets), and JSON array (for code). The receiving app picks the best format automatically.

**Partial table selection snaps to columns.** Select within a table and Nexus snaps to cell boundaries. Select three cells in the PID column → `1234\n5678\n9012`, not half-truncated rows. Standard click+drag in tables behaves like a spreadsheet (cell selection). Alt+drag forces raw text selection ignoring cell boundaries.

**Header inclusion.** Partial table copies (e.g., rows 5–10) do not include the header row by default. Hold Shift while copying (or toggle in settings) to include headers. Rationale: spreadsheet users want headers, text editor users don't — the default should be the less surprising behavior (no phantom header row), with an easy override.

**Mixed-region selection.** When a selection spans a table region and a non-table region (or two different structured types), Nexus falls back to plain text for the entire clipboard entry. Attempting to combine TSV + JSON in one clipboard payload would confuse receiving apps. The right-click menu can still offer "Copy Table as TSV" / "Copy JSON Block" individually.

#### Semantic Copy

Beyond text selection, structured output contains discrete **copyable tokens** — paths, PIDs, IP addresses, branch names, URLs. Cmd+click a token to copy it directly without drag-selecting. The value knows what it is: copying a path gives you a properly quoted path, copying a PID gives you just the number.

#### Block-Level Copy Actions

Right-click a block (or use a keyboard shortcut) for options beyond text selection:
- **Copy output** — just the output text
- **Copy command** — just the command that produced it
- **Copy as JSON / TSV / Markdown** — structured export
- **Copy full output** — for truncated/paginated blocks, the complete output
- **Copy command + output** — formatted for sharing (command prefixed with `$ `)

#### Paste Behavior

**Never auto-executes.** Pasted text always goes into the input buffer. Period. No trailing newline triggers execution. When text is pasted, it gets a subtle visual flash/highlight to confirm "I caught this, it's safe, it won't run until you press Enter."

**Multi-line awareness.** Pasting multi-line content auto-enters multi-line input mode. Pasting a single line with a trailing newline strips the newline.

**Paste detection.** Nexus inspects pasted content and shows a non-blocking inline suggestion (accept with Tab, ignore with Enter):
- JSON blob → offers to pretty-print or pipe through `jq`
- File path → offers to `cat` / `ls` it
- URL → offers to `curl` or `open` it
- Stack trace → links each frame to local source files if they exist
- Multi-line shell script → enters multi-line mode
- Command with `$ ` prefix (copied from docs/READMEs) → strips the prefix
- Git SHA → offers to `git show <hash>`
- Issue reference (`JIRA-1234`, `#123`) → offers to open the ticket

#### Clipboard History

Cmd+Shift+V opens a searchable clipboard history. Entries from structured copies show as mini previews (table snippets, JSON previews) rather than raw text walls.

**Implementation:**
- [ ] Text selection across blocks with chrome stripping
- [ ] WYSIWYG copy for truncated/paginated output
- [ ] Multi-format clipboard for table selections (text, TSV, JSON)
- [ ] Cell-snapping selection in tables (Alt+drag for raw override)
- [ ] Header inclusion toggle (Shift+copy or setting)
- [ ] Plain text fallback for mixed-region selections
- [ ] Cmd+click semantic token copy (paths, PIDs, IPs, etc.)
- [ ] Right-click block copy menu (output, command, structured formats)
- [ ] "Copy Full Output" for truncated blocks
- [ ] Paste never executes, visual flash on paste
- [ ] Multi-line paste enters multi-line input mode
- [ ] Paste detection with inline suggestions (JSON, paths, URLs, SHAs, issue refs)
- [ ] `$ ` prefix stripping on paste
- [ ] Clipboard history panel (Cmd+Shift+V) with structured previews

---

### 6. No Discoverability
**Pain:** Staring at a blank void. If you don't know `tar` exists, you'll never find it.

**Solution:**
- Command palette (Cmd+K) to search all commands
- Inline help as you type
- Contextual suggestions based on history and directory
- Browsable command list in sidebar

**Status:** 🔨 Planned.

**Implementation:**
- [ ] Command palette UI (fuzzy search all commands)
- [ ] `help` command with browsable categories
- [ ] Inline suggestions based on partial input
- [ ] "Did you mean?" for typos
- [ ] Context-aware suggestions (in git repo → git commands more prominent)

---

### 7. Poor Mouse Support
**Pain:** Scrolling in vim vs scrolling the buffer. Which layer am I in? Click does nothing useful.

**Solution:** It's a GUI. Mouse works everywhere consistently:
- Click to select text
- Click table headers to sort
- Right-click for context menus
- Scroll works on the visible content, not a hidden buffer layer

**Status:** 🔨 In progress.

**Implementation:**
- [ ] Text selection with mouse
- [x] Clickable table headers (sort) ✅
- [ ] Right-click context menus
- [ ] Clickable paths, URLs, PIDs with actions
- [x] Scroll behavior consistent ✅

---

### 8. Text Reflow Chaos
**Pain:** Resize window, text scrambles. Line breaks in wrong places.

**Solution:** We control layout. Tables resize columns. Text wraps at word boundaries. No scrambling.

**Status:** ⚠️ Partial. Basic layout works, needs polish for edge cases.

**Implementation:**
- [ ] Tables resize columns proportionally
- [ ] Long text wraps intelligently
- [ ] Resize doesn't scramble existing output

---

### 9. No Images or Media
**Pain:** Can't see a chart, preview an image, or view any media without leaving terminal.

**Solution:** `Value::Image`, `Value::Chart`, etc. Rendered inline as native GUI elements.

**Status:** 🔨 Planned.

**Implementation:**
- [ ] Add `Value::Image(Vec<u8>, ImageFormat)` type
- [ ] Render images inline in output
- [ ] `cat image.png` displays the image
- [ ] Chart rendering for data visualization
- [ ] `du | chart pie` syntax

---

### 10. Config File Theming
**Pain:** Edit a text file to change your font. Restart to see changes.

**Solution:** Native settings UI. Pick fonts from a dropdown. Color picker. Live preview.

**Status:** 🔨 Planned.

**Implementation:**
- [ ] Settings panel (Cmd+,)
- [ ] Font picker (system fonts)
- [ ] Color scheme picker with preview
- [ ] Font size slider
- [ ] Padding/spacing controls
- [ ] Changes apply immediately

---

### 11. Dangerous Globs
**Pain:** `rm *.txt` typo becomes `rm * .txt` and deletes everything. No preview, no undo.

**Solution:** Preview glob expansion before executing destructive commands. "These 15 files will be deleted. Proceed?"

**Status:** 🔨 Planned.

**Implementation:**
- [ ] Detect destructive commands (rm, mv to existing)
- [ ] Show preview of affected files before execution
- [ ] Require confirmation for large operations
- [ ] Optional: track deleted files for potential recovery

---

### 12. The Sudo Trap
**Pain:** Type long command, "Permission denied", retype with sudo.

**Solution:** Detect permission errors. Offer "Re-run with sudo?" button/hotkey.

**Status:** 🔨 Planned.

**Implementation:**
- [ ] Detect "Permission denied" or EACCES in output
- [ ] Show "Re-run with sudo?" prompt
- [ ] One-click re-execution with sudo prefix
- [ ] Optional: remember for similar commands

---

### 13. Argument Anarchy
**Pain:** `-f`, `--file`, `f`, `file=` — every command different.

**Solution:** Native commands use consistent `--flag` and `--key=value` style. Help shown inline.

**Status:** ⚠️ Partial. Native commands are consistent. External commands can't be fixed.

**Implementation:**
- [ ] Consistent argument style in all native commands
- [ ] Inline help tooltips as you type
- [ ] Tab completion shows flag descriptions

---

### 14. Silent Commands
**Pain:** `cp` large file. Is it frozen? Working? No feedback.

**Solution:** Native commands emit progress events. UI shows spinners, progress bars.

**Status:** 🔨 In progress.

**Implementation:**
- [ ] Progress event type in ShellEvent
- [ ] UI renders progress bars
- [ ] Spinner for commands with unknown duration
- [ ] Native `cp`, `mv` emit progress for large files

---

### 15. History Amnesia
**Pain:** Limited history, not synced between tabs, can't find old commands.

**Solution:** SQLite-backed history. Infinite. Full-text search. Synced across sessions.

**Status:** ✅ Done.

**Implementation:**
- [x] SQLite database at ~/.nexus/nexus.db ✅
- [x] Store: command, timestamp, cwd, exit code, duration ✅
- [x] Full-text search across all history ✅
- [x] `history search <pattern>` command ✅
- [ ] Ctrl+R fuzzy search UI
- [x] Synced across sessions ✅

---

### 16. SSH Latency
**Pain:** Remote shell feels sluggish. Every keystroke round-trips to server.

**Solution:** Long-term: Nexus agent on remote, structured data over wire, local rendering. Short-term: local echo with misprediction correction.

**Status:** 📋 Future.

**Implementation:**
- [ ] Local echo for typing (predict what will appear)
- [ ] nexus-agent binary for remote systems
- [ ] Protocol for structured data over SSH
- [ ] Local UI renders remote structured data

---

### 17. Buffer Limits
**Pain:** Program outputs 10,000 lines, terminal remembers 1,000. Lost forever.

**Solution:** Everything persisted to disk. Scroll back as far as you want. Search all output.

**Status:** 🔨 In progress. (SQLite schema ready, block persistence next)

**Implementation:**
- [x] Store block outputs in memory ✅
- [x] SQLite schema for blocks table ✅
- [ ] Persist blocks to SQLite on completion
- [ ] Lazy loading for old blocks
- [ ] Search across all output
- [ ] Export block output to file

---

### 18. Shell Script Fragility
**Pain:** Whitespace sensitivity, quoting hell, silent failures.

**Solution:** Maintain bash compatibility but add guardrails:
- Syntax highlighting shows errors
- Lint warnings for common mistakes
- Eventually: optional stricter syntax mode

**Status:** ⚠️ Partial. Parser works, need lint/highlighting.

**Implementation:**
- [ ] Syntax highlighting in input
- [ ] Lint warnings (unquoted variables, etc.)
- [ ] ShellCheck integration for scripts
- [ ] Clear error messages with suggestions

---

### 19. Dumb Autocomplete
**Pain:** Suggests files when you need git branches. No context awareness.

**Solution:** Context-aware completion. Know the command being typed, suggest relevant things.

**Status:** 🔨 Planned.

**Implementation:**
- [ ] Parse partial command to understand context
- [ ] Git commands → suggest branches, remotes, files
- [ ] Docker commands → suggest containers, images
- [ ] Path arguments → suggest files
- [ ] Flag arguments → suggest valid flags with descriptions

---

### 20. Accessibility Gaps
**Pain:** Screen readers can't parse ncurses UIs. Dynamic regions confuse assistive tech.

**Solution:** Native GUI with proper accessibility tree. Standard widget semantics.

**Status:** 🔨 Planned.

**Implementation:**
- [ ] Verify Iced accessibility support
- [ ] Proper ARIA-like roles for custom widgets
- [ ] Tables announced correctly
- [ ] Screen reader testing
- [ ] High contrast mode

---

### 21. Drag and Drop Is Nonexistent
**Pain:** Terminals have zero drag and drop support. You can't drag a file in, drag output out, or move data between blocks. Everything requires manual copying and retyping paths.

**Solution:** Full native drag and drop, leveraging Nexus's structured `Value` types and block-based architecture to make drops context-aware.

**Status:** 🔨 Planned.

#### From Outside → Nexus
- Drop files from Finder onto the **input bar** → inserts properly quoted absolute paths. Multiple files → space-separated quoted paths.
- Drop files onto a **running block** → sends the path to stdin or opens it in that context.
- Drop files onto the **agent panel** → attaches as context for the AI conversation.
- Drop an image → preview inline if native command supports `Value::Image`, otherwise insert path.
- Drop text from other apps onto input → pastes into input buffer at cursor (never auto-executes).
- Drop a URL → offers: `curl`, `open`, or insert as text.
- Drop a folder onto the **tab bar** → `cd` into it in a new tab. This is how you open a project.

#### Within Nexus (Block-to-Block)
- Drag a table row from `ps aux` output → drops the PID. Nexus knows the schema, so it extracts the semantically useful field, not raw text.
- Drag an entire block → drops the full output, or offers a choice: raw text, JSON, the command itself.
- Drag a file path from `ls` output → inserts the quoted path. Knows it's a path, not arbitrary text.
- Drag blocks to **reorder** them in session history, or pin important results to the top.
- Drag a block to a **second pane** → side-by-side comparison of two command outputs.
- Drag a block to the **tab bar** → pins it as a persistent reference panel.
- Drag one block onto another → proposes piping them: `cmd1 | cmd2`. Visually building a pipeline.

#### From Nexus → Outside
- Drag a file listing row to Finder → copies/moves the actual file.
- Drag block output to Finder → creates a file with that content (auto-named, e.g., `ps-aux-2026-01-31.tsv`).
- Drag a table block into a spreadsheet → exports as TSV/CSV, not raw terminal text with ANSI garbage.
- Drag a code block into an editor → clean text, no line numbers or prompts.

#### Visual Design
- **Drop targets glow** contextually — input bar, agent panel, block gutters, tab bar all highlight when dragging something relevant.
- **Ghost preview** — while dragging, a small preview shows what will happen: "Insert path", "Pipe as stdin", "Open in new tab".
- **Smart field extraction** — when dragging from a table, hold a modifier key to pick which column to extract (PID vs. process name vs. CPU%).
- **Undo** — every drag-and-drop action is undoable with Cmd+Z.

**Why this is impossible elsewhere:** Traditional terminals can't do this because everything is flat text in a scrollback buffer. There's no concept of "this region is a PID" or "this block is a table." Nexus already has `Value` types and discrete blocks — drag and drop is a natural extension of that structure.

**Implementation:**
- [ ] Accept file drops from OS onto input bar (insert quoted paths)
- [ ] Accept file drops onto agent panel (attach as context)
- [ ] Accept folder drops onto tab bar (cd in new tab)
- [ ] Accept text/URL drops onto input bar
- [ ] Drag table rows/cells between blocks (smart field extraction)
- [ ] Drag blocks to reorder, pin, or open in split pane
- [ ] Drag block onto block to compose pipelines
- [ ] Export blocks to Finder as files (TSV/JSON/text)
- [ ] Export table blocks to external apps as structured data
- [ ] Drop target highlighting and ghost previews
- [ ] Modifier key for column selection when dragging from tables
- [ ] Undo support for all drag-and-drop actions

---

### 22. Filesystem Operations Are Irreversible
**Pain:** `rm` is permanent. `mv` to the wrong place requires remembering where it was. One typo and data is gone. The terminal gives you unlimited power with zero safety net.

**Solution:** Every built-in command that modifies the filesystem records a reversible undo plan. Cmd+Z on a block reverses its side effects. Third-party commands can participate through a plugin system and selective filesystem watching.

**Status:** 🔨 Planned.

#### Built-in Commands (Full Undo)

All Nexus-native filesystem commands are Rust implementations that return structured `Value` types. They don't shell out to `/bin/rm`. They record their own undo plans:

- **`rm`** — Moves files to `~/.nexus/trash/<timestamp>-<hash>/` instead of unlinking. Records original path, permissions, and ownership in a manifest. The block shows "Deleted 3 files" but the undo plan is stored silently alongside the block in SQLite.
- **`mv`** — Records source and destination. Undo is `mv` in reverse.
- **`cp`** — Records what was created. Undo deletes the copies.
- **`mkdir`** — Records created directories. Undo removes them (if still empty, warns otherwise).
- **`chmod` / `chown`** — Records previous mode/owner. Undo restores it.
- **File writes** (from the AI agent's edit tool, or a native `sed`-equivalent) — Snapshots the original file content before modification using **reflink (CoW) copies** where supported (APFS on macOS, Btrfs/XFS on Linux). A `cp --reflink=always` creates an instant snapshot consuming zero extra disk until the original diverges. This makes undo for file edits instantaneous even on multi-gigabyte files. Falls back to a regular copy on filesystems without reflink support.

Each block gets an **undo plan** — a structured list of reverse operations stored alongside the block in SQLite. Not a diff, not a journal — a concrete list: "move this file back here", "restore this content", "delete this copy."

#### The Sudo Barrier

If you run `sudo rm /etc/nginx/sites-enabled/default`, Nexus runs as your user and can't move that file to `~/.nexus/trash/`. Solution:

- Nexus detects `sudo` in the command.
- It creates a temporary trash location inside the target's parent directory (or `/tmp`), and uses `sudo mv` to move the file there, then records the undo plan.
- When you Cmd+Z a sudo block, Nexus prompts for the password (or uses the cached sudo timestamp) to execute the restoration with elevated privileges.

#### External / Third-Party Commands (Best-Effort Undo)

Nexus can't control what `git`, `docker`, or arbitrary scripts do internally. Three mechanisms provide undo, applied in order of preference:

**1. Plugin system (preferred).** Third-party tools register an undo provider with pattern-matched commands and corresponding reverse operations:
- A `git` plugin knows that `git checkout -- file` can be undone via reflog.
- A `docker` plugin knows how to handle container operations.
- Community-maintained plugins are safer and faster than filesystem diffing. A `git.lua` that maps `git checkout` → `git restore` is better than trying to diff the `.git` directory.

**2. Pre-snapshot heuristic.** Nexus parses the command and if it matches a known destructive pattern (`git checkout`, `docker rm`, `make clean`, commands touching known files), it snapshots affected files *before* execution. Opt-in per command pattern and configurable. False positives cost a bit of disk. False negatives mean no undo — no worse than today. Uses reflink copies when available.

**3. Filesystem watch (selective).** Only triggered if the command matches a configured pattern (e.g., `make`, `npm install`, `tar`). Snapshots the working directory's file tree metadata (paths, sizes, mtimes) before execution, diffs after. Heavy watching of the full subtree for every unknown command would introduce lag — so this is pattern-gated, not universal.

For unknown commands with no plugin and no watch pattern, Nexus simply doesn't offer undo. This is honest and no worse than every terminal today.

#### Undo UX

- Every block with a reversible operation shows a subtle **undo icon** in the gutter (visible on hover).
- **Hover to preview.** Hovering the undo icon shows a tooltip with the exact plan: "Undo will: Restore `data.csv` to `/Users/you/project/`, delete `data.json`." This builds trust — users can see exactly what will happen before committing.
- **Cmd+Z** undoes the most recent reversible block. Repeated Cmd+Z walks back through the undo stack. Each press reverses one command's effects (block-scoped, not character-scoped).
- **`undo`** is also a native command: `undo` (last block), `undo b3` (specific block), `undo --list` (show all undoable operations with their age and scope).

#### Conflict Detection

Before executing an undo, Nexus checks whether the world has changed:

- You `rm foo.txt`, then something else creates a new `foo.txt` → warns: "foo.txt already exists and differs from the original. Overwrite / Keep both / Cancel?"
- You `mv a.txt b.txt`, then modify `b.txt` → warns: "b.txt has been modified since it was moved. Restore original at a.txt, or move current version back?"
- Undo plans whose preconditions can't be verified (trash files garbage-collected, original location deleted) are marked as expired with an explanation.

#### Trash Management

Trash lives at `~/.nexus/trash/` with a manifest per entry (original path, timestamp, size, permissions).

- **Garbage collection:** configurable retention period (default: 30 days) and disk quota (default: 1 GB). Oldest items evicted first when either limit is hit.
- **`trash list`** — show trashed items with original paths and age.
- **`trash restore <item>`** — restore to original location.
- **`trash empty`** — permanently delete all trashed items.
- **Trash is browseable via autocomplete.** `cp ~/.nexus/trash/<Tab>` completes deleted files by their *original names* (e.g., `data.csv`), mapping transparently through the hash structure. The trash acts as a virtual filesystem — you don't need to know the internal storage format.

**Implementation:**
- [ ] Undo plan data structure and SQLite schema
- [ ] Native `rm` → move to trash with manifest
- [ ] Reflink (CoW) snapshots for file edits (APFS/Btrfs/XFS, fallback to regular copy)
- [ ] Native `mv` → record source/dest, reversible
- [ ] Native `cp` → record created files, reversible
- [ ] Native `mkdir`, `chmod`, `chown` → record previous state, reversible
- [ ] Sudo detection: trash via elevated temp directory, undo with sudo prompt
- [ ] Undo gutter icon with hover-to-preview tooltip
- [ ] Cmd+Z block-scoped undo with stack
- [ ] `undo` native command (last, specific block, --list)
- [ ] Conflict detection and interactive resolution prompts
- [ ] Plugin/hook system for third-party undo providers
- [ ] Pre-snapshot heuristic for known destructive external commands
- [ ] Selective filesystem watch for configured command patterns
- [ ] Trash management commands (list, restore, empty)
- [ ] Trash garbage collection (time-based + disk quota)
- [ ] Trash browseable via autocomplete (original filenames)

---

## The Broader Pain: 40 Things People Hate About Terminals

The 22 problems above are architectural. Below is the full landscape of terminal friction that real people experience — the things that make someone close the terminal and open a GUI instead. Many map to existing numbered problems; others are new surface area. Nexus should attempt to address all of them.

### Already addressed by numbered problems above

| Pain point | Covered by |
|---|---|
| Copy/Paste Issues | #5 Copy/Paste |
| No Mouse Support | #7 Mouse Support |
| Terminal Customization | #10 Config File Theming |
| No Undo Button | #22 Filesystem Undo |
| Fear of Breaking Things | #11 Dangerous Globs, #22 Filesystem Undo |
| Wildcard Surprises | #11 Dangerous Globs |
| Permission Issues | #12 The Sudo Trap |
| Lack of Autocomplete | #19 Autocomplete |
| Lack of Visual Progress Bars | #14 Silent Commands |
| History Navigation | #15 History Amnesia |
| Output Scrolling | #17 Buffer Limits |
| Escape Sequence Hell | #1 (structured Values) |
| Command Options/Flags | #13 Argument Anarchy |
| Copying from the Web | #5 (paste detection strips `$ ` prefix) |
| Space in Filenames | #5 (paste detection), #21 (drag inserts quoted paths) |

### New pain points Nexus should solve

#### 23. Unintuitive / Cryptic Commands
**Pain:** `tar -xzvf`, `chmod 755`, `find . -name "*.txt" -exec rm {} \;` — the syntax is hostile to humans. You either memorize it or Google it every time.

**Solution:** Nexus's AI agent is always available. Type what you mean in English, get the command. But more importantly, native commands use readable syntax. And for external commands, inline help and the command palette (#6) surface what's available without memorization.

---

#### 24. Poor Error Messages
**Pain:** `ENOENT`, `segfault`, `exit code 137` — errors that mean nothing to most people.

**Solution:** Native commands return structured `Value::Error` with human-readable messages. For external commands, Nexus intercepts common cryptic errors and annotates them: "exit code 137" → "Killed (out of memory)". The AI agent can explain any error with full context of what was running.

---

#### 25. Invisible Password Entry
**Pain:** Type your sudo password, see nothing. Not even asterisks. "Is it working? Did I type it wrong?"

**Solution:** Show asterisks (or dots) during password entry. Nexus controls the PTY and can detect password prompts (sudo, ssh, etc.) and switch to a masked input mode. Simple, obvious, long overdue.

---

#### 26. Stuck Processes / Exiting Programs
**Pain:** Accidentally opened `vim` and can't get out. A command is running and you don't know how to stop it. `Ctrl+C` doesn't always work.

**Solution:** Every running block has a visible kill button. Nexus detects known "trap" programs (vim, less, man) and shows contextual escape hints: "Press `:q!` to exit vim" or "Press `q` to exit less". For stuck processes, escalation is one click: SIGINT → SIGTERM → SIGKILL.

---

#### 27. Case Sensitivity Confusion
**Pain:** `cd Documents` works but `cd documents` doesn't. File matching is case-sensitive on Linux, case-insensitive on macOS. Inconsistent and surprising.

**Solution:** Autocomplete is case-insensitive by default. When a command fails due to case mismatch, Nexus suggests the correct casing: "Did you mean `Documents`?" Native commands like `ls` can flag case-sensitive matches.

---

#### 28. Navigation Confusion (Paths and Filesystem)
**Pain:** "Where am I?" "What's `..`?" "What's the difference between `./folder` and `/folder`?" The filesystem is invisible — you navigate blind.

**Solution:** The prompt always shows current directory. Native `ls` returns structured `FileEntry` values that are clickable and navigable. The AI agent can explain paths. Long-term: a visual breadcrumb bar and optional sidebar file browser.

---

#### 29. "Command Not Found" / PATH Issues
**Pain:** You installed something, it's on disk, but the terminal can't find it. `$PATH` is a mystery.

**Solution:** When a command isn't found, Nexus searches common installation locations and package managers: "Did you mean `/usr/local/bin/node`? It's not in your PATH." Offers to fix it. The `which` / `where` native command explains exactly what's happening.

---

#### 30. Environment Variables
**Pain:** "What is `$PATH`?" "How do I set one?" "Why did it disappear when I opened a new tab?"

**Solution:** `env` native command returns a structured, searchable table of all environment variables. Setting variables through Nexus persists them properly (writes to the right rc file). The AI can explain any variable.

---

#### 31. Piping and Redirection
**Pain:** `|`, `>`, `>>`, `2>&1`, `< /dev/null` — the plumbing is powerful but the syntax is opaque.

**Solution:** Native commands compose naturally with structured data (pipe a table into a filter, pipe into `sort`). The drag-and-drop pipeline builder (#21 — drag block onto block → proposes pipe) makes composition visual. The AI agent can construct pipelines from plain English descriptions.

---

#### 32. Multi-step Processes
**Pain:** Deploy requires 5 commands in sequence. Miss one and start over.

**Solution:** Nexus supports multi-line input and scripts directly. The AI agent can execute multi-step workflows. Long-term: saved command sequences ("recipes") that can be replayed with one action. Block re-run buttons make repetition trivial.

---

#### 33. Limited Help / Documentation
**Pain:** `man` pages are dense novels. `--help` output flies past. You end up on StackOverflow anyway.

**Solution:** The AI agent *is* the help system — it has context about your command, your directory, your recent output. Native commands have structured help that the UI can render as a browsable reference rather than a wall of text. The command palette (#6) surfaces commands with descriptions.

---

#### 34. Installing Software / Dependencies
**Pain:** `brew install`, `apt-get`, `pip install`, `npm install` — every ecosystem has its own package manager with its own quirks. Dependency conflicts, version mismatches, broken installs.

**Solution:** Nexus can't unify package managers, but it can make the experience less painful. Detect failed installs and explain what went wrong. The AI agent knows how to install most tools on your platform. Long-term: a `install` native command that detects your OS and routes to the right package manager.

---

#### 35. Config File Syntax Breakage
**Pain:** Edit `.zshrc`, make a typo, new terminals won't open. Now you need to fix a config file from a broken shell.

**Solution:** Nexus's own config is a GUI (settings panel, #10). For shell configs, the AI agent can validate syntax before you save. When Nexus detects a broken rc file (shell fails to initialize), it offers to open the file and highlight the error. Nexus itself doesn't depend on rc files to function.

---

#### 36. Zombie / Background Processes
**Pain:** Started something with `&`, forgot about it. `jobs` shows nothing in a new tab. `ps aux | grep` is the last resort.

**Solution:** Native `ps` returns structured `Value::Table` with process info. Nexus tracks child processes per block and can show which blocks spawned background jobs. Kill buttons work on background processes too. Long-term: a jobs panel showing all active processes across tabs.

---

#### 37. Tar/Archive Confusion
**Pain:** `tar -xzvf` is a meme for a reason. Every archive format has different flags.

**Solution:** A native `extract` command that auto-detects the format (tar, gz, bz2, xz, zip, 7z) and does the right thing. No flags needed. `extract archive.tar.gz` just works. Shows progress (#14) and lists what was extracted as structured output.

---

#### 38. Prompt Clutter
**Pain:** A prompt showing `user@hostname:/very/deep/nested/directory/structure/here $` leaves no room to type.

**Solution:** Nexus controls the prompt. It's clean by default — just the last directory component. Full path is always visible in the window title or a breadcrumb bar, not eating your input space. Customizable via settings, not rc file hacking.

---

#### 39. Text Editors in Terminal
**Pain:** vi keybindings, nano's `^X` meaning Ctrl+X, emacs pinky. Terminal editors are their own skill tree.

**Solution:** Nexus detects full-screen TUI apps and renders them in the terminal emulator. But for quick edits, the AI agent can edit files directly with its edit tool — you describe the change in English. Long-term: a built-in file editor with familiar GUI keybindings (Cmd+S to save, Cmd+Z to undo).

---

#### 40. SSH Key Management
**Pain:** Generate keys, copy the public key to the right place, add to agent, deal with passphrases. One wrong step and "Permission denied (publickey)."

**Solution:** A native `ssh-setup` command that walks through the process interactively with structured output at each step. The AI agent knows SSH inside and out. When SSH fails, Nexus parses the error and explains exactly what's wrong: "The server rejected your key. Your public key at `~/.ssh/id_ed25519.pub` is not in the server's `authorized_keys`."

---

#### 41. Regex Pain
**Pain:** Regular expressions are a language within a language. Different flavors in grep vs sed vs awk vs PCRE.

**Solution:** The AI agent writes regex for you — describe what you want to match in English. Native `grep` and `find` support simpler glob patterns alongside regex. When a regex fails or matches unexpectedly, Nexus can show what it matched and why (structured match output with highlights).

---

#### 42. Git via CLI
**Pain:** Detached HEAD, merge conflicts as raw diff markers, rebase gone wrong. Git's CLI is powerful but terrifying.

**Solution:** Native `git status` and `git log` already return structured values. Merge conflicts could render as a visual diff with accept-left/accept-right/accept-both buttons. The AI agent can resolve conflicts and explain git states. Long-term: structured output for all git operations with contextual actions (click a branch to checkout, click a commit to diff).

---

#### 43. Clearing the Screen
**Pain:** Not knowing `clear` or `Ctrl+L` exists. Staring at a wall of old output.

**Solution:** Block-based architecture solves this inherently. Each command is isolated. You don't need to clear — just look at the current block. Collapse old blocks to hide them. But `clear` and `Ctrl+L` still work for familiarity.

---

#### 44. Slow Feedback Loop
**Pain:** Run a command, it fails, edit the command, run again. No indication of what changed or why. Feels like trial and error.

**Solution:** Blocks show exit codes, duration, and structured errors. The AI can explain failures. Re-run button makes iteration instant. Native commands give structured error output that points to exactly what's wrong, not just "failed."

---

#### 45. Terminal Multiplexing
**Pain:** SSH disconnects kill your process. tmux/screen have their own keybindings to learn. Splits and sessions are hard to manage.

**Solution:** Long-term: Nexus runs as a persistent daemon. Sessions survive disconnects. Tabs and splits are native GUI features — no tmux keybindings to learn. For remote work, the Nexus SSH agent (#16) keeps sessions alive server-side.

---

## Priority Order

### P0 — Core Experience
1. **#7 Mouse Support** — Click, select, right-click menus
2. **#6 Discoverability** — Command palette
3. **#15 History** — SQLite persistence, search
4. **#5 Copy/Paste** — Native, safe, structured

### P1 — Polish
5. **#21 Drag and Drop** — Native DnD with structured data awareness
6. **#22 Filesystem Undo** — Reversible operations, trash, conflict detection
7. **#14 Progress** — Spinners, progress bars
8. **#19 Autocomplete** — Context-aware suggestions
9. **#11 Dangerous Globs** — Preview before delete
10. **#12 Sudo Trap** — Re-run with elevation

### P2 — Rich Features
11. **#9 Images/Media** — Inline rendering
12. **#10 Theming** — Settings UI
13. **#4 Unicode** — Verify wide character handling
14. **#8 Reflow** — Edge case handling

### P3 — Advanced
15. **#18 Script Linting** — Syntax warnings
16. **#20 Accessibility** — Screen reader support
17. **#16 SSH** — Remote agent protocol

---

## Measuring Success

For each problem, success means a user who experienced that pain point says "this is so much better."

Not "look at this cool feature" but "I no longer have this problem."
