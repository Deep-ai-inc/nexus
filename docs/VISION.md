# Nexus Vision: The Year 3000 Terminal, Built Today

## The North Star

A shell that builds a mental model of you. It remembers everything, sees connections you miss, understands intent not just syntax, and grows smarter over time. Computing feels like thinking.

## Architecture Layers

We build from the bottom up. Each layer enables the ones above.

```
┌─────────────────────────────────────────────────────────────┐
│  INTENT LAYER                                               │
│  "deploy to staging" → understands goal, plans steps        │
├─────────────────────────────────────────────────────────────┤
│  CONTEXT LAYER                                              │
│  Knows your project, patterns, preferences, history         │
├─────────────────────────────────────────────────────────────┤
│  REACTIVE LAYER                                             │
│  Live updates, file watchers, streaming, interactive UI     │
├─────────────────────────────────────────────────────────────┤
│  COMPUTATION GRAPH                                          │
│  Commands as nodes, data flows between, change propagates   │
├─────────────────────────────────────────────────────────────┤
│  PERSISTENT MEMORY                                          │
│  Every command, every output, forever. Time-travel.         │
├─────────────────────────────────────────────────────────────┤
│  STRUCTURED DATA (exists today)                             │
│  Value types, native commands, pipelines                    │
└─────────────────────────────────────────────────────────────┘
```

---

## Layer 1: Persistent Memory

**Goal:** Nothing is forgotten. Any past state is accessible.

### Data Model

```rust
struct Session {
    id: SessionId,
    started_at: Timestamp,
    blocks: Vec<Block>,
}

struct Block {
    id: BlockId,
    command: String,              // What was typed
    parsed_ast: Ast,              // Parsed structure
    input_refs: Vec<BlockId>,     // What blocks this depended on
    output: Option<Value>,        // Structured result
    exit_code: i32,
    started_at: Timestamp,
    duration_ms: u64,
    cwd: PathBuf,
    env_snapshot: HashMap<String, String>,  // Env at execution time
}
```

### Capabilities Unlocked

- **Time travel**: Jump to any point in session history
- **Replay**: Re-run commands with same or different context
- **Search**: "When did I last modify nginx config?"
- **Undo**: Restore previous state (for reversible operations)

### Implementation

1. Add `output: Option<Value>` to Block (UI already has this partially)
2. Add `input_refs` tracking when commands reference previous outputs
3. Persist sessions to SQLite: `~/.nexus/sessions/`
4. Add `history` command with structured search
5. UI: Session timeline view, click to jump

---

## Layer 2: Computation Graph

**Goal:** Commands form a DAG. Outputs flow between nodes. Changes propagate.

### Data Model

```rust
struct ComputationGraph {
    nodes: HashMap<BlockId, ComputationNode>,
    edges: Vec<(BlockId, BlockId)>,  // data flows from → to
}

struct ComputationNode {
    block_id: BlockId,
    inputs: Vec<InputRef>,     // What this node consumes
    output: Option<Value>,     // Cached result
    dirty: bool,               // Needs recomputation?
}

enum InputRef {
    PreviousBlock(BlockId),           // $3, $prev
    BlockField(BlockId, String),      // $3.files
    LiveSource(LiveSourceId),         // file watcher, etc.
}
```

### Syntax (POSIX-compatible)

```bash
ls -la                          # Block 1, output stored
| where size > 1MB              # Implicitly uses $prev (block 1)
$1 | sort name                  # Explicitly reference block 1
$prev.files | head 5            # Reference field of previous output
```

### Capabilities Unlocked

- **Reference any output**: `$1`, `$prev`, `$last_success`
- **Change propagation**: Edit block 1 → blocks 2,3 can recompute
- **Lazy evaluation**: Don't compute until result is needed
- **Caching**: Expensive operations cached, invalidated on input change

### Implementation

1. Parser: Recognize `$N`, `$prev`, `$name` as block references
2. Parser: `|` at line start means "pipe from previous output"
3. Evaluator: Resolve references to stored Values
4. Shell state: Track dependency graph
5. UI: Show dependency arrows between blocks (optional)
6. UI: "Recompute" button on blocks with stale inputs

---

## Layer 3: Reactive Layer

**Goal:** Pipelines stay alive. Data updates in real-time.

### Data Model

```rust
enum LiveSource {
    FileWatcher { path: PathBuf, pattern: Option<Glob> },
    ProcessOutput { pid: Pid },
    NetworkStream { url: Url },
    Timer { interval: Duration },
    Webhook { port: u16, path: String },
}

struct ReactivePipeline {
    source: LiveSource,
    transforms: Vec<BlockId>,  // Pipeline stages
    sink: BlockId,             // Final output block
}
```

### Syntax

```bash
watch ~/Downloads | where ext == "pdf" | notify "New PDF: {name}"
tail -f /var/log/app.log | where level == "error" | alert
every 5s | http GET /api/health | where status != 200 | alert
```

### Capabilities Unlocked

- **Live dashboards**: System monitor that updates in place
- **File triggers**: React to filesystem changes
- **Streaming processing**: Process logs in real-time
- **Polling made easy**: `every 5s` as a source

### Implementation

1. Add `watch` command using notify/fsevents
2. Add `every` command as interval source
3. Blocks can be "streaming" - UI updates in place
4. Pipeline stages process events as they arrive
5. UI: Streaming blocks show live content, pause/resume

---

## Layer 4: Context Layer

**Goal:** The shell knows your world. It learns and remembers.

### Data Model

```rust
struct Context {
    // Environment detection
    project: Option<ProjectContext>,
    git: Option<GitContext>,

    // Learned patterns
    command_frequency: HashMap<String, u32>,
    command_sequences: Vec<(String, String, u32)>,  // A followed by B, count
    time_patterns: HashMap<String, Vec<TimeOfDay>>, // When you run what

    // User model
    preferences: UserPreferences,
    corrections: Vec<Correction>,  // When user fixed a mistake
}

struct ProjectContext {
    root: PathBuf,
    kind: ProjectKind,  // Rust, Node, Python, Go, etc.
    config_files: Vec<PathBuf>,
    scripts: HashMap<String, String>,  // package.json scripts, Makefile targets
}

struct GitContext {
    repo_root: PathBuf,
    branch: String,
    remotes: Vec<String>,
    dirty: bool,
    recent_commits: Vec<CommitSummary>,
}
```

### Capabilities Unlocked

- **Smart completions**: Suggest based on project type and history
- **Ambient awareness**: Prompt shows relevant info without asking
- **Pattern detection**: "You usually run tests after this"
- **Project commands**: `npm run` scripts, Makefile targets as first-class

### Implementation

1. Detect project type on `cd` (look for Cargo.toml, package.json, etc.)
2. Parse project config for available commands/scripts
3. Track command frequency and sequences
4. Store context in `~/.nexus/context.db`
5. Prompt integration: Show branch, project, dirty state
6. Completion integration: Suggest contextually

---

## Layer 5: Intent Layer

**Goal:** Understand what the user wants, not just what they typed.

### Capabilities

```bash
> deploy api to staging
# Shell understands: build → push → deploy
# Shows plan:
#   1. docker build -t api:latest .
#   2. docker push registry/api:latest
#   3. kubectl apply -f k8s/staging/
# [Run] [Edit] [Cancel]

> find that config file I edited yesterday
# Searches history + filesystem
# Returns: "Did you mean ~/.config/nexus/settings.toml (modified 18h ago)?"

> why is the build failing
# Reads recent error output
# AI analyzes: "The build fails because module 'foo' is missing..."
```

### Data Model

```rust
enum IntentResult {
    DirectExecution(Command),           // Clear intent, just run
    PlanProposal(Plan),                 // Complex intent, show plan
    Clarification(Vec<Question>),       // Ambiguous, ask user
    Search(SearchResults),              // Information retrieval
    Explanation(String),                // Understanding request
}

struct Plan {
    goal: String,
    steps: Vec<PlannedStep>,
    risks: Vec<Risk>,
    reversible: bool,
}

struct PlannedStep {
    description: String,
    command: Command,
    preview: Option<DiffPreview>,  // What will change
}
```

### Implementation

1. AI integration receives full context (recent blocks, project, history)
2. Natural language commands go through intent parser
3. Complex intents generate plans for approval
4. Preview system shows diffs before destructive ops
5. Learn from corrections: user edits plan → remember for next time

---

## Implementation Phases

### Phase 1: Persistent Memory (Foundation)
- [ ] Store block outputs in Block struct
- [ ] Add session persistence (SQLite)
- [ ] Implement `$prev` reference in parser
- [ ] Implement `|` at line start syntax
- [ ] Add `history search` command
- [ ] UI: Show output in collapsed block, expand on click

### Phase 2: Computation Graph
- [ ] Track `input_refs` on blocks
- [ ] Implement `$N` and `$name` references
- [ ] Add dependency resolution in evaluator
- [ ] Mark blocks dirty when inputs change
- [ ] UI: Recompute button, dependency visualization

### Phase 3: Reactive Pipelines
- [ ] Implement `watch` command with fsevents
- [ ] Implement `every` interval source
- [ ] Streaming block UI (updates in place)
- [ ] Backpressure handling for fast sources

### Phase 4: Context Awareness
- [ ] Project detection on directory change
- [ ] Parse package.json/Cargo.toml/Makefile for scripts
- [ ] Git context integration
- [ ] Command frequency tracking
- [ ] Smart completions based on context

### Phase 5: Intent Understanding
- [ ] Natural language command parsing
- [ ] Plan generation for complex intents
- [ ] Preview/diff system for destructive ops
- [ ] Learn from user corrections

---

## What Makes This Different

| Traditional Shell | Nexus |
|-------------------|-------|
| Stateless - outputs vanish | Everything remembered forever |
| Commands are strings | Commands are nodes in a graph |
| Text in, text out | Structured data, rich visualization |
| Static - run once | Reactive - live updates |
| Context-blind | Knows your project, patterns, history |
| Syntax-driven | Intent-aware |
| Tool you use | Partner that learns you |

---

## The Seed of Year 3000

Each layer plants a seed:

1. **Persistent Memory** → Nothing forgotten, time is navigable
2. **Computation Graph** → Outputs are alive, connected, flowing
3. **Reactive Layer** → The system is awake, responding, present
4. **Context Layer** → It knows you, your world, your patterns
5. **Intent Layer** → Thinking together, not commanding

The terminal stops being a tool you use and becomes an extension of how you think about computing.
