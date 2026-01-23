//! Nexus Kernel - The shell interpreter core.
//!
//! This crate contains the shell interpreter, including:
//! - Parser (Tree-sitter integration)
//! - Evaluator (AST walker)
//! - Process management (PTY, job control)

pub mod parser;
pub mod eval;
pub mod process;

mod state;
mod error;

pub use state::ShellState;
pub use error::ShellError;

use crossbeam_channel::Sender;
use nexus_api::ShellEvent;

/// The shell kernel - owns interpreter state and executes commands.
pub struct Kernel {
    state: ShellState,
    event_tx: Sender<ShellEvent>,
    parser: parser::Parser,
}

impl Kernel {
    /// Create a new kernel with the given event sender.
    pub fn new(event_tx: Sender<ShellEvent>) -> anyhow::Result<Self> {
        Ok(Self {
            state: ShellState::new()?,
            event_tx,
            parser: parser::Parser::new()?,
        })
    }

    /// Execute a command line and return the exit code.
    pub fn execute(&mut self, input: &str) -> anyhow::Result<i32> {
        let ast = self.parser.parse(input)?;
        eval::execute(&mut self.state, &ast, &self.event_tx)
    }

    /// Get a reference to the current shell state.
    pub fn state(&self) -> &ShellState {
        &self.state
    }
}
