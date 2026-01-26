//! History search handler.
//!
//! Pure functions that take `&mut InputState` and kernel reference.

use std::sync::Arc;

use tokio::sync::Mutex;

use nexus_kernel::Kernel;

use crate::handlers::input::InputResult;
use crate::state::InputState;

/// Start history search mode.
pub fn start(input: &mut InputState, kernel: &Arc<Mutex<Kernel>>) -> InputResult {
    input.search_active = true;
    input.search_query.clear();
    input.search_index = 0;

    let kernel_guard = kernel.blocking_lock();
    input.search_results = kernel_guard.get_recent_history(50);
    drop(kernel_guard);

    InputResult::none()
}

/// Update history search with new query.
pub fn search(input: &mut InputState, kernel: &Arc<Mutex<Kernel>>, query: String) -> InputResult {
    input.search_query = query.clone();
    input.search_index = 0;

    let kernel_guard = kernel.blocking_lock();
    if query.is_empty() {
        input.search_results = kernel_guard.get_recent_history(50);
    } else {
        let search_query = format!("\"{}\"*", query.replace('"', "\"\""));
        input.search_results = kernel_guard.search_history(&search_query, 50);
    }
    drop(kernel_guard);

    InputResult::none()
}

/// Select a history search result.
pub fn select(input: &mut InputState, index: usize) -> InputResult {
    if let Some(entry) = input.search_results.get(index) {
        input.buffer = entry.command.clone();
    }
    input.search_active = false;
    input.search_query.clear();
    input.search_results.clear();
    InputResult::none()
}

/// Cancel history search.
pub fn cancel(input: &mut InputState) -> InputResult {
    input.search_active = false;
    input.search_query.clear();
    input.search_results.clear();
    InputResult::none()
}

