//! Block and related types for representing command execution in the UI.

use std::collections::{HashMap, HashSet};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU16;
use std::time::Instant;

use nexus_api::{BlockId, BlockState, FileEntry, OutputFormat, Value};
use nexus_term::TerminalParser;

use crate::agent_block::AgentBlock;

/// Sort state for a table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableSort {
    /// Which column is being sorted (by index).
    pub column: Option<usize>,
    /// Sort direction (true = ascending, false = descending).
    pub ascending: bool,
}

impl TableSort {
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle sort on a column. If already sorting by this column, reverse direction.
    /// If sorting by a different column, start ascending.
    pub fn toggle(&mut self, column_index: usize) {
        if self.column == Some(column_index) {
            self.ascending = !self.ascending;
        } else {
            self.column = Some(column_index);
            self.ascending = true;
        }
    }
}

/// Lazy tree expansion state for file list output.
/// Only allocated when a user expands a directory chevron.
#[derive(Debug, Clone, Default)]
pub struct FileTreeState {
    /// Which directory paths are currently expanded.
    pub expanded: HashSet<PathBuf>,
    /// Cached contents of expanded directories.
    pub children: HashMap<PathBuf, Vec<FileEntry>>,
}

impl FileTreeState {
    /// Check if a path is expanded.
    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    /// Toggle expansion of a directory.
    /// Returns true if the directory is now expanded (needs content loading).
    /// On collapse, clears expanded descendants and their cached children.
    pub fn toggle(&mut self, path: PathBuf) -> bool {
        if self.expanded.contains(&path) {
            self.collapse_subtree(&path);
            false
        } else {
            self.expanded.insert(path);
            true
        }
    }

    /// Store children for an expanded directory.
    pub fn set_children(&mut self, path: PathBuf, entries: Vec<FileEntry>) {
        self.children.insert(path, entries);
    }

    /// Get children of an expanded directory.
    pub fn get_children(&self, path: &Path) -> Option<&Vec<FileEntry>> {
        self.children.get(path)
    }

    /// Remove a directory and all its descendants from expanded + children.
    fn collapse_subtree(&mut self, root: &Path) {
        self.expanded.retain(|p| !p.starts_with(root));
        self.children.retain(|p, _| !p.starts_with(root));
    }
}

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
    pub fn update_viewer(&mut self, msg: &crate::nexus_app::message::ViewerMsg) -> bool {
        use crate::nexus_app::message::ViewerMsg;

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

/// Focus state - makes illegal states unrepresentable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    /// The command input field is focused.
    Input,
    /// A specific block is focused for interaction.
    Block(BlockId),
    /// The agent question text input is focused.
    AgentInput,
}

/// Input mode - determines how commands are processed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum InputMode {
    /// Normal shell mode - commands are executed by the kernel.
    #[default]
    Shell,
    /// Agent mode - input is sent to the AI agent.
    Agent,
}

/// PTY event types for communication with the PTY subprocess.
#[derive(Debug, Clone)]
pub enum PtyEvent {
    Output(Vec<u8>),
    Exited(i32),
}

/// A job displayed in the status bar.
#[derive(Debug, Clone)]
pub struct VisualJob {
    pub id: u32,
    pub command: String,
    pub state: VisualJobState,
}

/// Visual state of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualJobState {
    Running,
    Stopped,
}

impl VisualJob {
    pub fn new(id: u32, command: String, state: VisualJobState) -> Self {
        Self { id, command, state }
    }

    /// Get a shortened display name for the job.
    pub fn display_name(&self) -> String {
        if self.command.len() > 20 {
            format!("{}...", &self.command[..17])
        } else {
            self.command.clone()
        }
    }

    /// Get the icon for this job state.
    pub fn icon(&self) -> &'static str {
        match self.state {
            VisualJobState::Running => "●",
            VisualJobState::Stopped => "⏸",
        }
    }
}

/// Interactive viewer state attached to a block.
#[derive(Debug)]
pub enum ViewState {
    Pager {
        scroll_line: usize,
        search: Option<String>,
        current_match: usize,
    },
    ProcessMonitor {
        sort_by: ProcSort,
        sort_desc: bool,
        interval_ms: u64,
    },
    TreeBrowser {
        collapsed: HashSet<usize>,
        selected: Option<usize>,
    },
    DiffViewer {
        scroll_line: usize,
        current_file: usize,
        collapsed_indices: HashSet<usize>,
    },
}

impl ViewState {
    /// Map a key press to a viewer message. Returns None if the key is not handled.
    pub fn handle_key(
        &self,
        id: BlockId,
        key: &strata::event_context::Key,
    ) -> Option<crate::nexus_app::message::ViewerMsg> {
        use strata::event_context::{Key, NamedKey};
        use crate::nexus_app::message::ViewerMsg;

        match self {
            ViewState::Pager { .. } => match key {
                Key::Character(c) if c == "j" => Some(ViewerMsg::ScrollDown(id)),
                Key::Character(c) if c == "k" => Some(ViewerMsg::ScrollUp(id)),
                Key::Named(NamedKey::Space) => Some(ViewerMsg::PageDown(id)),
                Key::Character(c) if c == "b" => Some(ViewerMsg::PageUp(id)),
                Key::Character(c) if c == "g" => Some(ViewerMsg::GoToTop(id)),
                Key::Character(c) if c == "G" => Some(ViewerMsg::GoToBottom(id)),
                Key::Character(c) if c == "/" => Some(ViewerMsg::SearchStart(id)),
                Key::Character(c) if c == "n" => Some(ViewerMsg::SearchNext(id)),
                Key::Character(c) if c == "q" => Some(ViewerMsg::Exit(id)),
                _ => None,
            },
            ViewState::ProcessMonitor { .. } => match key {
                Key::Character(c) if c == "c" => Some(ViewerMsg::SortBy(id, ProcSort::Cpu)),
                Key::Character(c) if c == "m" => Some(ViewerMsg::SortBy(id, ProcSort::Mem)),
                Key::Character(c) if c == "p" => Some(ViewerMsg::SortBy(id, ProcSort::Pid)),
                Key::Character(c) if c == "q" => Some(ViewerMsg::Exit(id)),
                _ => None,
            },
            ViewState::TreeBrowser { .. } => match key {
                Key::Named(NamedKey::ArrowUp) => Some(ViewerMsg::TreeUp(id)),
                Key::Character(c) if c == "k" => Some(ViewerMsg::TreeUp(id)),
                Key::Named(NamedKey::ArrowDown) => Some(ViewerMsg::TreeDown(id)),
                Key::Character(c) if c == "j" => Some(ViewerMsg::TreeDown(id)),
                Key::Named(NamedKey::Space) | Key::Named(NamedKey::Enter) => {
                    Some(ViewerMsg::TreeToggle(id))
                }
                Key::Character(c) if c == "q" => Some(ViewerMsg::Exit(id)),
                _ => None,
            },
            ViewState::DiffViewer { .. } => match key {
                Key::Character(c) if c == "j" => Some(ViewerMsg::ScrollDown(id)),
                Key::Character(c) if c == "k" => Some(ViewerMsg::ScrollUp(id)),
                Key::Character(c) if c == "n" => Some(ViewerMsg::DiffNextFile(id)),
                Key::Character(c) if c == "p" => Some(ViewerMsg::DiffPrevFile(id)),
                Key::Named(NamedKey::Space) => Some(ViewerMsg::DiffToggleFile(id)),
                Key::Named(NamedKey::PageDown) => Some(ViewerMsg::PageDown(id)),
                Key::Named(NamedKey::PageUp) => Some(ViewerMsg::PageUp(id)),
                Key::Character(c) if c == "g" => Some(ViewerMsg::GoToTop(id)),
                Key::Character(c) if c == "G" => Some(ViewerMsg::GoToBottom(id)),
                Key::Character(c) if c == "q" => Some(ViewerMsg::Exit(id)),
                _ => None,
            },
        }
    }

    // =========================================================================
    // Scroll/navigation methods — encapsulate viewer state manipulation
    // =========================================================================

    /// Scroll up by one line. Returns true if the state changed.
    pub fn scroll_up(&mut self) -> bool {
        match self {
            ViewState::Pager { scroll_line, .. }
            | ViewState::DiffViewer { scroll_line, .. } => {
                *scroll_line = scroll_line.saturating_sub(1);
                true
            }
            _ => false,
        }
    }

    /// Scroll down by one line. Returns true if the state changed.
    pub fn scroll_down(&mut self) -> bool {
        match self {
            ViewState::Pager { scroll_line, .. }
            | ViewState::DiffViewer { scroll_line, .. } => {
                *scroll_line += 1;
                true
            }
            _ => false,
        }
    }

    /// Page up (30 lines). Returns true if the state changed.
    pub fn page_up(&mut self) -> bool {
        match self {
            ViewState::Pager { scroll_line, .. }
            | ViewState::DiffViewer { scroll_line, .. } => {
                *scroll_line = scroll_line.saturating_sub(30);
                true
            }
            _ => false,
        }
    }

    /// Page down (30 lines). Returns true if the state changed.
    pub fn page_down(&mut self) -> bool {
        match self {
            ViewState::Pager { scroll_line, .. }
            | ViewState::DiffViewer { scroll_line, .. } => {
                *scroll_line += 30;
                true
            }
            _ => false,
        }
    }

    /// Go to top (line 0). Returns true if the state changed.
    pub fn go_to_top(&mut self) -> bool {
        match self {
            ViewState::Pager { scroll_line, .. }
            | ViewState::DiffViewer { scroll_line, .. } => {
                *scroll_line = 0;
                true
            }
            _ => false,
        }
    }

    /// Go to bottom (max line). Returns true if the state changed.
    pub fn go_to_bottom(&mut self) -> bool {
        match self {
            ViewState::Pager { scroll_line, .. }
            | ViewState::DiffViewer { scroll_line, .. } => {
                // Set to a very large value; rendering will clamp
                *scroll_line = usize::MAX / 2;
                true
            }
            _ => false,
        }
    }

    /// Toggle tree node collapse. Returns true if the state changed.
    pub fn tree_toggle(&mut self, selected_idx: Option<usize>) -> bool {
        if let ViewState::TreeBrowser { collapsed, .. } = self {
            if let Some(sel) = selected_idx {
                if collapsed.contains(&sel) {
                    collapsed.remove(&sel);
                } else {
                    collapsed.insert(sel);
                }
                return true;
            }
        }
        false
    }

    /// Move tree selection up. Returns true if the state changed.
    pub fn tree_up(&mut self) -> bool {
        if let ViewState::TreeBrowser { selected, .. } = self {
            if let Some(sel) = selected {
                *sel = sel.saturating_sub(1);
            }
            return true;
        }
        false
    }

    /// Move tree selection down. Returns true if the state changed.
    pub fn tree_down(&mut self, node_count: usize) -> bool {
        if let ViewState::TreeBrowser { selected, .. } = self {
            if let Some(sel) = selected {
                if *sel + 1 < node_count {
                    *sel += 1;
                }
            }
            return true;
        }
        false
    }

    /// Move to next diff file. Returns true if the state changed.
    pub fn diff_next_file(&mut self, file_count: usize) -> bool {
        if let ViewState::DiffViewer { current_file, .. } = self {
            if *current_file + 1 < file_count {
                *current_file += 1;
            }
            return true;
        }
        false
    }

    /// Move to previous diff file. Returns true if the state changed.
    pub fn diff_prev_file(&mut self) -> bool {
        if let ViewState::DiffViewer { current_file, .. } = self {
            *current_file = current_file.saturating_sub(1);
            return true;
        }
        false
    }

    /// Toggle diff file collapse. Returns true if the state changed.
    pub fn diff_toggle_file(&mut self) -> bool {
        if let ViewState::DiffViewer { current_file, collapsed_indices, .. } = self {
            let idx = *current_file;
            if !collapsed_indices.remove(&idx) {
                collapsed_indices.insert(idx);
            }
            return true;
        }
        false
    }

    /// Get the selected tree node index (for TreeBrowser).
    pub fn tree_selected(&self) -> Option<usize> {
        if let ViewState::TreeBrowser { selected, .. } = self {
            *selected
        } else {
            None
        }
    }

    /// Get the current file index (for DiffViewer).
    pub fn diff_current_file(&self) -> Option<usize> {
        if let ViewState::DiffViewer { current_file, .. } = self {
            Some(*current_file)
        } else {
            None
        }
    }
}

/// Sort criteria for process monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcSort {
    Cpu,
    Mem,
    Pid,
    Command,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== TableSort tests ==========

    #[test]
    fn test_table_sort_new_defaults() {
        let sort = TableSort::new();
        assert_eq!(sort.column, None);
        // Default::default() for bool is false
        assert!(!sort.ascending);
    }

    #[test]
    fn test_table_sort_toggle_sets_column() {
        let mut sort = TableSort::new();
        sort.toggle(2);
        assert_eq!(sort.column, Some(2));
        assert!(sort.ascending);
    }

    #[test]
    fn test_table_sort_toggle_same_column_reverses() {
        let mut sort = TableSort::new();
        sort.toggle(1);
        assert!(sort.ascending);
        sort.toggle(1);
        assert!(!sort.ascending);
        sort.toggle(1);
        assert!(sort.ascending);
    }

    #[test]
    fn test_table_sort_toggle_different_column_resets() {
        let mut sort = TableSort::new();
        sort.toggle(0);
        sort.toggle(0); // Now descending
        assert!(!sort.ascending);
        sort.toggle(3); // Switch to different column
        assert_eq!(sort.column, Some(3));
        assert!(sort.ascending); // Reset to ascending
    }

    #[test]
    fn test_table_sort_default_trait() {
        let sort: TableSort = Default::default();
        assert_eq!(sort.column, None);
        // Default::default() for bool is false
        assert!(!sort.ascending);
    }

    #[test]
    fn test_table_sort_clone() {
        let mut sort = TableSort::new();
        sort.toggle(5);
        let cloned = sort.clone();
        assert_eq!(cloned.column, Some(5));
        assert!(cloned.ascending);
    }

    #[test]
    fn test_table_sort_partial_eq() {
        let sort1 = TableSort { column: Some(1), ascending: true };
        let sort2 = TableSort { column: Some(1), ascending: true };
        let sort3 = TableSort { column: Some(1), ascending: false };
        assert_eq!(sort1, sort2);
        assert_ne!(sort1, sort3);
    }

    // ========== FileTreeState tests ==========

    #[test]
    fn test_file_tree_state_default() {
        let tree = FileTreeState::default();
        assert!(tree.expanded.is_empty());
        assert!(tree.children.is_empty());
    }

    #[test]
    fn test_file_tree_is_expanded() {
        let mut tree = FileTreeState::default();
        let path = PathBuf::from("/test/path");
        assert!(!tree.is_expanded(&path));
        tree.expanded.insert(path.clone());
        assert!(tree.is_expanded(&path));
    }

    #[test]
    fn test_file_tree_toggle_expand() {
        let mut tree = FileTreeState::default();
        let path = PathBuf::from("/test/dir");

        // Toggle to expand
        let result = tree.toggle(path.clone());
        assert!(result); // Now expanded
        assert!(tree.is_expanded(&path));
    }

    #[test]
    fn test_file_tree_toggle_collapse() {
        let mut tree = FileTreeState::default();
        let path = PathBuf::from("/test/dir");

        tree.toggle(path.clone()); // Expand
        let result = tree.toggle(path.clone()); // Collapse
        assert!(!result); // Now collapsed
        assert!(!tree.is_expanded(&path));
    }

    #[test]
    fn test_file_tree_set_and_get_children() {
        let mut tree = FileTreeState::default();
        let path = PathBuf::from("/test/dir");
        let entries = vec![
            FileEntry {
                name: "file1.txt".to_string(),
                path: PathBuf::from("/test/dir/file1.txt"),
                file_type: nexus_api::FileType::File,
                size: 100,
                permissions: 0o644,
                modified: None,
                accessed: None,
                created: None,
                is_hidden: false,
                is_symlink: false,
                symlink_target: None,
                uid: None,
                gid: None,
                owner: None,
                group: None,
                nlink: None,
            },
        ];

        tree.set_children(path.clone(), entries.clone());
        let retrieved = tree.get_children(&path);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().len(), 1);
        assert_eq!(retrieved.unwrap()[0].name, "file1.txt");
    }

    #[test]
    fn test_file_tree_get_children_none() {
        let tree = FileTreeState::default();
        let path = PathBuf::from("/nonexistent");
        assert!(tree.get_children(&path).is_none());
    }

    #[test]
    fn test_file_tree_collapse_subtree() {
        let mut tree = FileTreeState::default();
        let root = PathBuf::from("/root");
        let child = PathBuf::from("/root/child");
        let grandchild = PathBuf::from("/root/child/grandchild");
        let unrelated = PathBuf::from("/other");

        // Expand all
        tree.toggle(root.clone());
        tree.toggle(child.clone());
        tree.toggle(grandchild.clone());
        tree.toggle(unrelated.clone());

        // Add children entries
        tree.set_children(root.clone(), vec![]);
        tree.set_children(child.clone(), vec![]);
        tree.set_children(grandchild.clone(), vec![]);

        // Collapse root - should remove root and all descendants
        tree.toggle(root.clone());

        assert!(!tree.is_expanded(&root));
        assert!(!tree.is_expanded(&child));
        assert!(!tree.is_expanded(&grandchild));
        assert!(tree.is_expanded(&unrelated)); // Unrelated path still expanded

        assert!(tree.get_children(&root).is_none());
        assert!(tree.get_children(&child).is_none());
    }

    // ========== VisualJob tests ==========

    #[test]
    fn test_visual_job_new() {
        let job = VisualJob::new(1, "sleep 100".to_string(), VisualJobState::Running);
        assert_eq!(job.id, 1);
        assert_eq!(job.command, "sleep 100");
        assert_eq!(job.state, VisualJobState::Running);
    }

    #[test]
    fn test_visual_job_display_name_short() {
        let job = VisualJob::new(1, "ls -la".to_string(), VisualJobState::Running);
        assert_eq!(job.display_name(), "ls -la");
    }

    #[test]
    fn test_visual_job_display_name_truncates_long() {
        let job = VisualJob::new(1, "this is a very long command that exceeds twenty chars".to_string(), VisualJobState::Running);
        let name = job.display_name();
        assert_eq!(name.len(), 20); // 17 chars + "..."
        assert!(name.ends_with("..."));
    }

    #[test]
    fn test_visual_job_display_name_exactly_20() {
        let job = VisualJob::new(1, "12345678901234567890".to_string(), VisualJobState::Running);
        assert_eq!(job.display_name(), "12345678901234567890");
    }

    #[test]
    fn test_visual_job_icon_running() {
        let job = VisualJob::new(1, "cmd".to_string(), VisualJobState::Running);
        assert_eq!(job.icon(), "●");
    }

    #[test]
    fn test_visual_job_icon_stopped() {
        let job = VisualJob::new(1, "cmd".to_string(), VisualJobState::Stopped);
        assert_eq!(job.icon(), "⏸");
    }

    #[test]
    fn test_visual_job_state_eq() {
        assert_eq!(VisualJobState::Running, VisualJobState::Running);
        assert_eq!(VisualJobState::Stopped, VisualJobState::Stopped);
        assert_ne!(VisualJobState::Running, VisualJobState::Stopped);
    }

    // ========== InputMode tests ==========

    #[test]
    fn test_input_mode_default_is_shell() {
        let mode: InputMode = Default::default();
        assert_eq!(mode, InputMode::Shell);
    }

    #[test]
    fn test_input_mode_variants() {
        assert_eq!(InputMode::Shell, InputMode::Shell);
        assert_eq!(InputMode::Agent, InputMode::Agent);
        assert_ne!(InputMode::Shell, InputMode::Agent);
    }

    #[test]
    fn test_input_mode_clone() {
        let mode = InputMode::Agent;
        let cloned = mode;
        assert_eq!(cloned, InputMode::Agent);
    }

    // ========== Focus tests ==========

    #[test]
    fn test_focus_input() {
        let focus = Focus::Input;
        assert_eq!(focus, Focus::Input);
    }

    #[test]
    fn test_focus_block() {
        let focus = Focus::Block(BlockId(42));
        if let Focus::Block(id) = focus {
            assert_eq!(id.0, 42);
        } else {
            panic!("Expected Focus::Block");
        }
    }

    #[test]
    fn test_focus_agent_input() {
        let focus = Focus::AgentInput;
        assert_eq!(focus, Focus::AgentInput);
    }

    #[test]
    fn test_focus_ne() {
        assert_ne!(Focus::Input, Focus::AgentInput);
        assert_ne!(Focus::Input, Focus::Block(BlockId(1)));
        assert_ne!(Focus::Block(BlockId(1)), Focus::Block(BlockId(2)));
    }

    #[test]
    fn test_focus_clone() {
        let focus = Focus::Block(BlockId(5));
        let cloned = focus;
        assert_eq!(cloned, Focus::Block(BlockId(5)));
    }

    // ========== ProcSort tests ==========

    #[test]
    fn test_proc_sort_variants() {
        assert_eq!(ProcSort::Cpu, ProcSort::Cpu);
        assert_eq!(ProcSort::Mem, ProcSort::Mem);
        assert_eq!(ProcSort::Pid, ProcSort::Pid);
        assert_eq!(ProcSort::Command, ProcSort::Command);
    }

    #[test]
    fn test_proc_sort_ne() {
        assert_ne!(ProcSort::Cpu, ProcSort::Mem);
        assert_ne!(ProcSort::Pid, ProcSort::Command);
    }

    #[test]
    fn test_proc_sort_clone() {
        let sort = ProcSort::Cpu;
        let cloned = sort;
        assert_eq!(cloned, ProcSort::Cpu);
    }

    #[test]
    fn test_proc_sort_debug() {
        let debug_str = format!("{:?}", ProcSort::Cpu);
        assert_eq!(debug_str, "Cpu");
    }

    // ========== PtyEvent tests ==========

    #[test]
    fn test_pty_event_output() {
        let event = PtyEvent::Output(vec![65, 66, 67]);
        if let PtyEvent::Output(data) = event {
            assert_eq!(data, vec![65, 66, 67]);
        } else {
            panic!("Expected PtyEvent::Output");
        }
    }

    #[test]
    fn test_pty_event_exited() {
        let event = PtyEvent::Exited(0);
        if let PtyEvent::Exited(code) = event {
            assert_eq!(code, 0);
        } else {
            panic!("Expected PtyEvent::Exited");
        }
    }

    #[test]
    fn test_pty_event_clone() {
        let event = PtyEvent::Exited(1);
        let cloned = event.clone();
        if let PtyEvent::Exited(code) = cloned {
            assert_eq!(code, 1);
        }
    }

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
