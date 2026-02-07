//! Viewer state types: ViewState, FileTreeState, TableSort.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use nexus_api::{BlockId, FileEntry};

use super::enums::ProcSort;

/// Column sort state for table output. Clicking the same column header
/// toggles ascending/descending; clicking a different column resets to ascending.
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

/// Interactive viewer overlay attached to a finished block.
///
/// When a command produces structured output that supports interactive
/// exploration (less, top, tree, git diff), a `ViewState` is attached
/// to the block. It captures navigation state (scroll position, selected
/// node, sort column) and maps vim-style keys to `ViewerMsg` actions.
/// Pressing `q` exits the viewer and drops this state.
#[derive(Debug)]
pub enum ViewState {
    /// Scrollable text viewer (less, man pages).
    Pager {
        scroll_line: usize,
        search: Option<String>,
        current_match: usize,
    },
    /// Live process table (top) with sortable columns.
    ProcessMonitor {
        sort_by: ProcSort,
        sort_desc: bool,
        interval_ms: u64,
    },
    /// Expandable directory tree (tree command).
    TreeBrowser {
        collapsed: HashSet<usize>,
        selected: Option<usize>,
    },
    /// Side-by-side diff viewer (git diff).
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
    ) -> Option<crate::app::message::ViewerMsg> {
        use strata::event_context::{Key, NamedKey};
        use crate::app::message::ViewerMsg;

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

    // ========== ViewState navigation tests ==========

    fn make_pager() -> ViewState {
        ViewState::Pager { scroll_line: 10, search: None, current_match: 0 }
    }

    fn make_tree() -> ViewState {
        ViewState::TreeBrowser { collapsed: HashSet::new(), selected: Some(3) }
    }

    fn make_diff() -> ViewState {
        ViewState::DiffViewer { scroll_line: 5, current_file: 2, collapsed_indices: HashSet::new() }
    }

    fn make_proc() -> ViewState {
        ViewState::ProcessMonitor { sort_by: ProcSort::Cpu, sort_desc: true, interval_ms: 1000 }
    }

    // --- Scroll ---

    #[test]
    fn test_scroll_up_pager() {
        let mut vs = make_pager();
        assert!(vs.scroll_up());
        if let ViewState::Pager { scroll_line, .. } = vs { assert_eq!(scroll_line, 9); }
    }

    #[test]
    fn test_scroll_up_at_zero_clamps() {
        let mut vs = ViewState::Pager { scroll_line: 0, search: None, current_match: 0 };
        assert!(vs.scroll_up());
        if let ViewState::Pager { scroll_line, .. } = vs { assert_eq!(scroll_line, 0); }
    }

    #[test]
    fn test_scroll_up_on_tree_returns_false() {
        let mut vs = make_tree();
        assert!(!vs.scroll_up());
    }

    #[test]
    fn test_scroll_down_pager() {
        let mut vs = make_pager();
        assert!(vs.scroll_down());
        if let ViewState::Pager { scroll_line, .. } = vs { assert_eq!(scroll_line, 11); }
    }

    #[test]
    fn test_scroll_down_diff() {
        let mut vs = make_diff();
        assert!(vs.scroll_down());
        if let ViewState::DiffViewer { scroll_line, .. } = vs { assert_eq!(scroll_line, 6); }
    }

    #[test]
    fn test_scroll_down_on_proc_returns_false() {
        let mut vs = make_proc();
        assert!(!vs.scroll_down());
    }

    // --- Page ---

    #[test]
    fn test_page_up_pager() {
        let mut vs = ViewState::Pager { scroll_line: 50, search: None, current_match: 0 };
        assert!(vs.page_up());
        if let ViewState::Pager { scroll_line, .. } = vs { assert_eq!(scroll_line, 20); }
    }

    #[test]
    fn test_page_up_clamps_at_zero() {
        let mut vs = ViewState::Pager { scroll_line: 10, search: None, current_match: 0 };
        assert!(vs.page_up());
        if let ViewState::Pager { scroll_line, .. } = vs { assert_eq!(scroll_line, 0); }
    }

    #[test]
    fn test_page_down_pager() {
        let mut vs = make_pager();
        assert!(vs.page_down());
        if let ViewState::Pager { scroll_line, .. } = vs { assert_eq!(scroll_line, 40); }
    }

    // --- Go to top/bottom ---

    #[test]
    fn test_go_to_top() {
        let mut vs = make_pager();
        assert!(vs.go_to_top());
        if let ViewState::Pager { scroll_line, .. } = vs { assert_eq!(scroll_line, 0); }
    }

    #[test]
    fn test_go_to_bottom() {
        let mut vs = make_pager();
        assert!(vs.go_to_bottom());
        if let ViewState::Pager { scroll_line, .. } = vs { assert!(scroll_line > 1_000_000); }
    }

    // --- Tree navigation ---

    #[test]
    fn test_tree_up() {
        let mut vs = make_tree();
        assert!(vs.tree_up());
        if let ViewState::TreeBrowser { selected, .. } = vs { assert_eq!(selected, Some(2)); }
    }

    #[test]
    fn test_tree_up_at_zero_clamps() {
        let mut vs = ViewState::TreeBrowser { collapsed: HashSet::new(), selected: Some(0) };
        assert!(vs.tree_up());
        if let ViewState::TreeBrowser { selected, .. } = vs { assert_eq!(selected, Some(0)); }
    }

    #[test]
    fn test_tree_down() {
        let mut vs = make_tree();
        assert!(vs.tree_down(10));
        if let ViewState::TreeBrowser { selected, .. } = vs { assert_eq!(selected, Some(4)); }
    }

    #[test]
    fn test_tree_down_at_max_clamps() {
        let mut vs = ViewState::TreeBrowser { collapsed: HashSet::new(), selected: Some(9) };
        assert!(vs.tree_down(10));
        if let ViewState::TreeBrowser { selected, .. } = vs { assert_eq!(selected, Some(9)); }
    }

    #[test]
    fn test_tree_toggle_collapse() {
        let mut vs = make_tree();
        assert!(vs.tree_toggle(Some(5)));
        if let ViewState::TreeBrowser { collapsed, .. } = &vs {
            assert!(collapsed.contains(&5));
        }
        // Toggle again to uncollapse
        assert!(vs.tree_toggle(Some(5)));
        if let ViewState::TreeBrowser { collapsed, .. } = &vs {
            assert!(!collapsed.contains(&5));
        }
    }

    #[test]
    fn test_tree_toggle_none_selected() {
        let mut vs = make_tree();
        assert!(!vs.tree_toggle(None));
    }

    #[test]
    fn test_tree_on_pager_returns_false() {
        let mut vs = make_pager();
        assert!(!vs.tree_up());
        assert!(!vs.tree_down(10));
    }

    // --- Diff navigation ---

    #[test]
    fn test_diff_next_file() {
        let mut vs = make_diff();
        assert!(vs.diff_next_file(5));
        if let ViewState::DiffViewer { current_file, .. } = vs { assert_eq!(current_file, 3); }
    }

    #[test]
    fn test_diff_next_file_at_max_clamps() {
        let mut vs = ViewState::DiffViewer { scroll_line: 0, current_file: 4, collapsed_indices: HashSet::new() };
        assert!(vs.diff_next_file(5));
        if let ViewState::DiffViewer { current_file, .. } = vs { assert_eq!(current_file, 4); }
    }

    #[test]
    fn test_diff_prev_file() {
        let mut vs = make_diff();
        assert!(vs.diff_prev_file());
        if let ViewState::DiffViewer { current_file, .. } = vs { assert_eq!(current_file, 1); }
    }

    #[test]
    fn test_diff_prev_file_at_zero_clamps() {
        let mut vs = ViewState::DiffViewer { scroll_line: 0, current_file: 0, collapsed_indices: HashSet::new() };
        assert!(vs.diff_prev_file());
        if let ViewState::DiffViewer { current_file, .. } = vs { assert_eq!(current_file, 0); }
    }

    #[test]
    fn test_diff_toggle_file() {
        let mut vs = make_diff(); // current_file = 2
        assert!(vs.diff_toggle_file());
        if let ViewState::DiffViewer { collapsed_indices, .. } = &vs {
            assert!(collapsed_indices.contains(&2));
        }
        // Toggle again
        assert!(vs.diff_toggle_file());
        if let ViewState::DiffViewer { collapsed_indices, .. } = &vs {
            assert!(!collapsed_indices.contains(&2));
        }
    }

    #[test]
    fn test_diff_on_pager_returns_false() {
        let mut vs = make_pager();
        assert!(!vs.diff_next_file(5));
        assert!(!vs.diff_prev_file());
        assert!(!vs.diff_toggle_file());
    }

    // --- Accessors ---

    #[test]
    fn test_tree_selected() {
        let vs = make_tree();
        assert_eq!(vs.tree_selected(), Some(3));
    }

    #[test]
    fn test_tree_selected_on_pager() {
        let vs = make_pager();
        assert_eq!(vs.tree_selected(), None);
    }

    #[test]
    fn test_diff_current_file() {
        let vs = make_diff();
        assert_eq!(vs.diff_current_file(), Some(2));
    }

    #[test]
    fn test_diff_current_file_on_tree() {
        let vs = make_tree();
        assert_eq!(vs.diff_current_file(), None);
    }

    // ========== handle_key tests ==========

    use nexus_api::BlockId;
    use strata::event_context::{Key, NamedKey};
    use crate::app::message::ViewerMsg;

    const ID: BlockId = BlockId(1);

    // --- Pager keys ---

    #[test]
    fn test_pager_key_j_scrolls_down() {
        let vs = make_pager();
        assert_eq!(vs.handle_key(ID, &Key::character("j")), Some(ViewerMsg::ScrollDown(ID)));
    }

    #[test]
    fn test_pager_key_k_scrolls_up() {
        let vs = make_pager();
        assert_eq!(vs.handle_key(ID, &Key::character("k")), Some(ViewerMsg::ScrollUp(ID)));
    }

    #[test]
    fn test_pager_space_pages_down() {
        let vs = make_pager();
        assert_eq!(vs.handle_key(ID, &Key::named(NamedKey::Space)), Some(ViewerMsg::PageDown(ID)));
    }

    #[test]
    fn test_pager_key_b_pages_up() {
        let vs = make_pager();
        assert_eq!(vs.handle_key(ID, &Key::character("b")), Some(ViewerMsg::PageUp(ID)));
    }

    #[test]
    fn test_pager_key_g_goes_to_top() {
        let vs = make_pager();
        assert_eq!(vs.handle_key(ID, &Key::character("g")), Some(ViewerMsg::GoToTop(ID)));
    }

    #[test]
    fn test_pager_key_shift_g_goes_to_bottom() {
        let vs = make_pager();
        assert_eq!(vs.handle_key(ID, &Key::character("G")), Some(ViewerMsg::GoToBottom(ID)));
    }

    #[test]
    fn test_pager_key_q_exits() {
        let vs = make_pager();
        assert_eq!(vs.handle_key(ID, &Key::character("q")), Some(ViewerMsg::Exit(ID)));
    }

    #[test]
    fn test_pager_unhandled_key() {
        let vs = make_pager();
        assert_eq!(vs.handle_key(ID, &Key::character("z")), None);
    }

    // --- ProcessMonitor keys ---

    #[test]
    fn test_proc_key_c_sorts_cpu() {
        let vs = make_proc();
        assert_eq!(vs.handle_key(ID, &Key::character("c")), Some(ViewerMsg::SortBy(ID, ProcSort::Cpu)));
    }

    #[test]
    fn test_proc_key_m_sorts_mem() {
        let vs = make_proc();
        assert_eq!(vs.handle_key(ID, &Key::character("m")), Some(ViewerMsg::SortBy(ID, ProcSort::Mem)));
    }

    #[test]
    fn test_proc_key_p_sorts_pid() {
        let vs = make_proc();
        assert_eq!(vs.handle_key(ID, &Key::character("p")), Some(ViewerMsg::SortBy(ID, ProcSort::Pid)));
    }

    #[test]
    fn test_proc_key_q_exits() {
        let vs = make_proc();
        assert_eq!(vs.handle_key(ID, &Key::character("q")), Some(ViewerMsg::Exit(ID)));
    }

    #[test]
    fn test_proc_unhandled_key() {
        let vs = make_proc();
        assert_eq!(vs.handle_key(ID, &Key::character("x")), None);
    }

    // --- TreeBrowser keys ---

    #[test]
    fn test_tree_key_arrow_up() {
        let vs = make_tree();
        assert_eq!(vs.handle_key(ID, &Key::named(NamedKey::ArrowUp)), Some(ViewerMsg::TreeUp(ID)));
    }

    #[test]
    fn test_tree_key_j_down() {
        let vs = make_tree();
        assert_eq!(vs.handle_key(ID, &Key::character("j")), Some(ViewerMsg::TreeDown(ID)));
    }

    #[test]
    fn test_tree_key_space_toggles() {
        let vs = make_tree();
        assert_eq!(vs.handle_key(ID, &Key::named(NamedKey::Space)), Some(ViewerMsg::TreeToggle(ID)));
    }

    #[test]
    fn test_tree_key_enter_toggles() {
        let vs = make_tree();
        assert_eq!(vs.handle_key(ID, &Key::named(NamedKey::Enter)), Some(ViewerMsg::TreeToggle(ID)));
    }

    #[test]
    fn test_tree_key_q_exits() {
        let vs = make_tree();
        assert_eq!(vs.handle_key(ID, &Key::character("q")), Some(ViewerMsg::Exit(ID)));
    }

    // --- DiffViewer keys ---

    #[test]
    fn test_diff_key_j_scrolls_down() {
        let vs = make_diff();
        assert_eq!(vs.handle_key(ID, &Key::character("j")), Some(ViewerMsg::ScrollDown(ID)));
    }

    #[test]
    fn test_diff_key_n_next_file() {
        let vs = make_diff();
        assert_eq!(vs.handle_key(ID, &Key::character("n")), Some(ViewerMsg::DiffNextFile(ID)));
    }

    #[test]
    fn test_diff_key_p_prev_file() {
        let vs = make_diff();
        assert_eq!(vs.handle_key(ID, &Key::character("p")), Some(ViewerMsg::DiffPrevFile(ID)));
    }

    #[test]
    fn test_diff_space_toggles_file() {
        let vs = make_diff();
        assert_eq!(vs.handle_key(ID, &Key::named(NamedKey::Space)), Some(ViewerMsg::DiffToggleFile(ID)));
    }

    #[test]
    fn test_diff_key_q_exits() {
        let vs = make_diff();
        assert_eq!(vs.handle_key(ID, &Key::character("q")), Some(ViewerMsg::Exit(ID)));
    }
}
