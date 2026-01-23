//! Nexus Kernel - The shell interpreter core.
//!
//! This crate contains the shell interpreter, including:
//! - Parser (Tree-sitter integration)
//! - Evaluator (AST walker)
//! - State management

pub mod parser;
pub mod eval;
pub mod process;

mod state;
mod error;

pub use state::ShellState;
pub use error::ShellError;
pub use parser::Parser;

use tokio::sync::broadcast;
use nexus_api::ShellEvent;

/// The shell kernel - owns interpreter state and executes commands.
pub struct Kernel {
    state: ShellState,
    event_tx: broadcast::Sender<ShellEvent>,
    parser: parser::Parser,
}

impl Kernel {
    /// Create a new kernel with an event broadcast channel.
    pub fn new() -> anyhow::Result<(Self, broadcast::Receiver<ShellEvent>)> {
        let (event_tx, event_rx) = broadcast::channel(1024);
        let kernel = Self {
            state: ShellState::new()?,
            event_tx,
            parser: parser::Parser::new()?,
        };
        Ok((kernel, event_rx))
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
}
