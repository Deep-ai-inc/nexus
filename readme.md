# Nexus: Implementation Scaffold & Roadmap

**Mission:** Build a converged shell runtime where the interpreter, pipeline, and GUI are a single unified system.

## 1. Project Structure (Rust Workspace)

```text
nexus/
├── Cargo.toml              # Workspace definition
├── nexus-kernel/           # Core shell interpreter (Pure Rust, no UI deps)
│   ├── src/parser/         # Tree-sitter integration & AST
│   ├── src/eval/           # AST walker, variable state, builtins
│   └── src/process/        # PTY management, job control, signals
├── nexus-pump/             # I/O handling (The "Middleman")
│   ├── src/pipe/           # Pump threads, ring buffers (memmap)
│   └── src/sniffer/        # Stream heuristics (JSON/magic bytes detection)
├── nexus-ui/               # GPUI frontend
│   ├── src/view_model/     # Projection of Kernel state for rendering
│   └── src/components/     # Block list, Input line, Lenses
├── nexus-api/              # Shared types & IPC protocol for Providers
└── nexus-providers/        # Built-in providers (Git, Docker, FS)
```

## 2. Core Component Scaffold

### A. The Kernel (`nexus-kernel`)
*   **Parser:** Initialize `tree-sitter` with `tree-sitter-bash`. Create a wrapper struct `AstNode` that converts the CST to our simplified AST.
*   **State:** Define `ShellState` struct containing `env: HashMap<String, String>`, `jobs: Vec<Job>`, and `cwd: PathBuf`.
*   **Eval Loop:** Implement a `step(ast_node, state) -> Result<ExitCode>` function. Start with simple `Command` nodes before tackling control flow.
*   **Event Bus:** Use `crossbeam::channel` to emit `ShellEvent` enum variants (`StdoutChunk`, `CommandStarted`, `EnvChanged`) to the UI.

### B. The Pump (`nexus-pump`)
*   **The Pipe:** Implement a function `spawn_pump(reader: File, writer: File, buffer: RingBuffer)`.
*   **Ring Buffer:** Create a fixed-size circular buffer using `memmap2` for zero-copy access by the UI.
*   **Sniffer:** Implement a lightweight `detect_format(&[u8]) -> Format` function that checks magic bytes and JSON structure on the first 1KB of data.

### C. The UI (`nexus-ui` / GPUI)
*   **Event Loop:** In the main app setup, spawn the Kernel in a background thread. Subscribe to the `ShellEvent` channel.
*   **Model:** Create a `Store` that holds a `Vec<Block>`. Update this store exclusively via incoming events.
*   **Rendering:**
    *   `BlockList`: A virtualized list view rendering `Block` components.
    *   `Block`: Composed of `Header` (command text), `Lens` (output view), and `Footer` (status).
    *   `TerminalLens`: Use `alacritty_terminal`'s state machine (headless) to parse ANSI codes from the ring buffer into a grid for rendering.

### D. Provider System (`nexus-api` + `nexus-providers`)
*   **Protocol:** Define a JSON-RPC schema over stdin/stdout for independent provider processes.
*   **Discovery:** A simple scanner that looks for executables in `~/.config/nexus/providers`.
*   **Integration:** The Kernel queries the `ProviderRegistry` during AST analysis to fetch completions or sidecar commands.

## 3. Implementation Phases (The "Tracer Bullet" Path)

**Phase 1: The "Hello World" Pipeline (Weeks 1-2)**
*   **Goal:** Type `ls -la`, see output in a GUI window.
*   **Tasks:**
    1.  Scaffold the Rust workspace.
    2.  Implement a dummy PTY spawner in Kernel.
    3.  Implement a basic Pump that copies PTY output to a `Vec<u8>`.
    4.  Render a raw text view in GPUI that reads that vector.
    *   *Constraint:* No parsing, no syntax highlighting, just bytes -> screen.

**Phase 2: The Shell Interpreter (Weeks 3-6)**
*   **Goal:** `echo "hello" | grep "h"` works.
*   **Tasks:**
    1.  Integrate Tree-sitter.
    2.  Implement the AST walker for pipelines and simple commands.
    3.  Wire up the actual Pump threads to manage the pipe between processes.
    4.  Implement `cd` and basic environment variable handling.

**Phase 3: The Lens & Block System (Weeks 7-10)**
*   **Goal:** `echo '{"a":1}'` renders as a JSON tree.
*   **Tasks:**
    1.  Implement the Sniffer in the Pump.
    2.  Create the `JsonLens` component in UI.
    3.  Implement the Block Reference syntax (`%1`) in the Kernel parser.
    4.  Persist blocks to disk (sqlite/files) for scrolling back.

**Phase 4: Interactivity (Weeks 11-14)**
*   **Goal:** Job control and TUI apps (vim) work.
*   **Tasks:**
    1.  Implement Process Group management and signal handling.
    2.  Detect "Alternate Screen" escape codes to toggle "Fullscreen Mode" in UI.
    3.  Forward keyboard events from GPUI to the PTY.

## 4. Immediate "Do Not" List
*   **Do not** attempt to parse `.bashrc`. Start with a clean TOML config.
*   **Do not** implement SSH yet. Focus on local PTYs.
*   **Do not** build the plugin system dynamically. Hardcode the Git provider initially to prove the sidecar model works.
