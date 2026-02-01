//! File drop resolution — maps hit-test results to drop zones.

use strata::content_address::SourceId;
use strata::layout_snapshot::HitResult;

use super::message::DropZone;
use super::source_ids;
use super::NexusState;

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
