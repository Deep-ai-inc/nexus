//! In-process command system for Nexus.
//!
//! Commands implemented here run within the shell process (no fork/exec),
//! return structured `Value` data, and can leverage full GUI capabilities.

mod basic;
mod cat;
mod claude;
mod cmp;
mod cut;
mod date;
mod env;
mod find;
mod fs;
mod grep;
mod hash;
mod head;
mod history;
mod jobs;
mod json;
mod links;
mod ls;
mod math;
mod nl;
mod path;
mod perms;
mod printf;
mod registry;
mod rev;
mod select;
mod seq;
mod shuf;
mod signal;
mod sort;
mod split;
mod system;
mod tail;
mod times;
mod ulimit;
mod tee;
mod tr;
mod uniq;
mod wc;
mod which;

#[cfg(test)]
mod test_utils;

pub use registry::CommandRegistry;

use crate::ShellState;
use nexus_api::{BlockId, ShellEvent, Value};
use tokio::sync::broadcast::Sender;

/// Context passed to commands during execution.
pub struct CommandContext<'a> {
    /// The current shell state (env, cwd, etc.)
    pub state: &'a mut ShellState,
    /// Event channel for streaming output
    pub events: &'a Sender<ShellEvent>,
    /// Block ID for this command invocation
    pub block_id: BlockId,
    /// Piped input from previous command (if any)
    pub stdin: Option<Value>,
}

/// Trait for commands that run in-process and return structured data.
pub trait NexusCommand: Send + Sync {
    /// The command name (e.g., "ls", "cat", "grep")
    fn name(&self) -> &'static str;

    /// Execute the command with the given arguments.
    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value>;
}
