//! File drop resolution — maps hit-test results to drop zones.

use strata::content_address::SourceId;
use strata::layout_snapshot::HitResult;

use crate::app::message::DropZone;
use crate::utils::ids as source_ids;
use crate::app::NexusState;

/// Resolve the drop zone from the current hit-test result.
pub fn resolve_drop_zone(state: &NexusState, hit: &Option<HitResult>) -> DropZone {
    match hit {
        // Hit a widget — check if it's the input bar area
        Some(HitResult::Widget(id)) => {
            if is_input_widget(state, *id) {
                DropZone::InputBar
            } else if let Some(block_id) = state.shell.block_for_source(*id) {
                DropZone::ShellBlock(block_id)
            } else if state.agent.block_for_source(*id).is_some() {
                DropZone::AgentPanel
            } else {
                DropZone::Empty
            }
        }
        // Hit content — check which block it belongs to
        Some(HitResult::Content(addr)) => {
            if let Some(block_id) = state.shell.block_for_source(addr.source_id) {
                DropZone::ShellBlock(block_id)
            } else if state.agent.block_for_source(addr.source_id).is_some() {
                DropZone::AgentPanel
            } else {
                DropZone::Empty
            }
        }
        None => DropZone::Empty,
    }
}

fn is_input_widget(_state: &NexusState, id: SourceId) -> bool {
    // The input bar's text input widget has a known ID
    id == source_ids::mode_toggle()
}

// =========================================================================
// Nexus temp file helpers (for native drag round-trip)
// =========================================================================

const NEXUS_DRAG_DIR: &str = "nexus-drag";

/// Get the temp directory used for nexus drag files.
pub fn nexus_temp_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(NEXUS_DRAG_DIR)
}

/// Check if a path is a nexus temp file (from our own drag operation).
pub fn is_nexus_temp_file(path: &std::path::Path) -> bool {
    path.starts_with(nexus_temp_dir())
}

/// Read a nexus temp file and return its content as text to insert.
/// Returns `None` if the file is not a nexus temp file or can't be read.
pub fn read_temp_file_content(path: &std::path::Path) -> Option<String> {
    if !is_nexus_temp_file(path) {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// Write drag data to a temp file, returning the path.
/// Does NOT clean stale files — the platform layer does that before each drag.
pub fn write_drag_temp_file(filename: &str, data: &[u8]) -> Result<std::path::PathBuf, std::io::Error> {
    let temp_dir = nexus_temp_dir();
    std::fs::create_dir_all(&temp_dir)?;
    let path = temp_dir.join(filename);
    std::fs::write(&path, data)?;
    Ok(path)
}

/// Shell-quote a path for safe insertion into the input bar.
pub fn shell_quote(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    if s.contains(|c: char| c.is_whitespace() || "\"'\\$`!#&|;(){}[]<>?*~".contains(c)) {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_nexus_temp_dir() {
        let temp_dir = nexus_temp_dir();
        assert!(temp_dir.ends_with("nexus-drag"));
    }

    #[test]
    fn test_is_nexus_temp_file_true() {
        let temp_dir = nexus_temp_dir();
        let path = temp_dir.join("test.txt");
        assert!(is_nexus_temp_file(&path));
    }

    #[test]
    fn test_is_nexus_temp_file_false() {
        let path = Path::new("/some/other/path/file.txt");
        assert!(!is_nexus_temp_file(path));
    }

    #[test]
    fn test_is_nexus_temp_file_nested() {
        let temp_dir = nexus_temp_dir();
        let path = temp_dir.join("subdir/nested.txt");
        assert!(is_nexus_temp_file(&path));
    }

    #[test]
    fn test_read_temp_file_content_not_nexus_file() {
        let path = Path::new("/some/random/file.txt");
        assert!(read_temp_file_content(path).is_none());
    }

    #[test]
    fn test_shell_quote_simple_path() {
        let path = Path::new("/simple/path/file.txt");
        assert_eq!(shell_quote(path), "/simple/path/file.txt");
    }

    #[test]
    fn test_shell_quote_with_spaces() {
        let path = Path::new("/path/with spaces/file.txt");
        assert_eq!(shell_quote(path), "'/path/with spaces/file.txt'");
    }

    #[test]
    fn test_shell_quote_with_special_chars() {
        let path = Path::new("/path/with$dollar");
        assert_eq!(shell_quote(path), "'/path/with$dollar'");
    }

    #[test]
    fn test_shell_quote_with_single_quote() {
        let path = Path::new("/path/it's/file.txt");
        assert_eq!(shell_quote(path), "'/path/it'\\''s/file.txt'");
    }

    #[test]
    fn test_shell_quote_with_backtick() {
        let path = Path::new("/path/with`backtick");
        assert_eq!(shell_quote(path), "'/path/with`backtick'");
    }

    #[test]
    fn test_shell_quote_with_asterisk() {
        let path = Path::new("/path/*wild");
        assert_eq!(shell_quote(path), "'/path/*wild'");
    }

    #[test]
    fn test_shell_quote_with_question_mark() {
        let path = Path::new("/path/what?");
        assert_eq!(shell_quote(path), "'/path/what?'");
    }

    #[test]
    fn test_drop_zone_equality() {
        use crate::app::message::DropZone;
        use nexus_api::BlockId;

        assert_eq!(DropZone::InputBar, DropZone::InputBar);
        assert_eq!(DropZone::AgentPanel, DropZone::AgentPanel);
        assert_eq!(DropZone::Empty, DropZone::Empty);
        assert_eq!(DropZone::ShellBlock(BlockId(1)), DropZone::ShellBlock(BlockId(1)));
        assert_ne!(DropZone::ShellBlock(BlockId(1)), DropZone::ShellBlock(BlockId(2)));
        assert_ne!(DropZone::InputBar, DropZone::AgentPanel);
    }
}
