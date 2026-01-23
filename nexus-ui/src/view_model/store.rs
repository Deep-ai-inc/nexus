//! The central store for UI state.

use std::collections::HashMap;
use std::path::PathBuf;

use crossbeam_channel::Receiver;
use nexus_api::{BlockId, BlockState, ShellEvent};

use super::BlockViewModel;

/// The central store holding all UI state.
pub struct Store {
    /// All blocks, indexed by ID.
    blocks: HashMap<BlockId, BlockViewModel>,

    /// Block IDs in display order.
    block_order: Vec<BlockId>,

    /// Current working directory.
    cwd: PathBuf,

    /// Event receiver.
    events: Receiver<ShellEvent>,
}

impl Store {
    /// Create a new store with the given event receiver.
    pub fn new(events: Receiver<ShellEvent>, initial_cwd: PathBuf) -> Self {
        Self {
            blocks: HashMap::new(),
            block_order: Vec::new(),
            cwd: initial_cwd,
            events,
        }
    }

    /// Process all pending events.
    pub fn process_events(&mut self) {
        while let Ok(event) = self.events.try_recv() {
            self.handle_event(event);
        }
    }

    /// Handle a single event.
    fn handle_event(&mut self, event: ShellEvent) {
        match event {
            ShellEvent::CommandStarted {
                block_id,
                command,
                cwd,
            } => {
                let block = BlockViewModel::new(block_id, command, cwd);
                self.blocks.insert(block_id, block);
                self.block_order.push(block_id);
            }

            ShellEvent::StdoutChunk { block_id, data } => {
                if let Some(block) = self.blocks.get_mut(&block_id) {
                    block.append_stdout(&data);
                }
            }

            ShellEvent::StderrChunk { block_id, data } => {
                if let Some(block) = self.blocks.get_mut(&block_id) {
                    block.append_stderr(&data);
                }
            }

            ShellEvent::CommandFinished {
                block_id,
                exit_code,
                duration_ms,
            } => {
                if let Some(block) = self.blocks.get_mut(&block_id) {
                    block.finish(exit_code, duration_ms);
                }
            }

            ShellEvent::CwdChanged { new, .. } => {
                self.cwd = new;
            }

            _ => {}
        }
    }

    /// Get a block by ID.
    pub fn get_block(&self, id: BlockId) -> Option<&BlockViewModel> {
        self.blocks.get(&id)
    }

    /// Iterate over all blocks in display order.
    pub fn blocks(&self) -> impl Iterator<Item = &BlockViewModel> {
        self.block_order
            .iter()
            .filter_map(|id| self.blocks.get(id))
    }

    /// Get the current working directory.
    pub fn cwd(&self) -> &PathBuf {
        &self.cwd
    }

    /// Get the number of blocks.
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }
}
