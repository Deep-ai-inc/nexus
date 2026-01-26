# Nexus Shell Completeness Plan

## Philosophy: Next-Gen Shell, Not Better Bash

Nexus owns **both** the shell and the terminal. This is a superpower. Don't just emulate text output from 1990s POSIX - replace it with UI elements where it makes sense.

**Guiding Principles:**
1. **UI > Text** - Jobs, errors, tables should be visual widgets, not printed strings
2. **Shim, Don't Replace** - For complex tools (curl, tar, ssh), run system binaries
3. **Structured First** - Prioritize `each`/`map` over `xargs`; native math over `$(())`
4. **Discoverable UX** - Smart prompts beat cryptic syntax (`Ctrl+F for sudo` > `!!`)
5. **Lazy Loading** - Shell must start in <10ms; load heavy libs on demand

---

## Phase 1: Muscle Memory + Visual UX

### 1.1 Tab Completion (Programmable)
**Priority:** P0 - Single most important feature

**Implementation:**
- [ ] Path completion with file type icons (leverage `FileEntry` metadata)
- [ ] Command completion (PATH + builtins + native commands)
- [ ] Git-aware completion (branches, remotes, files)
- [ ] Flag completion after command name
- [ ] Fuzzy matching with ranking
- [ ] Fish-style ghost text preview (gray suggestion)

**Files:**
- `nexus-ui/src/app.rs` - Tab key handling, completion popup
- `nexus-kernel/src/completion.rs` (new) - Completion engine

---

### 1.2 Visual Job Control
**Priority:** P0 - Differentiator

**Philosophy:** Don't print `[1]+ Stopped vim` - show it in the UI.

**Implementation:**
- [ ] **Job Sidebar/Status Bar** - Visual indicator for background/stopped jobs
- [ ] **Ctrl+Z** → Job flies into sidebar with icon + command name
- [ ] **Click to manage** - Click job → menu with `fg`, `bg`, `kill` options
- [ ] **Hover for details** - PID, runtime, resource usage
- [ ] **Notification on completion** - Toast/badge when background job finishes
- [ ] **Builtins for scripts** - `fg`, `bg`, `jobs`, `kill` work for compatibility
  - But suppress text output when running interactively (UI handles it)

**Visual Design:**
```
┌─────────────────────────────────────────────┐
│ ~/project $ ls                              │
│ [output...]                                 │
│                                             │
│ ~/project $ _                               │
├─────────────────────────────────────────────┤
│ ⏸ vim file.txt (stopped)  │  ● npm start   │  ← Status bar
└─────────────────────────────────────────────┘
```

**Files:**
- `nexus-ui/src/widgets/job_indicator.rs` (new)
- `nexus-kernel/src/process/job.rs` - Enhance job tracking
- `nexus-kernel/src/eval/builtins.rs` - `fg`, `bg`, `jobs`, `kill`

---

### 1.3 Command History (Up/Down + Ctrl+R)
**Priority:** P0

**Implementation:**
- [ ] Up/Down arrow navigation
- [ ] Ctrl+R incremental search with live preview
- [ ] Prefix matching (type `git` then Up → cycles git commands)
- [ ] `!!`, `!$`, `!n` for compatibility (but not promoted in UI)

**Files:**
- `nexus-ui/src/app.rs` - Key handling
- `nexus-kernel/src/persistence.rs` - Search methods

---

### 1.4 Brace Expansion
**Priority:** P0 - Power users depend on this

**Implementation:**
- [ ] `{a,b,c}` → `a b c`
- [ ] `{1..10}` → `1 2 3 ... 10`
- [ ] `{1..10..2}` → `1 3 5 7 9`
- [ ] `{a..z}` → alphabet
- [ ] Nested: `{a,b{1,2}}` → `a b1 b2`
- [ ] Combined: `file{1,2}.txt` → `file1.txt file2.txt`

**Files:**
- `nexus-kernel/src/eval/expand.rs` - `expand_braces()`

---

### 1.5 `kill` Builtin
**Priority:** P0 - Daily necessity

**Implementation:**
- [ ] `kill PID`, `kill -9 PID`, `kill -KILL PID`
- [ ] `kill %n` - Kill by job spec
- [ ] `kill -l` - List signals
- [ ] Support multiple PIDs

**Note:** Interactive use should prefer clicking job indicator → Kill

---

## Phase 2: Smart Error Recovery UX

### 2.1 Permission Denied Recovery
**Priority:** P0 - Differentiator

**Philosophy:** Don't make users learn `sudo !!`. Show them a button.

**Implementation:**
- [ ] Detect "permission denied" in stderr
- [ ] Show inline prompt: `⚠️ Permission denied. [Retry with sudo] [Cancel]`
- [ ] Hotkey hint: `Press Ctrl+S to retry with sudo`
- [ ] Remember preference per command pattern

**Visual:**
```
$ rm /etc/hosts
rm: /etc/hosts: Permission denied

┌─────────────────────────────────────────┐
│ ⚠️ Permission denied                     │
│ [Retry with sudo]  [Copy error]  [Help] │
└─────────────────────────────────────────┘
```

---

### 2.2 Command Not Found Recovery
**Priority:** P0 - Differentiator

**Implementation:**
- [ ] Fuzzy match against known commands: "Did you mean `git`?"
- [ ] Detect installable packages: "Install with `brew install foo`? [Yes] [No]"
- [ ] Suggest typo fixes inline

**Visual:**
```
$ gti status
Command not found: gti

┌──────────────────────────────────────┐
│ Did you mean?                        │
│ [git status]  [Install gti]  [Help]  │
└──────────────────────────────────────┘
```

---

### 2.3 Destructive Command Confirmation
**Priority:** P1

**Implementation:**
- [ ] Detect `rm -rf`, `chmod -R 777`, etc.
- [ ] Show preview: "This will delete 47 files. [Proceed] [Cancel] [Show files]"
- [ ] Glob preview before execution

---

## Phase 3: Interactive Output Widgets

### 3.1 Interactive Tables
**Priority:** P0 - Already partially done, enhance it

**Current:** Table widget exists with clickable headers for sorting.

**Enhancements:**
- [ ] **Right-click context menu** on rows
  - `ps` row → Kill, Send Signal, Open in Activity Monitor
  - `ls` row → Open, Delete, Rename, Copy Path
  - `git log` row → Checkout, Cherry-pick, Copy SHA
- [ ] **Multi-select** with Shift+Click
- [ ] **Filter input** - Type to filter visible rows
- [ ] **Column resize** - Drag column borders
- [ ] **Copy as TSV/JSON** - Cmd+C copies structured data

**Files:**
- `nexus-ui/src/widgets/table.rs` - Enhance interactivity

---

### 3.2 `ps` - Structured Process Listing
**Priority:** P0

**Implementation:**
- [ ] Return `Value::Table` with: PID, USER, CPU%, MEM%, TIME, COMMAND
- [ ] **Right-click row → Kill process**
- [ ] **Sort by any column** (click header)
- [ ] **Filter** - Type to search by command name
- [ ] Lazy load with `sysinfo` crate (only when `ps` is invoked)

**Files:**
- `nexus-kernel/src/commands/ps.rs` (new)

---

### 3.3 `history` - Interactive History Browser
**Priority:** P1

**Implementation:**
- [ ] Return `Value::Table` with: #, TIME, CWD, COMMAND, EXIT_CODE
- [ ] Click row → Insert command into prompt
- [ ] Right-click → Copy, Re-run, Re-run with modifications
- [ ] Filter by typing

---

## Phase 4: Structured Data & Iteration

### 4.1 `each` / `map` / `filter` Iterators
**Priority:** P0 - This is the structured shell advantage

**Philosophy:** `xargs` exists because Unix pipes are text. We have structured data.

**Implementation:**
- [ ] `each { block }` - Execute block for each item, `$it` is current item
- [ ] `map { expr }` - Transform each item
- [ ] `filter { condition }` - Keep items matching condition
- [ ] `reduce { acc, it -> expr }` - Fold into single value
- [ ] `any { condition }` - True if any match
- [ ] `all { condition }` - True if all match

**Examples:**
```bash
# Old way (xargs)
find . -name "*.txt" | xargs rm

# Nexus way
find . -name "*.txt" | each { rm $it }

# Transform
ls | map { $it.name | uppercase }

# Filter
ps | filter { $it.cpu > 50 } | each { kill $it.pid }
```

**Files:**
- `nexus-kernel/src/commands/each.rs` (new)
- `nexus-kernel/src/commands/map.rs` (new)
- `nexus-kernel/src/commands/filter.rs` (new)

---

### 4.2 `xargs` (Compatibility Only)
**Priority:** P2 - For pasted scripts

**Implementation:**
- [ ] Basic `xargs` for compatibility
- [ ] `-I {}` placeholder
- [ ] `-P n` parallel execution
- [ ] `-0` null delimiter

**Note:** Don't promote; point users to `each` instead.

---

### 4.3 `open` - Smart File Opener
**Priority:** P0

**Implementation:**
- [ ] Auto-detect file type and parse:
  - `.json` → `Value::Record` or `Value::List`
  - `.csv` → `Value::Table`
  - `.toml`/`.yaml` → `Value::Record`
  - `.txt`/unknown → `Value::String`
- [ ] `open data.json | get users | filter { $it.active }`

**Files:**
- `nexus-kernel/src/commands/open.rs` (new)

---

### 4.4 `from-json` / `to-json` / `from-csv` / `to-csv`
**Priority:** P0 - Already have `from-json`/`to-json`, add CSV

**Implementation:**
- [ ] Ensure `from-json`, `to-json` are robust
- [ ] Add `from-csv`, `to-csv`
- [ ] Add `from-toml`, `to-toml`

---

## Phase 5: Core Shell Completeness

### 5.1 Function Execution + `return`
**Priority:** P0 - Users define functions in .rc files

**Current:** Parser handles functions, evaluator doesn't execute

**Implementation:**
- [ ] Function storage in `ShellState`
- [ ] Function invocation (before PATH lookup)
- [ ] `return [n]` - Exit function with code
- [ ] Positional params: `$1`, `$2`, `$@`, `$*`
- [ ] `local var=value` - Scoped variables

**Files:**
- `nexus-kernel/src/state.rs` - Function storage
- `nexus-kernel/src/eval/mod.rs` - Function dispatch
- `nexus-kernel/src/eval/builtins.rs` - `return`, `local`

---

### 5.2 `break` / `continue`
**Priority:** P0 - Required for loops

**Implementation:**
- [ ] `break [n]` - Exit n levels of loop
- [ ] `continue [n]` - Skip to next iteration

**Approach:** Return `ControlFlow` enum from evaluator instead of just exit code.

---

### 5.3 `case` / `esac`
**Priority:** P1

**Implementation:**
- [ ] Pattern matching with `|` alternatives
- [ ] `*)` default case
- [ ] `;;` terminator

---

### 5.4 FD Redirection (`2>&1`)
**Priority:** P0 - Users need to capture stderr

**Implementation:**
- [ ] `2>&1` - Redirect stderr to stdout
- [ ] `>&2` - Redirect stdout to stderr
- [ ] `2>/dev/null` - Discard stderr
- [ ] `&>file` - Redirect both streams

**Files:**
- `nexus-kernel/src/eval/mod.rs` - Handle fd setup
- `nexus-kernel/src/process/mod.rs` - PTY fd configuration

---

### 5.5 Arithmetic: Native Math Expressions
**Priority:** P0

**Philosophy:** `$(( 1 + 1 ))` is ugly. Support it for compatibility, but allow native math.

**Implementation:**
- [ ] `$((expr))` for compatibility with pasted scripts
- [ ] Native math in expressions: `let x = 1 + 1` or context-aware parsing
- [ ] Operators: `+`, `-`, `*`, `/`, `%`, `**`
- [ ] Comparison: `<`, `>`, `<=`, `>=`, `==`, `!=`
- [ ] Variables in expressions: `$((x + 1))`

**Exploration:** Can we detect `1 + 1` as math in certain contexts without breaking POSIX compat?

---

### 5.6 Parameter Expansion
**Priority:** P0 - Vital for environment handling

**Implementation:**
- [ ] `${var:-default}` - Default if unset
- [ ] `${var:=default}` - Assign default
- [ ] `${var:+alternate}` - Alternate if set
- [ ] `${var:?error}` - Error if unset
- [ ] `${#var}` - String length
- [ ] `${var%pattern}` - Remove suffix
- [ ] `${var#pattern}` - Remove prefix

**Files:**
- `nexus-kernel/src/eval/expand.rs`

---

### 5.7 `[[ ... ]]` Extended Test
**Priority:** P0 - Users don't use old `[`

**Implementation:**
- [ ] Pattern matching: `[[ $str == *.txt ]]`
- [ ] Regex: `[[ $str =~ ^[0-9]+$ ]]`
- [ ] Safe string comparison (no quoting issues)
- [ ] `&&`, `||` inside brackets

---

### 5.8 Process Substitution (`<(cmd)`)
**Priority:** P1 - Common in modern workflows

**Implementation:**
- [ ] `<(cmd)` - Replace with fd path containing output
- [ ] `>(cmd)` - Replace with fd path for input
- [ ] Use case: `diff <(cmd1) <(cmd2)`

---

## Phase 6: Essential Utilities

### 6.1 Native Commands (Build These)

| Command | Priority | Notes |
|---------|----------|-------|
| `chmod` | P0 | Permission issues are #1 blocker |
| `chown` | P1 | Less common but needed |
| `ln` | P1 | Symlinks for configs |
| `stat` | P1 | Already have `FileEntry`, just expose |
| `tee` | P1 | Duplicate output |
| `du` | P1 | Disk usage as table |
| `df` | P1 | Filesystem space as table |
| `readlink` | P2 | Resolve symlinks |
| `file` | P2 | Detect file type |
| `md5sum`/`sha256sum` | P2 | Checksums |
| `base64` | P2 | Encoding |

---

### 6.2 System Binary Shims (Don't Reimplement)

These should run system binaries via PTY:

| Command | Reason |
|---------|--------|
| `curl` | Thousands of edge cases, SSL, proxies |
| `wget` | Same |
| `tar` | Complex format handling |
| `gzip`/`gunzip` | Use system binary |
| `ssh` | Interactive, requires PTY |
| `scp`/`rsync` | Complex protocols |
| `less`/`more` | PTY pager |
| `vim`/`nano` | PTY editors |
| `htop`/`top` | PTY monitors |
| `man` | PTY pager |

**New Native Alternatives (Optional P2+):**
- `fetch` or `http` - Structured HTTP client returning `Value::Record`
- `archive` - Native tar/zip with structured listing

---

### 6.3 Here Documents
**Priority:** P1

**Implementation:**
- [ ] `<<EOF` ... `EOF`
- [ ] `<<'EOF'` (no expansion)
- [ ] `<<-EOF` (strip leading tabs)

---

## Phase 7: Polish

### 7.1 `set -o pipefail`
**Priority:** P1

- [ ] Pipeline fails if any command fails
- [ ] Track exit codes of all stages

---

### 7.2 Traps
**Priority:** P2

- [ ] `trap 'cmd' EXIT` - Run on shell exit
- [ ] `trap 'cmd' INT` - Handle Ctrl+C
- [ ] `trap 'cmd' ERR` - Run on error (bashism)

---

### 7.3 `PROMPT_COMMAND` / Custom Prompts
**Priority:** P2

- [ ] Run command before each prompt
- [ ] `PS1` / `PS2` formatting

**Note:** UI-first approach may replace this with visual status bar

---

## Implementation Order

### Sprint 1: Core Interactivity (Week 1-2)
- [ ] Brace expansion `{a,b}`, `{1..5}`
- [ ] `break`, `continue`
- [ ] `kill` builtin (full)
- [ ] History Up/Down navigation
- [ ] Visual job control foundation (job sidebar)

### Sprint 2: Control Flow + Functions (Week 2-3)
- [ ] Function execution + `return` + `local`
- [ ] `case`/`esac`
- [ ] FD redirection `2>&1`
- [ ] Parameter expansion `${var:-default}`
- [ ] Arithmetic `$((...))`

### Sprint 3: Interactive UX (Week 3-4)
- [ ] Tab completion (paths + commands)
- [ ] Ctrl+R history search
- [ ] Permission denied recovery prompt
- [ ] Command not found suggestions
- [ ] `[[ ... ]]` extended test

### Sprint 4: Structured Iteration (Week 4-5)
- [ ] `each { }` iterator
- [ ] `map { }` transform
- [ ] `filter { }`
- [ ] `open` smart file parser
- [ ] `from-csv` / `to-csv`

### Sprint 5: Essential Utilities (Week 5-6)
- [ ] `chmod` / `chown`
- [ ] `ps` (interactive table)
- [ ] `ln`, `stat`, `tee`
- [ ] `du` / `df`
- [ ] Table right-click context menus

### Sprint 6: Advanced (Week 6+)
- [ ] Ctrl+Z + full `fg`/`bg` flow
- [ ] Process substitution `<(cmd)`
- [ ] Here documents
- [ ] Git-aware tab completion
- [ ] Destructive command preview

---

## Success Metrics

1. **10ms startup** - Shell feels instant
2. **Daily driver** - Can use Nexus exclusively for a week
3. **Scripts work** - Common install scripts execute correctly
4. **RC files load** - `.bashrc` works with minor modifications
5. **Wow moments** - Users notice visual job control, smart error recovery

---

## Dependencies

```toml
# Add lazily / on-demand
sysinfo = "0.30"     # For `ps` - load only when called
csv = "1.3"          # For from-csv/to-csv
toml = "0.8"         # For from-toml/to-toml
```

---

## Anti-Goals (Explicitly Not Doing)

- ❌ Native `curl` reimplementation
- ❌ Native `tar`/`gzip` reimplementation
- ❌ Native `ssh` reimplementation
- ❌ `xargs` as P0 (use `each` instead)
- ❌ Associative arrays (users switch to Python)
- ❌ Complex `${var/pat/rep}` substitution
- ❌ `fc` command (UI history is better)
- ❌ `select` menus (build native UI picker)
- ❌ `coproc` (niche)

---

## The Nexus Advantage

| Legacy Shell | Nexus |
|--------------|-------|
| `[1]+ Stopped vim` text | Visual job sidebar |
| `sudo !!` | "Retry with sudo?" button |
| `ps aux \| grep node` | Interactive filterable table |
| `xargs` text parsing | `each { }` structured iteration |
| `$(( 1 + 1 ))` | Native math expressions |
| `curl ... \| jq .` | `http get url \| get .field` |
| `Command not found` | "Did you mean? [Install?]" |

This is what makes Nexus a **next-gen shell**, not just a better bash.
