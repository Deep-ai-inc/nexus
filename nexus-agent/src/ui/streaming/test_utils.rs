//! Common test utilities for streaming processors
//!
//! This module contains shared test helpers, mocks and utilities that are used
//! by both the XML and JSON processor tests.
use crate::ui::streaming::DisplayFragment;
use crate::ui::{UIError, UserInterface};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A test UI that collects display fragments and merges them appropriately
#[derive(Clone)]
pub struct TestUI {
    fragments: Arc<Mutex<VecDeque<DisplayFragment>>>,
    raw_fragments: Arc<Mutex<Vec<DisplayFragment>>>, // Added to record raw fragments
}

impl TestUI {
    pub fn new() -> Self {
        Self {
            fragments: Arc::new(Mutex::new(VecDeque::new())),
            raw_fragments: Arc::new(Mutex::new(Vec::new())), // Initialize new field
        }
    }

    pub fn get_fragments(&self) -> Vec<DisplayFragment> {
        let guard = self.fragments.lock().unwrap();
        guard.iter().cloned().collect()
    }

    // Attempt to merge a new fragment with the last one if they are of the same type
    fn merge_fragments(last: &mut DisplayFragment, new: &DisplayFragment) -> bool {
        match (last, new) {
            // Merge plain text fragments
            (DisplayFragment::PlainText(last_text), DisplayFragment::PlainText(new_text)) => {
                last_text.push_str(new_text);
                true
            }

            // Merge thinking text fragments
            (DisplayFragment::ThinkingText(last_text), DisplayFragment::ThinkingText(new_text)) => {
                last_text.push_str(new_text);
                true
            }

            // Merge tool parameters with the same name and tool_id
            (
                DisplayFragment::ToolParameter {
                    name: last_name,
                    value: last_value,
                    tool_id: last_id,
                },
                DisplayFragment::ToolParameter {
                    name: new_name,
                    value: new_value,
                    tool_id: new_id,
                },
            ) => {
                if last_name == new_name && last_id == new_id {
                    last_value.push_str(new_value);
                    true
                } else {
                    false
                }
            }

            // No other fragments can be merged
            _ => false,
        }
    }

    // Method to get the raw, unmerged fragments
    pub fn get_raw_fragments(&self) -> Vec<DisplayFragment> {
        self.raw_fragments.lock().unwrap().clone()
    }
}

#[async_trait]
impl UserInterface for TestUI {
    async fn send_event(&self, _event: crate::ui::UiEvent) -> Result<(), UIError> {
        Ok(())
    }

    fn display_fragment(&self, fragment: &DisplayFragment) -> Result<(), UIError> {
        // Record the raw fragment before any merging
        self.raw_fragments.lock().unwrap().push(fragment.clone());

        let mut guard = self.fragments.lock().unwrap();

        // Check if we can merge this fragment with the previous one
        if let Some(last_fragment) = guard.back_mut() {
            if Self::merge_fragments(last_fragment, fragment) {
                // Successfully merged, don't add a new fragment
                return Ok(());
            }
        }

        // If we couldn't merge, add the new fragment
        guard.push_back(fragment.clone());
        Ok(())
    }

    fn should_streaming_continue(&self) -> bool {
        // Test implementation always continues streaming
        true
    }

    fn notify_rate_limit(&self, _seconds_remaining: u64) {
        // Test implementation does nothing with rate limit notifications
    }

    fn clear_rate_limit(&self) {
        // Test implementation does nothing with rate limit clearing
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Helper function to split text into small chunks for testing tag handling
pub fn chunk_str(s: &str, chunk_size: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut chunks = Vec::new();

    for chunk in chars.chunks(chunk_size) {
        chunks.push(chunk.iter().collect::<String>());
    }

    chunks
}

/// Helper function to print fragments for debugging
#[allow(dead_code)]
pub fn print_fragments(fragments: &[DisplayFragment]) {
    println!("Collected {} fragments:", fragments.len());
    for (i, fragment) in fragments.iter().enumerate() {
        match fragment {
            DisplayFragment::PlainText(text) => println!("  [{i}] PlainText: {text}"),
            DisplayFragment::Image {
                media_type: _,
                data: _,
            } => println!("  [{i}] Image: ..."),
            DisplayFragment::ThinkingText(text) => println!("  [{i}] ThinkingText: {text}"),
            DisplayFragment::ToolName { name, id } => {
                println!("  [{i}] ToolName: {name} (id: {id})")
            }
            DisplayFragment::ToolParameter {
                name,
                value,
                tool_id,
            } => println!("  [{i}] ToolParameter: {name}={value} (tool_id: {tool_id})"),
            DisplayFragment::ToolEnd { id } => println!("  [{i}] ToolEnd: (id: {id})"),
            DisplayFragment::ReasoningSummaryStart => {
                println!("  [{i}] ReasoningSummaryStart",);
            }
            DisplayFragment::ReasoningSummaryDelta(delta) => {
                println!(
                    "  [{i}] ReasoningSummaryDelta: {}",
                    delta.chars().take(50).collect::<String>()
                )
            }
            DisplayFragment::ToolOutput { tool_id, chunk } => {
                println!("  [{i}] ToolOutput(tool_id: {tool_id}, chunk: {chunk:?})")
            }
            DisplayFragment::ToolTerminal {
                tool_id,
                terminal_id,
            } => println!("  [{i}] ToolTerminal(tool_id: {tool_id}, terminal_id: {terminal_id})"),
            DisplayFragment::ReasoningComplete => println!("  [{i}] ReasoningComplete"),

            DisplayFragment::CompactionDivider { summary } => {
                println!("  [{i}] CompactionDivider: {summary}");
            }
            DisplayFragment::HiddenToolCompleted => {
                println!("  [{i}] HiddenToolCompleted");
            }
        }
    }
}

/// Helper function to check if two fragments match in content (ignoring IDs)
pub fn fragments_match(expected: &DisplayFragment, actual: &DisplayFragment) -> bool {
    match (expected, actual) {
        (DisplayFragment::PlainText(expected_text), DisplayFragment::PlainText(actual_text)) => {
            expected_text == actual_text
        }
        (
            DisplayFragment::ThinkingText(expected_text),
            DisplayFragment::ThinkingText(actual_text),
        ) => expected_text == actual_text,
        (
            DisplayFragment::ToolName {
                name: expected_name,
                ..
            },
            DisplayFragment::ToolName {
                name: actual_name, ..
            },
        ) => expected_name == actual_name,
        (
            DisplayFragment::ToolParameter {
                name: expected_name,
                value: expected_value,
                ..
            },
            DisplayFragment::ToolParameter {
                name: actual_name,
                value: actual_value,
                ..
            },
        ) => expected_name == actual_name && expected_value == actual_value,
        (DisplayFragment::ToolEnd { .. }, DisplayFragment::ToolEnd { .. }) => true,
        (DisplayFragment::ReasoningSummaryStart, DisplayFragment::ReasoningSummaryStart) => true,
        (
            DisplayFragment::ReasoningSummaryDelta(expected_delta),
            DisplayFragment::ReasoningSummaryDelta(actual_delta),
        ) => expected_delta == actual_delta,
        (
            DisplayFragment::ToolTerminal {
                terminal_id: expected_terminal,
                ..
            },
            DisplayFragment::ToolTerminal {
                terminal_id: actual_terminal,
                ..
            },
        ) => expected_terminal == actual_terminal,
        (
            DisplayFragment::CompactionDivider { summary: expected },
            DisplayFragment::CompactionDivider { summary: actual },
        ) => expected == actual,
        _ => false,
    }
}

/// Helper function to sort ToolParameter fragments by name (for order-independent comparison)
fn sort_key(fragment: &DisplayFragment) -> (u8, String) {
    match fragment {
        DisplayFragment::PlainText(_) => (0, String::new()),
        DisplayFragment::ToolName { name, .. } => (1, name.clone()),
        DisplayFragment::ToolParameter { name, .. } => (2, name.clone()),
        DisplayFragment::ToolEnd { .. } => (3, String::new()),
        _ => (4, String::new()),
    }
}

/// Helper function to assert that actual fragments match expected fragments.
/// ToolParameter fragments within a tool call are compared as a set (order-independent)
/// to handle non-deterministic JSON object iteration order.
pub fn assert_fragments_match(expected: &[DisplayFragment], actual: &[DisplayFragment]) {
    assert_eq!(
        expected.len(),
        actual.len(),
        "Different number of fragments. Expected {}, got {}",
        expected.len(),
        actual.len()
    );

    // Sort both by the sort key to make ToolParameter order-independent
    let mut expected_sorted: Vec<_> = expected.to_vec();
    let mut actual_sorted: Vec<_> = actual.to_vec();
    expected_sorted.sort_by_key(|f| sort_key(f));
    actual_sorted.sort_by_key(|f| sort_key(f));

    for (i, (expected, actual)) in expected_sorted.iter().zip(actual_sorted.iter()).enumerate() {
        assert!(
            fragments_match(expected, actual),
            "Fragment mismatch at position {i}: \nExpected: {expected:?}\nActual: {actual:?}"
        );
    }
}
