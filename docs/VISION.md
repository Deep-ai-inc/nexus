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
**Pain:** Shift-click to select, invisible newlines, pasted commands execute immediately (security risk).

**Solution:** Native clipboard. Click-drag to select. Tables copy as TSV. Pasted text goes to input buffer, not executed until Enter.

**Status:** 🔨 In progress.

**Implementation:**
- [ ] Native text selection with mouse
- [ ] Copy structured data (tables → TSV, records → JSON)
- [ ] Paste goes to input, never auto-executes
- [ ] Right-click → Copy menu

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

## Priority Order

### P0 — Core Experience
1. **#7 Mouse Support** — Click, select, right-click menus
2. **#6 Discoverability** — Command palette
3. **#15 History** — SQLite persistence, search
4. **#5 Copy/Paste** — Native, safe, structured

### P1 — Polish
5. **#14 Progress** — Spinners, progress bars
6. **#19 Autocomplete** — Context-aware suggestions
7. **#11 Dangerous Globs** — Preview before delete
8. **#12 Sudo Trap** — Re-run with elevation

### P2 — Rich Features
9. **#9 Images/Media** — Inline rendering
10. **#10 Theming** — Settings UI
11. **#4 Unicode** — Verify wide character handling
12. **#8 Reflow** — Edge case handling

### P3 — Advanced
13. **#18 Script Linting** — Syntax warnings
14. **#20 Accessibility** — Screen reader support
15. **#16 SSH** — Remote agent protocol

---

## Measuring Success

For each problem, success means a user who experienced that pain point says "this is so much better."

Not "look at this cool feature" but "I no longer have this problem."
