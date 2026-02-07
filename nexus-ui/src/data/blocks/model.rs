//! Core block types: Block, UnifiedBlock, UnifiedBlockRef.

use std::collections::VecDeque;
use std::sync::atomic::AtomicU16;
use std::time::Instant;

use nexus_api::{BlockId, BlockState, OutputFormat, Value};
use nexus_term::TerminalParser;

use crate::data::agent_block::AgentBlock;
use super::enums::ProcSort;
use super::view::{ViewState, FileTreeState, TableSort};

/// Unified block type - either a shell command or agent conversation.
#[derive(Debug)]
pub enum UnifiedBlock {
    Shell(Block),
    Agent(AgentBlock),
}

impl UnifiedBlock {
    /// Get the block ID for ordering.
    pub fn id(&self) -> BlockId {
        match self {
            UnifiedBlock::Shell(b) => b.id,
            UnifiedBlock::Agent(b) => b.id,
        }
    }

    /// Check if the block is still running/active.
    pub fn is_running(&self) -> bool {
        match self {
            UnifiedBlock::Shell(b) => b.is_running(),
            UnifiedBlock::Agent(b) => b.is_running(),
        }
    }
}

/// Reference to a unified block for view rendering (avoids cloning).
pub enum UnifiedBlockRef<'a> {
    Shell(&'a Block),
    Agent(&'a AgentBlock),
}

/// A command block containing input and output.
#[derive(Debug)]
pub struct Block {
    pub id: BlockId,
    pub command: String,
    pub parser: TerminalParser,
    pub state: BlockState,
    #[allow(dead_code)]
    pub format: OutputFormat,
    pub collapsed: bool,
    pub started_at: Instant,
    pub duration_ms: Option<u64>,
    /// Version counter for lazy invalidation.
    pub version: u64,
    /// Native command output (structured data, not terminal output).
    pub native_output: Option<Value>,
    /// Sort state for table output.
    pub table_sort: TableSort,
    /// Whether output contained "permission denied".
    pub has_permission_denied: bool,
    /// Whether output contained "command not found".
    pub has_command_not_found: bool,
    /// Append-only event log (ping replies, etc.). Capped at 1000 entries.
    pub stream_log: VecDeque<Value>,
    /// Latest coalesced state (progress bar, live table, etc.).
    pub stream_latest: Option<Value>,
    /// Sequence counter for ordering streaming updates.
    pub stream_seq: u64,
    /// Interactive viewer state (pager, process monitor, tree browser).
    pub view_state: Option<ViewState>,
    /// Lazy tree expansion state (only allocated when a user clicks a chevron).
    pub file_tree: Option<FileTreeState>,
    /// OSC title set by the child process (via escape sequences).
    pub osc_title: Option<String>,
    /// High-water mark for content_rows, used to debounce shrink flicker
    /// on running blocks that do clear+reprint cycles (e.g. Claude Code).
    pub peak_content_rows: AtomicU16,
}

impl Block {
    pub fn new(id: BlockId, command: String) -> Self {
        Self {
            id,
            command,
            parser: TerminalParser::new(120, 24),
            state: BlockState::Running,
            format: OutputFormat::PlainText,
            collapsed: false,
            started_at: Instant::now(),
            duration_ms: None,
            version: 0,
            native_output: None,
            table_sort: TableSort::new(),
            has_permission_denied: false,
            has_command_not_found: false,
            stream_log: VecDeque::new(),
            stream_latest: None,
            stream_seq: 0,
            view_state: None,
            file_tree: None,
            osc_title: None,
            peak_content_rows: AtomicU16::new(0),
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state, BlockState::Running)
    }

    /// Get or create file tree expansion state.
    pub fn ensure_file_tree(&mut self) -> &mut FileTreeState {
        self.file_tree.get_or_insert_with(FileTreeState::default)
    }

    /// Get file tree expansion state (if any).
    pub fn file_tree(&self) -> Option<&FileTreeState> {
        self.file_tree.as_ref()
    }

    // =========================================================================
    // Viewer update — handles ViewerMsg by delegating to ViewState methods
    // =========================================================================

    /// Handle a viewer message. Returns true if the block state changed.
    /// This encapsulates all viewer state manipulation that was previously
    /// spread across dispatch_viewer_msg in state_update.rs.
    pub fn update_viewer(&mut self, msg: &crate::app::message::ViewerMsg) -> bool {
        use crate::app::message::ViewerMsg;

        // Handle SortBy specially (needs mutable access to multiple fields)
        if let ViewerMsg::SortBy(_, sort) = msg {
            let changed = self.handle_sort(*sort);
            if changed {
                self.version += 1;
            }
            return changed;
        }

        // Pre-compute bounds before borrowing view_state mutably
        let tree_count = self.tree_node_count();
        let diff_count = self.diff_file_count();

        let Some(view_state) = &mut self.view_state else {
            return false;
        };

        let changed = match msg {
            ViewerMsg::ScrollUp(_) => view_state.scroll_up(),
            ViewerMsg::ScrollDown(_) => view_state.scroll_down(),
            ViewerMsg::PageUp(_) => view_state.page_up(),
            ViewerMsg::PageDown(_) => view_state.page_down(),
            ViewerMsg::GoToTop(_) => view_state.go_to_top(),
            ViewerMsg::GoToBottom(_) => view_state.go_to_bottom(),
            ViewerMsg::SearchStart(_) | ViewerMsg::SearchNext(_) => {
                // Search TBD — no-op for now
                false
            }
            ViewerMsg::SortBy(_, _) => unreachable!(), // handled above
            ViewerMsg::TreeToggle(_) => {
                let selected = view_state.tree_selected();
                view_state.tree_toggle(selected)
            }
            ViewerMsg::TreeUp(_) => view_state.tree_up(),
            ViewerMsg::TreeDown(_) => view_state.tree_down(tree_count),
            ViewerMsg::DiffNextFile(_) => view_state.diff_next_file(diff_count),
            ViewerMsg::DiffPrevFile(_) => view_state.diff_prev_file(),
            ViewerMsg::DiffToggleFile(_) => view_state.diff_toggle_file(),
            ViewerMsg::Exit(_) => {
                // Exit is handled specially by the caller (needs side effects)
                return false;
            }
        };

        if changed {
            self.version += 1;
        }
        changed
    }

    /// Handle process monitor sort. Returns true if the state changed.
    fn handle_sort(&mut self, sort: ProcSort) -> bool {
        if let Some(ViewState::ProcessMonitor { ref mut sort_by, ref mut sort_desc, .. }) =
            self.view_state
        {
            if *sort_by == sort {
                *sort_desc = !*sort_desc;
            } else {
                *sort_by = sort;
                *sort_desc = true;
            }

            // Map ProcSort to column index (%CPU=2, %MEM=3, PID=1, Command=10)
            let col_idx = match sort {
                ProcSort::Cpu => 2,
                ProcSort::Mem => 3,
                ProcSort::Pid => 1,
                ProcSort::Command => 10,
            };
            let ascending = !*sort_desc;

            self.table_sort = TableSort {
                column: Some(col_idx),
                ascending,
            };

            // Re-sort current data
            Self::sort_table_rows(&mut self.native_output, col_idx, ascending);
            Self::sort_table_rows(&mut self.stream_latest, col_idx, ascending);

            return true;
        }
        false
    }

    /// Sort rows in a table Value (if it is a Table).
    fn sort_table_rows(value: &mut Option<Value>, col_idx: usize, ascending: bool) {
        if let Some(Value::Table { rows, .. }) = value {
            rows.sort_by(|a, b| {
                let va = a.get(col_idx);
                let vb = b.get(col_idx);
                let cmp = match (va, vb) {
                    // Compare numeric values
                    (Some(Value::Int(na)), Some(Value::Int(nb))) => na.cmp(nb),
                    (Some(Value::Float(na)), Some(Value::Float(nb))) => {
                        na.partial_cmp(nb).unwrap_or(std::cmp::Ordering::Equal)
                    }
                    (Some(Value::Int(na)), Some(Value::Float(nb))) => {
                        (*na as f64).partial_cmp(nb).unwrap_or(std::cmp::Ordering::Equal)
                    }
                    (Some(Value::Float(na)), Some(Value::Int(nb))) => {
                        na.partial_cmp(&(*nb as f64)).unwrap_or(std::cmp::Ordering::Equal)
                    }
                    // Fall back to text comparison
                    (Some(a), Some(b)) => a.to_text().cmp(&b.to_text()),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                };
                if ascending { cmp } else { cmp.reverse() }
            });
        }
    }

    /// Count tree nodes for TreeBrowser navigation bounds.
    fn tree_node_count(&self) -> usize {
        use nexus_api::DomainValue;
        self.native_output
            .as_ref()
            .and_then(|v| {
                if let Some(DomainValue::Tree(tree)) = v.as_domain() {
                    Some(tree.nodes.len())
                } else {
                    None
                }
            })
            .unwrap_or(0)
    }

    /// Count diff files for DiffViewer navigation bounds.
    fn diff_file_count(&self) -> usize {
        self.native_output
            .as_ref()
            .and_then(|v| {
                if let Value::List(items) = v {
                    Some(items.len())
                } else {
                    None
                }
            })
            .unwrap_or(0)
    }

    // =========================================================================
    // Clipboard helpers — encapsulate block data extraction for copy operations
    // =========================================================================

    /// Get the block's output text (native or terminal).
    pub fn copy_output(&self) -> String {
        if let Some(ref value) = self.native_output {
            value.to_text()
        } else {
            self.parser.grid_with_scrollback().to_string()
        }
    }

    /// Get the block's table output as TSV, if it has table output.
    pub fn copy_as_tsv(&self) -> Option<String> {
        if let Some(Value::Table { columns, rows }) = &self.native_output {
            let mut buf = String::new();
            // Header row
            for (i, col) in columns.iter().enumerate() {
                if i > 0 { buf.push('\t'); }
                buf.push_str(&col.name);
            }
            buf.push('\n');
            // Data rows
            for row in rows {
                for (i, cell) in row.iter().enumerate() {
                    if i > 0 { buf.push('\t'); }
                    let text = cell.to_text();
                    // Escape tabs/newlines within cell text
                    buf.push_str(&text.replace('\t', " ").replace('\n', " "));
                }
                buf.push('\n');
            }
            Some(buf)
        } else {
            None
        }
    }

    /// Get the block's native output as pretty-printed JSON, if it has native output.
    pub fn copy_as_json(&self) -> Option<String> {
        self.native_output
            .as_ref()
            .and_then(|v| serde_json::to_string_pretty(v).ok())
    }
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        // Different blocks are never equal
        if self.id != other.id {
            return false;
        }

        // Running blocks always need redrawing (cursor, new output)
        if self.is_running() {
            return false;
        }

        // Finished blocks: check if anything visual changed
        self.version == other.version
            && self.collapsed == other.collapsed
            && self.parser.size() == other.parser.size()
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use nexus_api::BlockId;

    use super::*;

    // ========== UnifiedBlock tests ==========

    #[test]
    fn test_unified_block_shell_id() {
        let block = Block::new(BlockId(123), "ls".to_string());
        let unified = UnifiedBlock::Shell(block);
        assert_eq!(unified.id(), BlockId(123));
    }

    // ========== Block tests ==========

    #[test]
    fn test_block_new() {
        let block = Block::new(BlockId(1), "echo hello".to_string());
        assert_eq!(block.id, BlockId(1));
        assert_eq!(block.command, "echo hello");
        assert!(block.is_running());
        assert!(!block.collapsed);
        assert!(block.native_output.is_none());
    }

    #[test]
    fn test_block_is_running() {
        let mut block = Block::new(BlockId(1), "cmd".to_string());
        assert!(block.is_running());
        block.state = BlockState::Success;
        assert!(!block.is_running());
    }

    #[test]
    fn test_block_ensure_file_tree() {
        let mut block = Block::new(BlockId(1), "ls".to_string());
        assert!(block.file_tree.is_none());

        let tree = block.ensure_file_tree();
        tree.toggle(PathBuf::from("/test"));

        assert!(block.file_tree.is_some());
        assert!(block.file_tree().unwrap().is_expanded(Path::new("/test")));
    }

    #[test]
    fn test_block_file_tree_accessor() {
        let block = Block::new(BlockId(1), "ls".to_string());
        assert!(block.file_tree().is_none());
    }

    #[test]
    fn test_block_partial_eq_different_ids() {
        let block1 = Block::new(BlockId(1), "ls".to_string());
        let block2 = Block::new(BlockId(2), "ls".to_string());
        assert_ne!(block1, block2);
    }

    #[test]
    fn test_block_partial_eq_running_always_ne() {
        let block1 = Block::new(BlockId(1), "ls".to_string());
        let block2 = Block::new(BlockId(1), "ls".to_string());
        // Running blocks always return false for eq
        assert_ne!(block1, block2);
    }

    #[test]
    fn test_block_partial_eq_finished_same_version() {
        let mut block1 = Block::new(BlockId(1), "ls".to_string());
        let mut block2 = Block::new(BlockId(1), "ls".to_string());
        block1.state = BlockState::Success;
        block2.state = BlockState::Success;
        block1.version = 5;
        block2.version = 5;
        assert_eq!(block1, block2);
    }

    #[test]
    fn test_block_partial_eq_different_version() {
        let mut block1 = Block::new(BlockId(1), "ls".to_string());
        let mut block2 = Block::new(BlockId(1), "ls".to_string());
        block1.state = BlockState::Success;
        block2.state = BlockState::Success;
        block1.version = 1;
        block2.version = 2;
        assert_ne!(block1, block2);
    }

    #[test]
    fn test_block_partial_eq_different_collapsed() {
        let mut block1 = Block::new(BlockId(1), "ls".to_string());
        let mut block2 = Block::new(BlockId(1), "ls".to_string());
        block1.state = BlockState::Success;
        block2.state = BlockState::Success;
        block1.collapsed = true;
        block2.collapsed = false;
        assert_ne!(block1, block2);
    }
}
