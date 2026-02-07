//! Viewer state types: ViewState, FileTreeState, TableSort.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use nexus_api::{BlockId, FileEntry};

use super::enums::ProcSort;

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
}
