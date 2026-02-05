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
const QUESTION_OPTION: u64 = 16;
const QUESTION_SUBMIT: u64 = 17;
const AGENT_TOOL: u64 = 18;
const AGENT_PERM_TEXT: u64 = 19;
const AGENT_QUESTION_TEXT: u64 = 20;
const AGENT_FOOTER: u64 = 21;
const VIEWER_EXIT: u64 = 22;
const TREE_EXPAND: u64 = 23;
const BLOCK_CONTAINER: u64 = 24;

// --- Shell block IDs ---

/// The outermost container Column for a shell block — used as a click target
/// so that clicks on empty space (e.g. between rows) still register as
/// belonging to this block for focus purposes.
pub fn block_container(id: BlockId) -> SourceId { block_space(id).id(BLOCK_CONTAINER) }
pub fn shell_header(id: BlockId) -> SourceId { block_space(id).id(SHELL_HEADER) }
pub fn shell_term(id: BlockId) -> SourceId { block_space(id).id(SHELL_TERM) }
pub fn native(id: BlockId) -> SourceId { block_space(id).id(NATIVE) }
pub fn table(id: BlockId) -> SourceId { block_space(id).id(TABLE) }
pub fn kill(id: BlockId) -> SourceId { block_space(id).id(KILL) }
pub fn image_output(id: BlockId) -> SourceId { block_space(id).id(IMAGE_OUTPUT) }
pub fn viewer_exit(id: BlockId) -> SourceId { block_space(id).id(VIEWER_EXIT) }

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

/// Tree expand chevron for a directory entry.
pub fn tree_expand(id: BlockId, index: usize) -> SourceId {
    block_space(id).child(TREE_EXPAND).id(index as u64)
}

pub fn agent_tool_toggle(id: BlockId, i: usize) -> SourceId {
    block_space(id).child(THINKING_TOGGLE).id(i as u64)
}

/// Source ID for a question option button: block → QUESTION_OPTION → question_idx → option_idx.
pub fn agent_question_option(id: BlockId, q: usize, opt: usize) -> SourceId {
    block_space(id).child(QUESTION_OPTION).child(q as u64).id(opt as u64)
}

/// Source ID for the free-form question submit button.
pub fn agent_question_submit(id: BlockId) -> SourceId {
    block_space(id).id(QUESTION_SUBMIT)
}

pub fn agent_tool(id: BlockId, i: usize) -> SourceId {
    block_space(id).child(AGENT_TOOL).id(i as u64)
}

pub fn agent_perm_text(id: BlockId) -> SourceId { block_space(id).id(AGENT_PERM_TEXT) }
pub fn agent_question_text(id: BlockId) -> SourceId { block_space(id).id(AGENT_QUESTION_TEXT) }
pub fn agent_footer(id: BlockId) -> SourceId { block_space(id).id(AGENT_FOOTER) }

// --- Global UI IDs (no block) ---

const GLOBAL: IdSpace = IdSpace::new(0xFFFF_FFFF_FFFF_FFFF);

pub fn mode_toggle() -> SourceId { GLOBAL.id(1) }
pub fn remove_attachment(i: usize) -> SourceId { GLOBAL.child(2).id(i as u64) }
pub fn ctx_menu_item(i: usize) -> SourceId { GLOBAL.child(3).id(i as u64) }

#[cfg(test)]
mod tests {
    use super::*;

    // Shell block ID tests

    #[test]
    fn test_block_container() {
        let id = block_container(BlockId(1));
        let id2 = block_container(BlockId(1));
        assert_eq!(id, id2);
        // Different blocks should give different IDs
        let id3 = block_container(BlockId(2));
        assert_ne!(id, id3);
    }

    #[test]
    fn test_shell_header() {
        let id = shell_header(BlockId(5));
        let id2 = shell_header(BlockId(5));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_shell_term() {
        let id = shell_term(BlockId(10));
        let id2 = shell_term(BlockId(10));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_native() {
        let id = native(BlockId(3));
        let id2 = native(BlockId(3));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_table() {
        let id = table(BlockId(7));
        let id2 = table(BlockId(7));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_kill() {
        let id = kill(BlockId(4));
        let id2 = kill(BlockId(4));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_image_output() {
        let id = image_output(BlockId(8));
        let id2 = image_output(BlockId(8));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_viewer_exit() {
        let id = viewer_exit(BlockId(9));
        let id2 = viewer_exit(BlockId(9));
        assert_eq!(id, id2);
    }

    // Agent block ID tests

    #[test]
    fn test_agent_query() {
        let id = agent_query(BlockId(100));
        let id2 = agent_query(BlockId(100));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_thinking() {
        let id = agent_thinking(BlockId(101));
        let id2 = agent_thinking(BlockId(101));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_response() {
        let id = agent_response(BlockId(102));
        let id2 = agent_response(BlockId(102));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_thinking_toggle() {
        let id = agent_thinking_toggle(BlockId(103));
        let id2 = agent_thinking_toggle(BlockId(103));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_stop() {
        let id = agent_stop(BlockId(104));
        let id2 = agent_stop(BlockId(104));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_perm_deny() {
        let id = agent_perm_deny(BlockId(105));
        let id2 = agent_perm_deny(BlockId(105));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_perm_allow() {
        let id = agent_perm_allow(BlockId(106));
        let id2 = agent_perm_allow(BlockId(106));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_perm_always() {
        let id = agent_perm_always(BlockId(107));
        let id2 = agent_perm_always(BlockId(107));
        assert_eq!(id, id2);
    }

    // Indexed ID tests

    #[test]
    fn test_table_sort() {
        let id = table_sort(BlockId(20), 0);
        let id2 = table_sort(BlockId(20), 0);
        assert_eq!(id, id2);
        // Different columns should give different IDs
        let id3 = table_sort(BlockId(20), 1);
        assert_ne!(id, id3);
    }

    #[test]
    fn test_anchor() {
        let id = anchor(BlockId(21), 5);
        let id2 = anchor(BlockId(21), 5);
        assert_eq!(id, id2);
        // Different indices should give different IDs
        let id3 = anchor(BlockId(21), 6);
        assert_ne!(id, id3);
    }

    #[test]
    fn test_tree_expand() {
        let id = tree_expand(BlockId(22), 10);
        let id2 = tree_expand(BlockId(22), 10);
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_tool_toggle() {
        let id = agent_tool_toggle(BlockId(23), 3);
        let id2 = agent_tool_toggle(BlockId(23), 3);
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_question_option() {
        let id = agent_question_option(BlockId(24), 1, 2);
        let id2 = agent_question_option(BlockId(24), 1, 2);
        assert_eq!(id, id2);
        // Different question or option should give different IDs
        let id3 = agent_question_option(BlockId(24), 1, 3);
        assert_ne!(id, id3);
        let id4 = agent_question_option(BlockId(24), 2, 2);
        assert_ne!(id, id4);
    }

    #[test]
    fn test_agent_question_submit() {
        let id = agent_question_submit(BlockId(25));
        let id2 = agent_question_submit(BlockId(25));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_tool() {
        let id = agent_tool(BlockId(26), 7);
        let id2 = agent_tool(BlockId(26), 7);
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_perm_text() {
        let id = agent_perm_text(BlockId(27));
        let id2 = agent_perm_text(BlockId(27));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_question_text() {
        let id = agent_question_text(BlockId(28));
        let id2 = agent_question_text(BlockId(28));
        assert_eq!(id, id2);
    }

    #[test]
    fn test_agent_footer() {
        let id = agent_footer(BlockId(29));
        let id2 = agent_footer(BlockId(29));
        assert_eq!(id, id2);
    }

    // Global UI ID tests

    #[test]
    fn test_mode_toggle() {
        let id = mode_toggle();
        let id2 = mode_toggle();
        assert_eq!(id, id2);
    }

    #[test]
    fn test_remove_attachment() {
        let id = remove_attachment(0);
        let id2 = remove_attachment(0);
        assert_eq!(id, id2);
        // Different indices should give different IDs
        let id3 = remove_attachment(1);
        assert_ne!(id, id3);
    }

    #[test]
    fn test_ctx_menu_item() {
        let id = ctx_menu_item(0);
        let id2 = ctx_menu_item(0);
        assert_eq!(id, id2);
        // Different indices should give different IDs
        let id3 = ctx_menu_item(1);
        assert_ne!(id, id3);
    }

    // Tests to verify different widget types give different IDs
    #[test]
    fn test_different_widget_types_give_different_ids() {
        let block = BlockId(1);
        let header = shell_header(block);
        let term = shell_term(block);
        let nat = native(block);
        let tab = table(block);
        let kil = kill(block);

        // All should be different from each other
        assert_ne!(header, term);
        assert_ne!(header, nat);
        assert_ne!(header, tab);
        assert_ne!(header, kil);
        assert_ne!(term, nat);
        assert_ne!(term, tab);
        assert_ne!(nat, tab);
    }
}
