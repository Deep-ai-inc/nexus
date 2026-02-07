//! Block storage — owns the block list, block-ID-to-index map, and image handles.

use std::collections::HashMap;

use nexus_api::BlockId;
use strata::ImageHandle;

use crate::blocks::Block;

/// Manages the block list, block-ID index, and decoded image handles.
///
/// All mutations to the block list should go through `BlockManager` methods
/// so the index stays in sync.
pub(crate) struct BlockManager {
    /// The ordered list of blocks.
    pub blocks: Vec<Block>,
    /// Fast BlockId → index lookup. Private to force callers through `get`/`get_mut`.
    block_index: HashMap<BlockId, usize>,
    /// Decoded image handles keyed by block ID: (handle, width, height).
    pub image_handles: HashMap<BlockId, (ImageHandle, u32, u32)>,
}

impl BlockManager {
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            block_index: HashMap::new(),
            image_handles: HashMap::new(),
        }
    }

    /// Look up a block by ID (immutable).
    pub fn get(&self, id: BlockId) -> Option<&Block> {
        self.block_index.get(&id).and_then(|&idx| self.blocks.get(idx))
    }

    /// Look up a block by ID (mutable).
    pub fn get_mut(&mut self, id: BlockId) -> Option<&mut Block> {
        if let Some(&idx) = self.block_index.get(&id) {
            self.blocks.get_mut(idx)
        } else {
            None
        }
    }

    /// Whether a block with the given ID exists.
    pub fn contains(&self, id: BlockId) -> bool {
        self.block_index.contains_key(&id)
    }

    /// Append a block, updating the index.
    pub fn push(&mut self, block: Block) {
        let idx = self.blocks.len();
        self.block_index.insert(block.id, idx);
        self.blocks.push(block);
    }

    /// Store a decoded image handle for a block.
    pub fn store_image(&mut self, id: BlockId, handle: ImageHandle, w: u32, h: u32) {
        self.image_handles.insert(id, (handle, w, h));
    }

    /// Get the image handle info for a block (handle, width, height).
    pub fn image_info(&self, id: BlockId) -> Option<(ImageHandle, u32, u32)> {
        self.image_handles.get(&id).copied()
    }

    /// The last block, if any.
    pub fn last(&self) -> Option<&Block> {
        self.blocks.last()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Clear all blocks, the index, and image handles.
    pub fn clear(&mut self) {
        self.blocks.clear();
        self.block_index.clear();
        self.image_handles.clear();
    }
}
