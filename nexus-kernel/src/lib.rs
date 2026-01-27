//! Nexus Kernel - The shell interpreter core.
//!
//! This crate contains the shell interpreter, including:
//! - Parser (Tree-sitter integration)
//! - Evaluator (AST walker)
//! - State management
//! - In-process commands (ls, cat, etc.)
//! - Persistence (SQLite-backed history and sessions)
//! - Tab completion

pub mod commands;
pub mod completion;
pub mod eval;
pub mod parser;
pub mod persistence;
pub mod process;

mod error;
mod state;

pub use commands::CommandRegistry;
pub use completion::{Completion, CompletionEngine, CompletionKind};
pub use error::ShellError;
pub use eval::is_builtin;
pub use parser::Parser;
pub use persistence::{HistoryEntry, Store};
pub use state::{ShellState, TrapAction};

/// Classification of how a command should be executed.
///
/// The UI uses this to decide whether to route through the kernel
/// (for pipelines, native commands, builtins) or spawn a PTY directly
/// (for single external commands that need interactive terminal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandClassification {
    /// Execute through kernel - pipelines, native commands, or shell builtins.
    /// The kernel handles these directly with structured Value output.
    Kernel,
    /// Execute via PTY - single external commands that need interactive terminal.
    /// These are forked processes with raw terminal I/O.
    Pty,
}

use nexus_api::ShellEvent;
use tokio::sync::broadcast;

/// The shell kernel - owns interpreter state and executes commands.
pub struct Kernel {
    state: ShellState,
    event_tx: broadcast::Sender<ShellEvent>,
    parser: parser::Parser,
    commands: CommandRegistry,
    /// SQLite-backed persistence for history and sessions.
    store: Option<Store>,
    /// Current session ID.
    session_id: Option<i64>,
}

impl Kernel {
    /// Create a new kernel with an event broadcast channel.
    pub fn new() -> anyhow::Result<(Self, broadcast::Receiver<ShellEvent>)> {
        let (event_tx, event_rx) = broadcast::channel(1024);

        // Try to open persistence store (non-fatal if it fails)
        let (store, session_id) = match Store::open_default() {
            Ok(store) => {
                let cwd = std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "/".to_string());
                let session_id = store.start_session(&cwd).ok();
                (Some(store), session_id)
            }
            Err(e) => {
                tracing::warn!("Failed to open persistence store: {}", e);
                (None, None)
            }
        };

        let kernel = Self {
            state: ShellState::new()?,
            event_tx,
            parser: parser::Parser::new()?,
            commands: CommandRegistry::new(),
            store,
            session_id,
        };
        Ok((kernel, event_rx))
    }

    /// Get a reference to the command registry.
    pub fn commands(&self) -> &CommandRegistry {
        &self.commands
    }

    /// Classify how a command should be executed.
    ///
    /// Returns `CommandClassification::Kernel` for:
    /// - Pipelines (contain `|`)
    /// - Native/in-process commands (ls, cat, grep, etc.)
    /// - Shell builtins (cd, export, etc.)
    ///
    /// Returns `CommandClassification::Pty` for:
    /// - Single external commands (git, vim, etc.)
    ///
    /// This method centralizes the decision logic so both UI and tests
    /// use the same classification.
    pub fn classify_command(&self, command: &str) -> CommandClassification {
        let has_pipe = command.contains('|');
        let first_word = command.split_whitespace().next().unwrap_or("");
        let is_native = self.commands.contains(first_word);
        let is_shell_builtin = is_builtin(first_word);

        if has_pipe || is_native || is_shell_builtin {
            CommandClassification::Kernel
        } else {
            CommandClassification::Pty
        }
    }

    /// Get a reference to the current shell state.
    pub fn state(&self) -> &ShellState {
        &self.state
    }

    /// Get a mutable reference to the shell state.
    pub fn state_mut(&mut self) -> &mut ShellState {
        &mut self.state
    }

    /// Parse a command line into an AST.
    pub fn parse(&mut self, input: &str) -> Result<parser::Ast, ShellError> {
        self.parser.parse(input)
    }

    /// Subscribe to shell events.
    pub fn subscribe(&self) -> broadcast::Receiver<ShellEvent> {
        self.event_tx.subscribe()
    }

    /// Emit a shell event.
    pub fn emit(&self, event: ShellEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Parse and execute a command line, returning the exit code.
    ///
    /// Special syntax:
    /// - Lines starting with `|` are pipeline continuations from previous output.
    ///   `| grep foo` becomes `_ | grep foo` internally.
    pub fn execute(&mut self, input: &str) -> anyhow::Result<i32> {
        self.execute_with_block_id(input, None)
    }

    /// Parse and execute a command line with a specific block ID.
    ///
    /// If block_id is provided, all events will use that ID (for UI integration).
    /// If None, the kernel will generate its own ID.
    pub fn execute_with_block_id(
        &mut self,
        input: &str,
        block_id: Option<nexus_api::BlockId>,
    ) -> anyhow::Result<i32> {
        // Handle pipeline continuation: `| cmd` becomes `_ | cmd`
        let processed_input = preprocess_input(input);
        let start = std::time::Instant::now();

        let ast = self.parser.parse(&processed_input)?;
        let exit_code = eval::execute_with_block_id(
            &mut self.state,
            &ast,
            &self.event_tx,
            &self.commands,
            block_id,
        )?;

        // Save to history (non-blocking, ignore errors)
        let duration_ms = start.elapsed().as_millis() as u64;
        if let Some(store) = &self.store {
            let cwd = self.state.cwd.display().to_string();
            let _ = store.add_history(
                input.trim(),
                &cwd,
                Some(exit_code),
                Some(duration_ms),
                self.session_id,
            );
        }

        Ok(exit_code)
    }

    /// Get a reference to the persistence store.
    pub fn store(&self) -> Option<&Store> {
        self.store.as_ref()
    }

    /// Get the current session ID.
    pub fn session_id(&self) -> Option<i64> {
        self.session_id
    }

    /// Get the event sender (for spawning commands that need to emit events).
    pub fn event_sender(&self) -> &broadcast::Sender<ShellEvent> {
        &self.event_tx
    }

    /// Check if there's a previous output available for pipeline continuation.
    pub fn has_previous_output(&self) -> bool {
        self.state.last_output.is_some()
    }

    /// Get completions for the given input at the cursor position.
    ///
    /// Returns (completions, start_offset) where start_offset is the position
    /// where the completed word starts.
    pub fn complete(&self, input: &str, cursor: usize) -> (Vec<Completion>, usize) {
        let engine = CompletionEngine::new(&self.state, &self.commands);
        engine.complete(input, cursor)
    }

    /// Search command history using full-text search.
    ///
    /// Returns matching history entries, most recent first.
    pub fn search_history(&self, query: &str, limit: usize) -> Vec<persistence::HistoryEntry> {
        self.store
            .as_ref()
            .and_then(|store| store.search_history(query, limit).ok())
            .unwrap_or_default()
    }

    /// Get recent command history.
    ///
    /// Returns the most recent commands, newest first.
    pub fn get_recent_history(&self, limit: usize) -> Vec<persistence::HistoryEntry> {
        self.store
            .as_ref()
            .and_then(|store| store.get_recent_history(limit).ok())
            .unwrap_or_default()
    }
}

/// Preprocess input to handle special syntax.
///
/// - Lines starting with `|` become `_ | ...` (pipeline continuation)
fn preprocess_input(input: &str) -> String {
    let trimmed = input.trim_start();

    // Pipeline continuation: `| cmd` -> `_ | cmd`
    if trimmed.starts_with('|') {
        format!("_ {}", trimmed)
    } else {
        input.to_string()
    }
}
