//! Context menu types.
//!
//! Rendering is handled by native macOS NSMenu (see `strata::platform::show_context_menu`).

use std::path::PathBuf;

use nexus_api::BlockId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextMenuItem {
    Copy,
    Paste,
    SelectAll,
    Clear,
    CopyCommand,
    CopyOutput,
    CopyAsJson,
    CopyAsTsv,
    Rerun,
    // File-specific actions
    QuickLook(PathBuf),
    Open(PathBuf),
    CopyPath(PathBuf),
    RevealInFinder(PathBuf),
    // Table cell actions
    /// Copy the cell's display text to the clipboard.
    CopyCellValue(String),
    /// Filter this column to rows matching this value.
    FilterByValue { value: String, col: usize },
    /// Filter this column to exclude rows matching this value.
    ExcludeValue { value: String, col: usize },
    /// Clear filter on a specific column.
    ClearColumnFilter(BlockId, usize),
    /// Clear all filters on this table.
    ClearAllFilters(BlockId),
}

impl ContextMenuItem {
    pub fn label(&self) -> &str {
        match self {
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::SelectAll => "Select All",
            Self::Clear => "Clear",
            Self::CopyCommand => "Copy Command",
            Self::CopyOutput => "Copy Output",
            Self::CopyAsJson => "Copy as JSON",
            Self::CopyAsTsv => "Copy as TSV",
            Self::Rerun => "Rerun",
            Self::QuickLook(_) => "Quick Look",
            Self::Open(_) => "Open",
            Self::CopyPath(_) => "Copy Path",
            Self::RevealInFinder(_) => "Reveal in Finder",
            Self::CopyCellValue(_) => "Copy Cell Value",
            Self::FilterByValue { .. } => "Filter to This Value",
            Self::ExcludeValue { .. } => "Exclude This Value",
            Self::ClearColumnFilter(_, _) => "Clear Column Filter",
            Self::ClearAllFilters(_) => "Clear All Filters",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ContextTarget {
    Block(BlockId),
    AgentBlock(BlockId),
    Input,
    /// A specific cell in a table (for cell-level actions).
    TableCell { block_id: BlockId, row: usize, col: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_menu_item_label_copy() {
        assert_eq!(ContextMenuItem::Copy.label(), "Copy");
    }

    #[test]
    fn test_context_menu_item_label_paste() {
        assert_eq!(ContextMenuItem::Paste.label(), "Paste");
    }

    #[test]
    fn test_context_menu_item_label_select_all() {
        assert_eq!(ContextMenuItem::SelectAll.label(), "Select All");
    }

    #[test]
    fn test_context_menu_item_label_clear() {
        assert_eq!(ContextMenuItem::Clear.label(), "Clear");
    }

    #[test]
    fn test_context_menu_item_label_copy_command() {
        assert_eq!(ContextMenuItem::CopyCommand.label(), "Copy Command");
    }

    #[test]
    fn test_context_menu_item_label_copy_output() {
        assert_eq!(ContextMenuItem::CopyOutput.label(), "Copy Output");
    }

    #[test]
    fn test_context_menu_item_label_copy_as_json() {
        assert_eq!(ContextMenuItem::CopyAsJson.label(), "Copy as JSON");
    }

    #[test]
    fn test_context_menu_item_label_copy_as_tsv() {
        assert_eq!(ContextMenuItem::CopyAsTsv.label(), "Copy as TSV");
    }

    #[test]
    fn test_context_menu_item_label_rerun() {
        assert_eq!(ContextMenuItem::Rerun.label(), "Rerun");
    }

    #[test]
    fn test_context_menu_item_label_quick_look() {
        let item = ContextMenuItem::QuickLook(PathBuf::from("/test"));
        assert_eq!(item.label(), "Quick Look");
    }

    #[test]
    fn test_context_menu_item_label_open() {
        let item = ContextMenuItem::Open(PathBuf::from("/test"));
        assert_eq!(item.label(), "Open");
    }

    #[test]
    fn test_context_menu_item_label_copy_path() {
        let item = ContextMenuItem::CopyPath(PathBuf::from("/test"));
        assert_eq!(item.label(), "Copy Path");
    }

    #[test]
    fn test_context_menu_item_label_reveal_in_finder() {
        let item = ContextMenuItem::RevealInFinder(PathBuf::from("/test"));
        assert_eq!(item.label(), "Reveal in Finder");
    }
}
