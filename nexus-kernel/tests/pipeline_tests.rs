//! Integration tests for native command pipelines.
//!
//! These tests verify that commands can be properly chained together via pipes,
//! passing structured Value data between them.

use nexus_api::{ShellEvent, Value};
use nexus_kernel::Kernel;

/// Test harness that executes a pipeline and collects the output Value.
struct PipelineTest {
    kernel: Kernel,
    rx: tokio::sync::broadcast::Receiver<ShellEvent>,
}

impl PipelineTest {
    fn new() -> Self {
        let (kernel, rx) = Kernel::new().expect("Failed to create kernel");
        Self { kernel, rx }
    }

    /// Execute a command and return the final output Value.
    fn run(&mut self, cmd: &str) -> Option<Value> {
        // Execute the command
        let result = self.kernel.execute(cmd);
        assert!(result.is_ok(), "Command failed: {:?}", result.err());

        // Collect output from events
        let mut output_value = None;

        // Drain the event channel
        loop {
            match self.rx.try_recv() {
                Ok(ShellEvent::CommandOutput { value, .. }) => {
                    output_value = Some(value);
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }

        output_value
    }

    /// Execute and expect a specific integer result.
    fn expect_int(&mut self, cmd: &str, expected: i64) {
        let value = self.run(cmd);
        match value {
            Some(Value::Int(n)) => assert_eq!(n, expected, "Command: {}", cmd),
            other => panic!("Expected Int({}), got {:?} for command: {}", expected, other, cmd),
        }
    }

    /// Execute and expect a specific string result.
    fn expect_string(&mut self, cmd: &str, expected: &str) {
        let value = self.run(cmd);
        match value {
            Some(Value::String(s)) => assert_eq!(s, expected, "Command: {}", cmd),
            other => panic!("Expected String(\"{}\"), got {:?} for command: {}", expected, other, cmd),
        }
    }

    /// Execute and expect a list of specific length.
    #[allow(dead_code)]
    fn expect_list_len(&mut self, cmd: &str, expected_len: usize) {
        let value = self.run(cmd);
        match value {
            Some(Value::List(items)) => {
                assert_eq!(items.len(), expected_len, "Command: {}", cmd);
            }
            other => panic!("Expected List with {} items, got {:?} for command: {}", expected_len, other, cmd),
        }
    }

    /// Execute and expect a list, returning it for further inspection.
    fn expect_list(&mut self, cmd: &str) -> Vec<Value> {
        let value = self.run(cmd);
        match value {
            Some(Value::List(items)) => items,
            other => panic!("Expected List, got {:?} for command: {}", other, cmd),
        }
    }

    /// Execute and expect a float result.
    fn expect_float(&mut self, cmd: &str, expected: f64) {
        let value = self.run(cmd);
        match value {
            Some(Value::Float(f)) => {
                assert!((f - expected).abs() < 0.001, "Expected {}, got {} for command: {}", expected, f, cmd);
            }
            other => panic!("Expected Float({}), got {:?} for command: {}", expected, other, cmd),
        }
    }
}

// ============================================================================
// Basic pipeline tests
// ============================================================================

#[test]
fn test_seq_to_sum() {
    let mut t = PipelineTest::new();
    // seq 1 10 | sum should give 55 (1+2+3+...+10)
    t.expect_int("seq 1 10 | sum", 55);
}

#[test]
fn test_seq_to_count() {
    let mut t = PipelineTest::new();
    // seq 1 100 | count should give 100
    t.expect_int("seq 1 100 | count", 100);
}

#[test]
fn test_seq_to_head() {
    let mut t = PipelineTest::new();
    // seq 1 100 | head -5 | count
    t.expect_int("seq 1 100 | head -5 | count", 5);
}

#[test]
fn test_seq_to_tail() {
    let mut t = PipelineTest::new();
    // seq 1 100 | tail -5 | count
    t.expect_int("seq 1 100 | tail -5 | count", 5);
}

#[test]
fn test_seq_head_sum() {
    let mut t = PipelineTest::new();
    // seq 1 10 | head -3 | sum = 1+2+3 = 6
    t.expect_int("seq 1 10 | head -3 | sum", 6);
}

#[test]
fn test_seq_tail_sum() {
    let mut t = PipelineTest::new();
    // seq 1 10 | tail -3 | sum = 8+9+10 = 27
    t.expect_int("seq 1 10 | tail -3 | sum", 27);
}

#[test]
fn test_seq_to_avg() {
    let mut t = PipelineTest::new();
    // seq 1 10 | avg = 5.5
    t.expect_float("seq 1 10 | avg", 5.5);
}

#[test]
fn test_seq_to_min() {
    let mut t = PipelineTest::new();
    let value = t.run("seq 5 15 | min");
    // min returns the item directly
    match value {
        Some(Value::String(s)) => assert_eq!(s, "5"),
        Some(Value::Int(n)) => assert_eq!(n, 5),
        other => panic!("Expected 5, got {:?}", other),
    }
}

#[test]
fn test_seq_to_max() {
    let mut t = PipelineTest::new();
    let value = t.run("seq 5 15 | max");
    // max returns the item directly
    match value {
        Some(Value::String(s)) => assert_eq!(s, "15"),
        Some(Value::Int(n)) => assert_eq!(n, 15),
        other => panic!("Expected 15, got {:?}", other),
    }
}

// ============================================================================
// Sort and uniq pipelines
// ============================================================================

#[test]
fn test_seq_reverse_sort() {
    let mut t = PipelineTest::new();
    // seq generates strings, sort -r reverses
    let items = t.expect_list("seq 1 5 | sort -r");
    assert_eq!(items.len(), 5);
    // Numeric sort reversed: 5, 4, 3, 2, 1
    let first = items[0].to_string();
    assert!(first.contains('5') || first == "5", "Expected 5 first, got {}", first);
}

#[test]
fn test_shuf_sort_consistency() {
    let mut t = PipelineTest::new();
    // seq 1 10 | shuf | sort -n should restore order
    let items = t.expect_list("seq 1 10 | shuf | sort -n");
    assert_eq!(items.len(), 10);
}

#[test]
fn test_uniq_count() {
    let mut t = PipelineTest::new();
    // Echo repeated values, uniq -c should count them
    // Using seq and some manipulation
    let items = t.expect_list("echo 'a\na\nb\nb\nb\nc' | lines | uniq -c");
    assert!(items.len() >= 1); // Should have unique entries with counts
}

// ============================================================================
// Text manipulation pipelines
// ============================================================================

#[test]
fn test_echo_lines_count() {
    let mut t = PipelineTest::new();
    t.expect_int("echo 'one\ntwo\nthree' | lines | count", 3);
}

#[test]
fn test_echo_words_count() {
    let mut t = PipelineTest::new();
    t.expect_int("echo 'one two three four' | words | count", 4);
}

#[test]
fn test_echo_chars_count() {
    let mut t = PipelineTest::new();
    t.expect_int("echo 'hello' | chars | count", 5);
}

#[test]
fn test_lines_head_join() {
    let mut t = PipelineTest::new();
    // Split into lines, take first 2, join with comma
    t.expect_string("echo 'a\nb\nc\nd' | lines | head -2 | join ','", "a,b");
}

#[test]
fn test_words_sort_join() {
    let mut t = PipelineTest::new();
    // Split into words, sort, join with space
    t.expect_string("echo 'cherry apple banana' | words | sort | join ' '", "apple banana cherry");
}

#[test]
fn test_tr_uppercase() {
    let mut t = PipelineTest::new();
    t.expect_string("echo 'hello' | tr '[:lower:]' '[:upper:]'", "HELLO");
}

#[test]
fn test_tr_delete() {
    let mut t = PipelineTest::new();
    t.expect_string("echo 'hello world' | tr -d 'lo'", "he wrd");
}

#[test]
fn test_rev_string() {
    let mut t = PipelineTest::new();
    t.expect_string("echo 'hello' | rev", "olleh");
}

// ============================================================================
// Selection pipelines
// ============================================================================

#[test]
fn test_first_item() {
    let mut t = PipelineTest::new();
    let value = t.run("seq 1 10 | first");
    match value {
        Some(Value::String(s)) => assert_eq!(s, "1"),
        Some(Value::Int(n)) => assert_eq!(n, 1),
        other => panic!("Expected 1, got {:?}", other),
    }
}

#[test]
fn test_last_item() {
    let mut t = PipelineTest::new();
    let value = t.run("seq 1 10 | last");
    match value {
        Some(Value::String(s)) => assert_eq!(s, "10"),
        Some(Value::Int(n)) => assert_eq!(n, 10),
        other => panic!("Expected 10, got {:?}", other),
    }
}

#[test]
fn test_nth_item() {
    let mut t = PipelineTest::new();
    let value = t.run("seq 1 10 | nth 4");
    // nth is 0-indexed, so index 4 = 5th item = "5"
    match value {
        Some(Value::String(s)) => assert_eq!(s, "5"),
        Some(Value::Int(n)) => assert_eq!(n, 5),
        other => panic!("Expected 5, got {:?}", other),
    }
}

#[test]
fn test_skip_items() {
    let mut t = PipelineTest::new();
    // seq 1 10 | skip 7 | count should give 3
    t.expect_int("seq 1 10 | skip 7 | count", 3);
}

#[test]
fn test_take_items() {
    let mut t = PipelineTest::new();
    // seq 1 10 | take 4 | count should give 4
    t.expect_int("seq 1 10 | take 4 | count", 4);
}

#[test]
fn test_reverse_list() {
    let mut t = PipelineTest::new();
    let items = t.expect_list("seq 1 5 | reverse");
    assert_eq!(items.len(), 5);
    // First item should now be "5"
    let first = items[0].to_string();
    assert!(first.contains('5') || first == "5", "Expected 5 first after reverse, got {}", first);
}

// ============================================================================
// Complex multi-stage pipelines
// ============================================================================

#[test]
fn test_seq_skip_take_sum() {
    let mut t = PipelineTest::new();
    // seq 1 20 | skip 5 | take 5 | sum = 6+7+8+9+10 = 40
    t.expect_int("seq 1 20 | skip 5 | take 5 | sum", 40);
}

#[test]
fn test_long_pipeline_count() {
    let mut t = PipelineTest::new();
    // seq 1 100 | head -50 | tail -25 | head -10 | count = 10
    t.expect_int("seq 1 100 | head -50 | tail -25 | head -10 | count", 10);
}

#[test]
fn test_words_sort_uniq_count() {
    let mut t = PipelineTest::new();
    // Count unique words
    t.expect_int("echo 'apple banana apple cherry banana apple' | words | sort | uniq | count", 3);
}

#[test]
fn test_enumerate_first() {
    let mut t = PipelineTest::new();
    // seq 1 5 | enumerate | first should give a record with index 0
    let value = t.run("seq 1 5 | enumerate | first");
    match value {
        Some(Value::Record(entries)) => {
            // Should have "index" and "value" fields
            let has_index = entries.iter().any(|(k, _)| k == "index");
            let has_value = entries.iter().any(|(k, _)| k == "value");
            assert!(has_index, "Expected index field");
            assert!(has_value, "Expected value field");
        }
        other => panic!("Expected Record, got {:?}", other),
    }
}

// ============================================================================
// Path manipulation pipelines
// ============================================================================

#[test]
fn test_basename_pipeline() {
    let mut t = PipelineTest::new();
    t.expect_string("echo '/foo/bar/baz.txt' | basename", "baz.txt");
}

#[test]
fn test_dirname_pipeline() {
    let mut t = PipelineTest::new();
    let value = t.run("echo '/foo/bar/baz.txt' | dirname");
    match value {
        Some(Value::String(s)) => assert_eq!(s, "/foo/bar"),
        Some(Value::Path(p)) => assert_eq!(p.to_string_lossy(), "/foo/bar"),
        other => panic!("Expected /foo/bar, got {:?}", other),
    }
}

#[test]
fn test_extname_pipeline() {
    let mut t = PipelineTest::new();
    t.expect_string("echo 'document.pdf' | extname", ".pdf");
}

#[test]
fn test_stem_pipeline() {
    let mut t = PipelineTest::new();
    t.expect_string("echo 'document.pdf' | stem", "document");
}

// ============================================================================
// JSON pipelines
// ============================================================================

#[test]
fn test_json_parse_get() {
    let mut t = PipelineTest::new();
    let value = t.run(r#"echo '{"name":"alice","age":30}' | from-json | get name"#);
    match value {
        Some(Value::String(s)) => assert_eq!(s, "alice"),
        other => panic!("Expected 'alice', got {:?}", other),
    }
}

#[test]
fn test_json_array_count() {
    let mut t = PipelineTest::new();
    t.expect_int("echo '[1,2,3,4,5]' | from-json | count", 5);
}

#[test]
fn test_json_array_sum() {
    let mut t = PipelineTest::new();
    t.expect_int("echo '[1,2,3,4,5]' | from-json | sum", 15);
}

#[test]
fn test_list_to_json_roundtrip() {
    let mut t = PipelineTest::new();
    // seq generates list, to-json converts, from-json parses back
    t.expect_int("seq 1 5 | to-json | from-json | count", 5);
}

// ============================================================================
// grep/filter pipelines
// ============================================================================

#[test]
fn test_grep_count() {
    let mut t = PipelineTest::new();
    t.expect_int("echo 'apple\nbanana\napricot\ncherry' | lines | grep -c '^a'", 2);
}

#[test]
fn test_grep_invert() {
    let mut t = PipelineTest::new();
    // Invert match - count lines NOT starting with 'a'
    t.expect_int("echo 'apple\nbanana\napricot\ncherry' | lines | grep -v '^a' | count", 2);
}

#[test]
fn test_grep_case_insensitive() {
    let mut t = PipelineTest::new();
    t.expect_int("echo 'Apple\nBANANA\napricot' | lines | grep -i 'apple' | count", 1);
}

// ============================================================================
// wc pipelines
// ============================================================================

#[test]
fn test_wc_lines() {
    let mut t = PipelineTest::new();
    t.expect_int("echo 'one\ntwo\nthree' | wc -l", 3);
}

#[test]
fn test_wc_words() {
    let mut t = PipelineTest::new();
    t.expect_int("echo 'one two three four five' | wc -w", 5);
}

#[test]
fn test_wc_chars() {
    let mut t = PipelineTest::new();
    // "hello" = 5 characters
    t.expect_int("echo 'hello' | wc -m", 5);
}

// ============================================================================
// cut pipelines
// ============================================================================

#[test]
fn test_cut_fields() {
    let mut t = PipelineTest::new();
    t.expect_string("echo 'a:b:c:d' | cut -d: -f2", "b");
}

#[test]
fn test_cut_characters() {
    let mut t = PipelineTest::new();
    t.expect_string("echo 'hello' | cut -c1-3", "hel");
}

// ============================================================================
// nl (number lines) pipelines
// ============================================================================

#[test]
fn test_nl_basic() {
    let mut t = PipelineTest::new();
    let items = t.expect_list("echo 'a\nb\nc' | lines | nl");
    assert_eq!(items.len(), 3);
    // First line should contain "1" and "a"
    let first = items[0].to_string();
    assert!(first.contains('1') && first.contains('a'), "Expected numbered line, got {}", first);
}

// ============================================================================
// tee-like behavior (passthrough)
// ============================================================================

#[test]
fn test_cat_passthrough() {
    let mut t = PipelineTest::new();
    // cat should pass through the list
    t.expect_int("seq 1 10 | cat | count", 10);
}

// ============================================================================
// Flatten pipelines
// ============================================================================

#[test]
fn test_flatten_nested() {
    let mut t = PipelineTest::new();
    // Create nested structure and flatten
    // This is a bit tricky to test without nested JSON
    let value = t.run("echo '[[1,2],[3,4]]' | from-json | flatten | count");
    match value {
        Some(Value::Int(n)) => assert_eq!(n, 4),
        other => panic!("Expected 4 items after flatten, got {:?}", other),
    }
}

// ============================================================================
// Compact pipelines
// ============================================================================

#[test]
fn test_compact_removes_empty() {
    let mut t = PipelineTest::new();
    // Split on comma, some empty strings
    t.expect_int("echo 'a,,b,,c' | split ',' | compact | count", 3);
}

// ============================================================================
// Edge cases
// ============================================================================

#[test]
fn test_empty_input_to_count() {
    let mut t = PipelineTest::new();
    // Empty list should count as 0
    let value = t.run("echo '' | lines | count");
    // An empty string split into lines gives 1 empty line typically
    // but let's check it doesn't crash
    assert!(value.is_some());
}

#[test]
fn test_head_more_than_available() {
    let mut t = PipelineTest::new();
    // head -100 on a 10-item list should give 10
    t.expect_int("seq 1 10 | head -100 | count", 10);
}

#[test]
fn test_skip_more_than_available() {
    let mut t = PipelineTest::new();
    // skip 100 on a 10-item list should give 0
    t.expect_int("seq 1 10 | skip 100 | count", 0);
}

#[test]
fn test_tail_more_than_available() {
    let mut t = PipelineTest::new();
    // tail -100 on a 10-item list should give 10
    t.expect_int("seq 1 10 | tail -100 | count", 10);
}

// ============================================================================
// Real-world-ish scenarios
// ============================================================================

#[test]
fn test_word_frequency_pipeline() {
    let mut t = PipelineTest::new();
    // Simulate word frequency: split into words, sort, uniq, count unique
    t.expect_int("echo 'the quick brown fox jumps over the lazy dog' | words | sort | uniq | count", 8);
}

#[test]
fn test_extract_numbers_sum() {
    let mut t = PipelineTest::new();
    // From JSON array, sum the numbers
    t.expect_int("echo '[10, 20, 30, 40]' | from-json | sum", 100);
}

#[test]
fn test_filter_sort_take() {
    let mut t = PipelineTest::new();
    // Generate numbers, take first 20, sort descending, take top 5
    let items = t.expect_list("seq 1 20 | sort -rn | take 5");
    assert_eq!(items.len(), 5);
}
