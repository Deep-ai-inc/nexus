//! Core block types: Block, UnifiedBlock, UnifiedBlockRef.

use std::collections::VecDeque;
use std::sync::atomic::AtomicU16;
use std::time::Instant;

use nexus_api::{BlockId, BlockState, OutputFormat, Value};
use nexus_term::TerminalParser;

use crate::data::agent_block::AgentBlock;
use super::enums::ProcSort;
use super::view::{ViewState, FileTreeState, TableSort};

/// A display item in the main scrollable view. Shell and Agent blocks are
/// interleaved in ascending `BlockId` order; the ID determines position.
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

/// A shell command block: user-typed command + its output.
///
/// Output can take three mutually-exclusive forms, checked in priority order:
/// 1. `live_value` — live-updating structured value (progress bar, table in `top`).
/// 2. `structured_output` — one-shot structured value (table from `ls`, tree from `find`).
/// 3. Terminal grid — raw PTY output stored in `parser`.
///
/// `event_log` is an orthogonal append-only event log (e.g. ping replies)
/// rendered below the primary output.
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
    pub structured_output: Option<Value>,
    /// Sort state for table output.
    pub table_sort: TableSort,
    /// Whether output contained "permission denied".
    pub has_permission_denied: bool,
    /// Whether output contained "command not found".
    pub has_command_not_found: bool,
    /// Append-only event log (ping replies, etc.). Capped at 1000 entries.
    pub event_log: VecDeque<Value>,
    /// Latest coalesced state (progress bar, live table, etc.).
    pub live_value: Option<Value>,
    /// Sequence counter for ordering streaming updates.
    pub event_seq: u64,
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
            structured_output: None,
            table_sort: TableSort::new(),
            has_permission_denied: false,
            has_command_not_found: false,
            event_log: VecDeque::new(),
            live_value: None,
            event_seq: 0,
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
            Self::sort_table_rows(&mut self.structured_output, col_idx, ascending);
            Self::sort_table_rows(&mut self.live_value, col_idx, ascending);

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
        self.structured_output
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
        self.structured_output
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
        if let Some(ref value) = self.structured_output {
            value.to_text()
        } else {
            self.parser.grid_with_scrollback().to_string()
        }
    }

    /// Get the block's table output as TSV, if it has table output.
    pub fn copy_as_tsv(&self) -> Option<String> {
        if let Some(Value::Table { columns, rows }) = &self.structured_output {
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
        self.structured_output
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
        assert!(block.structured_output.is_none());
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

    // ========== sort_table_rows tests ==========

    fn make_table(rows: Vec<Vec<Value>>) -> Option<Value> {
        Some(Value::Table {
            columns: vec![
                nexus_api::TableColumn::new("a"),
                nexus_api::TableColumn::new("b"),
            ],
            rows,
        })
    }

    fn extract_col0_ints(value: &Option<Value>) -> Vec<i64> {
        if let Some(Value::Table { rows, .. }) = value {
            rows.iter()
                .filter_map(|r| if let Value::Int(n) = r[0] { Some(n) } else { None })
                .collect()
        } else {
            vec![]
        }
    }

    #[test]
    fn test_sort_table_rows_int_ascending() {
        let mut table = make_table(vec![
            vec![Value::Int(3), Value::String("c".into())],
            vec![Value::Int(1), Value::String("a".into())],
            vec![Value::Int(2), Value::String("b".into())],
        ]);
        Block::sort_table_rows(&mut table, 0, true);
        assert_eq!(extract_col0_ints(&table), vec![1, 2, 3]);
    }

    #[test]
    fn test_sort_table_rows_int_descending() {
        let mut table = make_table(vec![
            vec![Value::Int(1), Value::String("a".into())],
            vec![Value::Int(3), Value::String("c".into())],
            vec![Value::Int(2), Value::String("b".into())],
        ]);
        Block::sort_table_rows(&mut table, 0, false);
        assert_eq!(extract_col0_ints(&table), vec![3, 2, 1]);
    }

    #[test]
    fn test_sort_table_rows_float() {
        let mut table = make_table(vec![
            vec![Value::Float(2.5), Value::Unit],
            vec![Value::Float(1.1), Value::Unit],
            vec![Value::Float(3.9), Value::Unit],
        ]);
        Block::sort_table_rows(&mut table, 0, true);
        if let Some(Value::Table { rows, .. }) = &table {
            let vals: Vec<f64> = rows.iter().filter_map(|r| {
                if let Value::Float(f) = r[0] { Some(f) } else { None }
            }).collect();
            assert_eq!(vals, vec![1.1, 2.5, 3.9]);
        }
    }

    #[test]
    fn test_sort_table_rows_mixed_int_float() {
        let mut table = make_table(vec![
            vec![Value::Float(2.5), Value::Unit],
            vec![Value::Int(1), Value::Unit],
            vec![Value::Float(1.5), Value::Unit],
            vec![Value::Int(3), Value::Unit],
        ]);
        Block::sort_table_rows(&mut table, 0, true);
        if let Some(Value::Table { rows, .. }) = &table {
            let vals: Vec<String> = rows.iter().map(|r| r[0].to_text()).collect();
            assert_eq!(vals, vec!["1", "1.5", "2.5", "3"]);
        }
    }

    #[test]
    fn test_sort_table_rows_text_fallback() {
        let mut table = make_table(vec![
            vec![Value::String("banana".into()), Value::Unit],
            vec![Value::String("apple".into()), Value::Unit],
            vec![Value::String("cherry".into()), Value::Unit],
        ]);
        Block::sort_table_rows(&mut table, 0, true);
        if let Some(Value::Table { rows, .. }) = &table {
            let vals: Vec<String> = rows.iter().map(|r| r[0].to_text()).collect();
            assert_eq!(vals, vec!["apple", "banana", "cherry"]);
        }
    }

    #[test]
    fn test_sort_table_rows_none_values() {
        // Rows shorter than col_idx → get(col_idx) returns None
        let mut table = make_table(vec![
            vec![Value::Int(2)],       // row has 1 element, col 1 is None
            vec![Value::Int(1), Value::Int(10)],
        ]);
        Block::sort_table_rows(&mut table, 1, true);
        if let Some(Value::Table { rows, .. }) = &table {
            // None sorts after Some
            assert_eq!(rows[0].len(), 2); // row with value first
            assert_eq!(rows[1].len(), 1); // short row last
        }
    }

    #[test]
    fn test_sort_table_rows_not_a_table() {
        let mut value = Some(Value::String("not a table".into()));
        Block::sort_table_rows(&mut value, 0, true);
        // Should be no-op
        assert_eq!(value, Some(Value::String("not a table".into())));
    }

    #[test]
    fn test_sort_table_rows_none_input() {
        let mut value: Option<Value> = None;
        Block::sort_table_rows(&mut value, 0, true);
        assert!(value.is_none());
    }

    // ========== Clipboard helper tests ==========

    #[test]
    fn test_copy_as_tsv_basic() {
        let mut block = Block::new(BlockId(1), "ls".to_string());
        block.structured_output = Some(Value::Table {
            columns: vec![
                nexus_api::TableColumn::new("name"),
                nexus_api::TableColumn::new("size"),
            ],
            rows: vec![
                vec![Value::String("foo.txt".into()), Value::Int(100)],
                vec![Value::String("bar.rs".into()), Value::Int(200)],
            ],
        });
        let tsv = block.copy_as_tsv().unwrap();
        assert_eq!(tsv, "name\tsize\nfoo.txt\t100\nbar.rs\t200\n");
    }

    #[test]
    fn test_copy_as_tsv_escapes_tabs_and_newlines() {
        let mut block = Block::new(BlockId(1), "cmd".to_string());
        block.structured_output = Some(Value::Table {
            columns: vec![nexus_api::TableColumn::new("val")],
            rows: vec![
                vec![Value::String("has\ttab".into())],
                vec![Value::String("has\nnewline".into())],
            ],
        });
        let tsv = block.copy_as_tsv().unwrap();
        assert_eq!(tsv, "val\nhas tab\nhas newline\n");
    }

    #[test]
    fn test_copy_as_tsv_not_a_table() {
        let mut block = Block::new(BlockId(1), "cmd".to_string());
        block.structured_output = Some(Value::String("hello".into()));
        assert!(block.copy_as_tsv().is_none());
    }

    #[test]
    fn test_copy_as_tsv_no_output() {
        let block = Block::new(BlockId(1), "cmd".to_string());
        assert!(block.copy_as_tsv().is_none());
    }

    #[test]
    fn test_copy_as_json() {
        let mut block = Block::new(BlockId(1), "cmd".to_string());
        block.structured_output = Some(Value::Record(vec![
            ("key".into(), Value::String("value".into())),
        ]));
        let json = block.copy_as_json().unwrap();
        assert!(json.contains("key"));
        assert!(json.contains("value"));
    }

    #[test]
    fn test_copy_as_json_no_output() {
        let block = Block::new(BlockId(1), "cmd".to_string());
        assert!(block.copy_as_json().is_none());
    }

    #[test]
    fn test_copy_output_with_structured() {
        let mut block = Block::new(BlockId(1), "cmd".to_string());
        block.structured_output = Some(Value::String("structured text".into()));
        assert_eq!(block.copy_output(), "structured text");
    }

    #[test]
    fn test_copy_output_falls_back_to_terminal() {
        let block = Block::new(BlockId(1), "cmd".to_string());
        // No structured output, falls back to parser grid (empty for new block)
        let output = block.copy_output();
        assert!(output.is_empty() || output.chars().all(|c| c.is_whitespace()));
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
