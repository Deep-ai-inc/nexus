//! Source ID helpers — single source of truth for all widget SourceIds.
//!
//! Uses `IdSpace` for zero-allocation, splitmix64-mixed IDs.
//! No `format!()` or `String` churn — all computations are pure arithmetic.

use nexus_api::BlockId;
use strata::component::IdSpace;
use strata::content_address::SourceId;

/// Derive a per-block ID namespace from a block's numeric ID.
const fn block_space(id: BlockId) -> IdSpace {
    IdSpace::new(id.0)
}

// Tags for different content types within a block.
const SHELL_HEADER: u64 = 1;
const SHELL_TERM: u64 = 2;
const NATIVE: u64 = 3;
const TABLE: u64 = 4;
const KILL: u64 = 5;
const AGENT_QUERY: u64 = 6;
const AGENT_THINKING: u64 = 7;
const AGENT_RESPONSE: u64 = 8;
const THINKING_TOGGLE: u64 = 9;
const STOP: u64 = 10;
const PERM_DENY: u64 = 11;
const PERM_ALLOW: u64 = 12;
const PERM_ALWAYS: u64 = 13;
const ANCHOR: u64 = 14;
const IMAGE_OUTPUT: u64 = 15;

// --- Shell block IDs ---

pub fn shell_header(id: BlockId) -> SourceId { block_space(id).id(SHELL_HEADER) }
pub fn shell_term(id: BlockId) -> SourceId { block_space(id).id(SHELL_TERM) }
pub fn native(id: BlockId) -> SourceId { block_space(id).id(NATIVE) }
pub fn table(id: BlockId) -> SourceId { block_space(id).id(TABLE) }
pub fn kill(id: BlockId) -> SourceId { block_space(id).id(KILL) }
pub fn image_output(id: BlockId) -> SourceId { block_space(id).id(IMAGE_OUTPUT) }

// --- Agent block IDs ---

pub fn agent_query(id: BlockId) -> SourceId { block_space(id).id(AGENT_QUERY) }
pub fn agent_thinking(id: BlockId) -> SourceId { block_space(id).id(AGENT_THINKING) }
pub fn agent_response(id: BlockId) -> SourceId { block_space(id).id(AGENT_RESPONSE) }
pub fn agent_thinking_toggle(id: BlockId) -> SourceId { block_space(id).id(THINKING_TOGGLE) }
pub fn agent_stop(id: BlockId) -> SourceId { block_space(id).id(STOP) }
pub fn agent_perm_deny(id: BlockId) -> SourceId { block_space(id).id(PERM_DENY) }
pub fn agent_perm_allow(id: BlockId) -> SourceId { block_space(id).id(PERM_ALLOW) }
pub fn agent_perm_always(id: BlockId) -> SourceId { block_space(id).id(PERM_ALWAYS) }

// --- Indexed IDs (block + index dimension) ---

pub fn table_sort(id: BlockId, col: usize) -> SourceId {
    block_space(id).child(TABLE).id(col as u64)
}

pub fn anchor(id: BlockId, index: usize) -> SourceId {
    block_space(id).child(ANCHOR).id(index as u64)
}

pub fn agent_tool_toggle(id: BlockId, i: usize) -> SourceId {
    block_space(id).child(THINKING_TOGGLE).id(i as u64)
}

// --- Global UI IDs (no block) ---

const GLOBAL: IdSpace = IdSpace::new(0xFFFF_FFFF_FFFF_FFFF);

pub fn mode_toggle() -> SourceId { GLOBAL.id(1) }
pub fn remove_attachment(i: usize) -> SourceId { GLOBAL.child(2).id(i as u64) }
pub fn ctx_menu_item(i: usize) -> SourceId { GLOBAL.child(3).id(i as u64) }
