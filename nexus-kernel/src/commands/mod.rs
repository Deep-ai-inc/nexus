//! In-process command system for Nexus.
//!
//! Commands implemented here run within the shell process (no fork/exec),
//! return structured `Value` data, and can leverage full GUI capabilities.

mod base64_cmd;
mod basic;
mod cat;
mod chmod;
mod clip;
mod date;
mod df;
mod diff;
mod du;
mod env;
mod find;
mod fs;
mod grep;
mod hash;
mod head;
mod help;
mod history;
mod iterators;
mod jobs;
mod json;
mod less;
mod link;
mod ls;
mod man;
mod math;
mod open;
mod path;
mod prev;
mod printf;
pub(crate) mod ps;
mod registry;
mod select;
mod seq;
mod shuf;
mod signal;
mod sort;
mod split;
mod system;
mod tail;
mod top;
mod tree;
mod unicode_stress;
mod tee;
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

// ---- Cancellation registry for long-running commands ----

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

static CANCEL_REGISTRY: std::sync::LazyLock<Mutex<HashMap<BlockId, Arc<AtomicBool>>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Register a cancellation flag for a block. Returns the flag for the command to poll.
pub fn register_cancel(block_id: BlockId) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    CANCEL_REGISTRY.lock().unwrap().insert(block_id, flag.clone());
    flag
}

/// Signal a block to cancel. Returns true if the block was found.
pub fn cancel_block(block_id: BlockId) -> bool {
    if let Some(flag) = CANCEL_REGISTRY.lock().unwrap().get(&block_id) {
        flag.store(true, Ordering::Relaxed);
        true
    } else {
        false
    }
}

/// Remove a block from the cancel registry (called when command exits).
pub fn unregister_cancel(block_id: BlockId) {
    CANCEL_REGISTRY.lock().unwrap().remove(&block_id);
}

/// Trait for commands that run in-process and return structured data.
pub trait NexusCommand: Send + Sync {
    /// The command name (e.g., "ls", "cat", "grep")
    fn name(&self) -> &'static str;

    /// One-line description for `help` output. Empty string = undocumented.
    fn description(&self) -> &'static str {
        ""
    }

    /// Execute the command with the given arguments.
    fn execute(&self, args: &[String], ctx: &mut CommandContext) -> anyhow::Result<Value>;
}

#[cfg(test)]
mod cancel_tests {
    use super::*;

    #[test]
    fn test_register_cancel_creates_flag() {
        let block_id = BlockId(10001);
        let flag = register_cancel(block_id);
        assert!(!flag.load(Ordering::Relaxed));
        // Cleanup
        unregister_cancel(block_id);
    }

    #[test]
    fn test_cancel_block_sets_flag() {
        let block_id = BlockId(10002);
        let flag = register_cancel(block_id);

        assert!(!flag.load(Ordering::Relaxed));
        let found = cancel_block(block_id);
        assert!(found);
        assert!(flag.load(Ordering::Relaxed));

        // Cleanup
        unregister_cancel(block_id);
    }

    #[test]
    fn test_cancel_block_returns_false_for_unregistered() {
        let block_id = BlockId(99999);
        let found = cancel_block(block_id);
        assert!(!found);
    }

    #[test]
    fn test_unregister_cancel_removes_block() {
        let block_id = BlockId(10003);
        let _flag = register_cancel(block_id);

        // Block should be found
        assert!(cancel_block(block_id));

        // Unregister
        unregister_cancel(block_id);

        // Block should no longer be found
        assert!(!cancel_block(block_id));
    }

    #[test]
    fn test_multiple_blocks_independent() {
        let block_a = BlockId(10004);
        let block_b = BlockId(10005);

        let flag_a = register_cancel(block_a);
        let flag_b = register_cancel(block_b);

        // Cancel only block A
        cancel_block(block_a);

        assert!(flag_a.load(Ordering::Relaxed));
        assert!(!flag_b.load(Ordering::Relaxed));

        // Cleanup
        unregister_cancel(block_a);
        unregister_cancel(block_b);
    }
}
