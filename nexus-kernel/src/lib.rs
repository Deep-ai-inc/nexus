//! Nexus Kernel - The shell interpreter core.
//!
//! This crate contains the shell interpreter, including:
//! - Parser (Tree-sitter integration)
//! - Evaluator (AST walker)
//! - State management
//! - In-process commands (ls, cat, etc.)
//! - Persistence (SQLite-backed sessions and blocks)
//! - Native shell history integration
//! - Tab completion

pub mod commands;
pub mod completion;
pub mod eval;
pub mod parser;
pub mod persistence;
pub mod process;
pub mod shell_history;

mod error;
mod state;

pub use commands::CommandRegistry;
pub use completion::{Completion, CompletionEngine, CompletionKind};
pub use error::ShellError;
pub use eval::is_builtin;
pub use parser::Parser;
pub use persistence::Store;
pub use shell_history::{ShellHistory, ShellHistoryEntry};
pub use state::{ShellState, TrapAction};

/// Check if a word is a shell keyword that tree-sitter parses as a statement
/// (flow-control and pipeline modifiers handled by the kernel's parser/evaluator).
fn is_shell_keyword(name: &str) -> bool {
    matches!(
        name,
        "if" | "while" | "until" | "for" | "case" | "function" | "watch"
    )
}

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
    /// SQLite-backed persistence for sessions and blocks.
    store: Option<Store>,
    /// Current session ID.
    session_id: Option<i64>,
    /// Native shell history (reads/writes ~/.zsh_history or ~/.bash_history).
    shell_history: Option<ShellHistory>,
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

        // Try to open native shell history (non-fatal if it fails)
        let shell_history = ShellHistory::open();
        if shell_history.is_none() {
            tracing::warn!("Could not detect shell history file; history will be in-memory only");
        }

        let kernel = Self {
            state: ShellState::new()?,
            event_tx,
            parser: parser::Parser::new()?,
            commands: CommandRegistry::new(),
            store,
            session_id,
            shell_history,
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
        let is_keyword = is_shell_keyword(first_word);

        if has_pipe || is_native || is_shell_builtin || is_keyword {
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

        let ast = self.parser.parse(&processed_input)?;
        let exit_code = eval::execute_with_block_id(
            &mut self.state,
            &ast,
            &self.event_tx,
            &self.commands,
            block_id,
        )?;

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

    /// Search command history using substring matching on native shell history.
    ///
    /// Returns matching history entries, most recent first.
    pub fn search_history(&self, query: &str, limit: usize) -> Vec<ShellHistoryEntry> {
        self.shell_history
            .as_ref()
            .map(|h| h.search(query, limit))
            .unwrap_or_default()
    }

    /// Get recent command history from native shell history.
    ///
    /// Returns the most recent commands, newest last (chronological order).
    pub fn get_recent_history(&self, limit: usize) -> Vec<ShellHistoryEntry> {
        self.shell_history
            .as_ref()
            .map(|h| h.recent(limit))
            .unwrap_or_default()
    }

    /// Append a command to native shell history.
    ///
    /// Called from the UI on submit (before execution) so both kernel and PTY
    /// commands are recorded, and commands survive crashes.
    pub fn append_history(&mut self, command: &str) {
        if let Some(h) = &mut self.shell_history {
            h.append(command);
        }
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
