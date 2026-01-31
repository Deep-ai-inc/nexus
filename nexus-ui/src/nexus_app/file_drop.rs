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

/// Shell-quote a path for safe insertion into the input bar.
pub fn shell_quote(path: &std::path::Path) -> String {
    let s = path.to_string_lossy();
    if s.contains(|c: char| c.is_whitespace() || "\"'\\$`!#&|;(){}[]<>?*~".contains(c)) {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.into_owned()
    }
}
