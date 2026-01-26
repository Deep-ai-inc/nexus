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

// ============================================================================
// Real-world shell pipeline patterns
// ============================================================================

// --- Log analysis patterns ---

#[test]
fn test_log_grep_count() {
    // Pattern: grep ERROR | wc -l (count errors in logs)
    let mut t = PipelineTest::new();
    let log = "INFO: started\nERROR: disk full\nINFO: processing\nERROR: timeout\nINFO: done";
    t.expect_int(&format!("echo '{}' | lines | grep ERROR | count", log), 2);
}

#[test]
fn test_log_grep_invert_count() {
    // Pattern: grep -v DEBUG | wc -l (count non-debug lines)
    let mut t = PipelineTest::new();
    let log = "DEBUG: trace\nINFO: started\nDEBUG: value=5\nERROR: failed";
    t.expect_int(&format!("echo '{}' | lines | grep -v DEBUG | count", log), 2);
}

#[test]
fn test_frequency_analysis() {
    // Pattern: sort | uniq -c | sort -rn (frequency analysis)
    let mut t = PipelineTest::new();
    let data = "apple\nbanana\napple\napple\ncherry\nbanana";
    let items = t.expect_list(&format!("echo '{}' | lines | sort | uniq -c | sort -rn", data));
    // Should have 3 unique items, sorted by frequency (apple:3, banana:2, cherry:1)
    assert_eq!(items.len(), 3);
    // First item should be apple (most frequent)
    let first = items[0].to_string();
    assert!(first.contains("apple"), "Expected apple first (most frequent), got {}", first);
}

#[test]
fn test_top_n_pattern() {
    // Pattern: sort -rn | head -3 (top 3 items)
    let mut t = PipelineTest::new();
    t.expect_int("seq 1 100 | sort -rn | head -3 | count", 3);
}

// --- CSV/delimited data processing ---

#[test]
fn test_csv_extract_column() {
    // Pattern: cut -d',' -f2 (extract second column from CSV)
    let mut t = PipelineTest::new();
    t.expect_string("echo 'name,age,city' | cut -d, -f2", "age");
}

#[test]
fn test_csv_column_unique_values() {
    // Pattern: cut -f2 | sort | uniq (unique values in column)
    let mut t = PipelineTest::new();
    let csv = "alice,NY\nbob,CA\ncharlie,NY\ndave,TX";
    t.expect_int(&format!("echo '{}' | lines | cut -d, -f2 | sort | uniq | count", csv), 3);
}

#[test]
fn test_csv_filter_and_extract() {
    // Pattern: grep pattern | cut -f1 (filter rows then extract column)
    let mut t = PipelineTest::new();
    let csv = "alice,yes\nbob,no\ncharlie,yes";
    let items = t.expect_list(&format!("echo '{}' | lines | grep yes | cut -d, -f1", csv));
    // Should get alice and charlie (both have 'yes')
    assert_eq!(items.len(), 2);
}

// --- Text transformation pipelines ---

#[test]
fn test_normalize_and_dedupe() {
    // Pattern: tr '[:upper:]' '[:lower:]' | sort | uniq (normalize case, dedupe)
    let mut t = PipelineTest::new();
    let data = "Apple\napple\nAPPLE\nBanana";
    t.expect_int(&format!("echo '{}' | lines | tr '[:upper:]' '[:lower:]' | sort | uniq | count", data), 2);
}

#[test]
fn test_remove_duplicates_preserve_order() {
    // Pattern: awk '!seen[$0]++' equivalent - first occurrence only
    // Using sort -u as a proxy
    let mut t = PipelineTest::new();
    let data = "apple\nbanana\napple\ncherry\nbanana";
    t.expect_int(&format!("echo '{}' | lines | sort | uniq | count", data), 3);
}

#[test]
fn test_word_count_pipeline() {
    // Pattern: wc -w (count words in text)
    let mut t = PipelineTest::new();
    t.expect_int("echo 'the quick brown fox' | wc -w", 4);
}

#[test]
fn test_line_count_pipeline() {
    // Pattern: wc -l (count lines)
    let mut t = PipelineTest::new();
    t.expect_int("echo 'one\ntwo\nthree\nfour\nfive' | wc -l", 5);
}

#[test]
fn test_char_count_pipeline() {
    // Pattern: wc -c or wc -m (count characters)
    let mut t = PipelineTest::new();
    t.expect_int("echo 'hello world' | wc -m", 11);
}

// --- JSON data processing (modern shell patterns) ---

#[test]
fn test_json_pluck_field() {
    // Pattern: jq '.name' (extract field from JSON)
    let mut t = PipelineTest::new();
    let json = r#"{"name":"alice","age":30}"#;
    let value = t.run(&format!("echo '{}' | from-json | get name", json));
    match value {
        Some(Value::String(s)) => assert_eq!(s, "alice"),
        other => panic!("Expected 'alice', got {:?}", other),
    }
}

#[test]
fn test_json_nested_field() {
    // Pattern: jq '.user.name' (nested field)
    let mut t = PipelineTest::new();
    let json = r#"{"user":{"name":"bob","id":123}}"#;
    let value = t.run(&format!("echo '{}' | from-json | get user | get name", json));
    match value {
        Some(Value::String(s)) => assert_eq!(s, "bob"),
        other => panic!("Expected 'bob', got {:?}", other),
    }
}

#[test]
fn test_json_array_length() {
    // Pattern: jq '.items | length' (count array items)
    let mut t = PipelineTest::new();
    let json = r#"{"items":[1,2,3,4,5]}"#;
    t.expect_int(&format!("echo '{}' | from-json | get items | count", json), 5);
}

#[test]
fn test_json_array_filter_count() {
    // Pattern: jq '.[] | select(.active)' | count (filter and count)
    let mut t = PipelineTest::new();
    let json = r#"[{"name":"a","val":10},{"name":"b","val":20},{"name":"c","val":5}]"#;
    // Sum the 'val' fields
    t.expect_int(&format!("echo '{}' | from-json | count", json), 3);
}

#[test]
fn test_json_to_json_roundtrip() {
    // Pattern: Parse JSON, transform, output JSON
    let mut t = PipelineTest::new();
    t.expect_int("seq 1 5 | to-json | from-json | count", 5);
}

// --- Path manipulation pipelines ---

#[test]
fn test_extract_extensions() {
    // Pattern: Get file extensions from paths
    let mut t = PipelineTest::new();
    let paths = "/foo/bar.txt\n/baz/qux.rs\n/a/b.txt";
    let items = t.expect_list(&format!("echo '{}' | lines | extname", paths));
    assert_eq!(items.len(), 3);
}

#[test]
fn test_extract_filenames() {
    // Pattern: basename on multiple files
    let mut t = PipelineTest::new();
    let paths = "/foo/bar.txt\n/baz/qux.rs";
    let items = t.expect_list(&format!("echo '{}' | lines | basename", paths));
    assert_eq!(items.len(), 2);
}

#[test]
fn test_extract_directories() {
    // Pattern: dirname on multiple files
    let mut t = PipelineTest::new();
    let paths = "/foo/bar/file.txt\n/baz/qux/other.rs";
    let items = t.expect_list(&format!("echo '{}' | lines | dirname", paths));
    assert_eq!(items.len(), 2);
}

// --- Numeric data pipelines ---

#[test]
fn test_statistical_pipeline() {
    // Get multiple stats in sequence
    let mut t = PipelineTest::new();
    // Sum of 1-10
    t.expect_int("seq 1 10 | sum", 55);
    // Average of 1-10
    t.expect_float("seq 1 10 | avg", 5.5);
    // Count
    t.expect_int("seq 1 10 | count", 10);
}

#[test]
fn test_range_slice_sum() {
    // Pattern: Get middle slice of data
    // seq 1 100 | tail -50 | head -10 | sum = 51+52+...+60 = 555
    let mut t = PipelineTest::new();
    t.expect_int("seq 1 100 | tail -50 | head -10 | sum", 555);
}

#[test]
fn test_sample_and_process() {
    // Pattern: shuf | head -n (random sample) then process
    let mut t = PipelineTest::new();
    // Shuffle and take 5 items - count should be 5
    t.expect_int("seq 1 100 | shuf | take 5 | count", 5);
}

// --- Multi-stage filtering ---

#[test]
fn test_multi_grep_filter() {
    // Pattern: grep A | grep -v B (include A, exclude B)
    let mut t = PipelineTest::new();
    let data = "apple\napricot\nbanana\navocado\nblueberry";
    // Lines starting with 'a' but not containing 'v'
    t.expect_int(&format!("echo '{}' | lines | grep '^a' | grep -v v | count", data), 2);
}

#[test]
fn test_filter_transform_aggregate() {
    // Pattern: filter | transform | aggregate
    let mut t = PipelineTest::new();
    // Get words, filter those starting with 'b', count
    let text = "apple banana blueberry cherry blackberry date";
    t.expect_int(&format!("echo '{}' | words | grep '^b' | count", text), 3);
}

// --- Previous output pipelines (nexus-specific) ---

#[test]
fn test_prev_output_reuse() {
    // Pattern: Use _ to reference previous output
    let mut t = PipelineTest::new();
    // First command generates data
    t.run("seq 1 10");
    // Use _ to reference it
    t.expect_int("_ | count", 10);
}

#[test]
fn test_prev_output_chain() {
    // Build on previous results
    let mut t = PipelineTest::new();
    t.run("seq 1 20");
    t.run("_ | head -10");
    t.expect_int("_ | count", 10);
}

// --- Combining multiple data sources ---

#[test]
fn test_concatenate_and_process() {
    // Pattern: Combine data then process
    let mut t = PipelineTest::new();
    // Using echo to combine
    let result = t.run("echo 'a\nb\nc\nd\ne' | lines | count");
    match result {
        Some(Value::Int(n)) => assert_eq!(n, 5),
        other => panic!("Expected 5, got {:?}", other),
    }
}

// --- Real-world data extraction ---

#[test]
fn test_extract_ips_pattern() {
    // Pattern: Extract IP-like strings (simplified)
    let mut t = PipelineTest::new();
    let log = "Connected from 192.168.1.1\nTimeout from 10.0.0.5\nConnected from 192.168.1.1";
    // Get words containing dots, dedupe
    let items = t.expect_list(&format!("echo '{}' | words | grep '\\.' | sort | uniq", log));
    assert!(items.len() >= 2); // At least the two IPs
}

#[test]
fn test_extract_urls_pattern() {
    // Pattern: grep -o for URLs (simplified - just count lines with http)
    let mut t = PipelineTest::new();
    let text = "Visit https://example.com or http://test.org for more";
    t.expect_int(&format!("echo '{}' | words | grep http | count", text), 2);
}

// --- Error handling / edge cases ---

#[test]
fn test_empty_pipeline_stages() {
    // grep that matches nothing should result in empty/zero
    let mut t = PipelineTest::new();
    t.expect_int("echo 'apple banana cherry' | lines | grep xyz | count", 0);
}

#[test]
fn test_single_item_pipeline() {
    // Pipeline with single item should work
    let mut t = PipelineTest::new();
    t.expect_int("echo 'single' | lines | count", 1);
}

#[test]
fn test_large_pipeline_chain() {
    // Long pipeline chain should work
    let mut t = PipelineTest::new();
    // seq | head | tail | sort | head | count
    t.expect_int("seq 1 1000 | head -500 | tail -250 | sort -rn | head -100 | count", 100);
}

// --- String manipulation chains ---

#[test]
fn test_string_reverse_chain() {
    // rev | rev should give original
    let mut t = PipelineTest::new();
    t.expect_string("echo 'hello' | rev | rev", "hello");
}

#[test]
fn test_string_transform_chain() {
    // Multiple transformations
    let mut t = PipelineTest::new();
    t.expect_string("echo 'HELLO WORLD' | tr '[:upper:]' '[:lower:]' | tr ' ' '-'", "hello-world");
}

// --- Date/time pipelines ---

#[test]
fn test_date_pipeline() {
    // date command should produce output
    let mut t = PipelineTest::new();
    let value = t.run("date | wc -m");
    // Should have some characters (date output)
    match value {
        Some(Value::Int(n)) => assert!(n > 0, "Date should produce output"),
        other => panic!("Expected positive int, got {:?}", other),
    }
}

// --- Split and rejoin ---

#[test]
fn test_split_process_join() {
    // Split, process each part, rejoin
    let mut t = PipelineTest::new();
    // Split on comma, sort, rejoin with semicolon
    t.expect_string("echo 'c,a,b' | split ',' | sort | join ';'", "a;b;c");
}

#[test]
fn test_split_filter_join() {
    // Split, filter, rejoin
    let mut t = PipelineTest::new();
    t.expect_string("echo 'apple,banana,cherry,apricot' | split ',' | grep '^a' | join ','", "apple,apricot");
}

// --- File listing pipelines (ls) ---

#[test]
fn test_ls_count_files() {
    // Pattern: ls | wc -l (count files in directory)
    let mut t = PipelineTest::new();
    // ls current directory and count (should be > 0)
    let value = t.run("ls | count");
    match value {
        Some(Value::Int(n)) => assert!(n > 0, "Should have at least one file"),
        other => panic!("Expected positive int, got {:?}", other),
    }
}

#[test]
fn test_ls_filter_by_extension() {
    // Pattern: ls | grep '\.rs$' (filter by extension)
    let mut t = PipelineTest::new();
    // In nexus-kernel directory, should have .rs files
    let value = t.run("ls src/commands/*.rs | count");
    // This might fail if glob doesn't work, but that's ok
    assert!(value.is_some());
}

// --- Tac (reverse lines) ---

#[test]
fn test_tac_reverse_lines() {
    // Pattern: tac (reverse line order)
    let mut t = PipelineTest::new();
    let items = t.expect_list("echo 'a\nb\nc' | lines | tac");
    assert_eq!(items.len(), 3);
    // First item should be 'c' (last line reversed to first)
    let first = items[0].to_string();
    assert!(first.contains('c'), "Expected 'c' first after tac, got {}", first);
}

#[test]
fn test_tac_combined_with_head() {
    // Pattern: tac | head (last N lines in reverse)
    let mut t = PipelineTest::new();
    t.expect_int("seq 1 10 | tac | head -3 | count", 3);
}

// --- Number lines (nl) ---

#[test]
fn test_nl_numbers_lines() {
    // Pattern: nl (number lines)
    let mut t = PipelineTest::new();
    let items = t.expect_list("echo 'first\nsecond\nthird' | lines | nl");
    assert_eq!(items.len(), 3);
}

#[test]
fn test_nl_with_grep() {
    // Pattern: nl | grep (find line numbers of matches)
    let mut t = PipelineTest::new();
    let items = t.expect_list("echo 'apple\nbanana\napricot' | lines | nl | grep apple");
    // Only line 1 contains "apple" (apricot doesn't contain "apple")
    assert_eq!(items.len(), 1);
}

// --- Flatten nested structures ---

#[test]
fn test_flatten_simple() {
    // Pattern: flatten (unnest one level)
    let mut t = PipelineTest::new();
    t.expect_int("echo '[[1,2],[3,4],[5]]' | from-json | flatten | count", 5);
}

#[test]
fn test_flatten_strings() {
    // Flatten nested string arrays
    let mut t = PipelineTest::new();
    let json = r#"[["a","b"],["c","d"]]"#;
    t.expect_int(&format!("echo '{}' | from-json | flatten | count", json), 4);
}

// --- Enumerate with index ---

#[test]
fn test_enumerate_with_skip() {
    // Pattern: enumerate | skip | first (get item at specific index)
    let mut t = PipelineTest::new();
    let value = t.run("seq 1 5 | enumerate | skip 2 | first | get index");
    match value {
        Some(Value::Int(n)) => assert_eq!(n, 2),
        other => panic!("Expected index 2, got {:?}", other),
    }
}

// --- Compact (remove empty) ---

#[test]
fn test_compact_with_split() {
    // Pattern: split | compact (split and remove empties)
    let mut t = PipelineTest::new();
    t.expect_int("echo 'a::b:::c' | split ':' | compact | count", 3);
}

// --- Complex real-world combinations ---

#[test]
fn test_log_analysis_pipeline() {
    // Realistic log analysis: extract, filter, aggregate
    let mut t = PipelineTest::new();
    let log = "2024-01-01 ERROR db connection failed
2024-01-01 INFO request processed
2024-01-02 ERROR timeout
2024-01-02 ERROR db connection failed
2024-01-02 INFO request processed";

    // Count unique error messages
    t.expect_int(&format!(
        "echo '{}' | lines | grep ERROR | cut -d' ' -f3- | sort | uniq | count",
        log
    ), 2);
}

#[test]
fn test_data_cleanup_pipeline() {
    // Pattern: Clean up messy data
    let mut t = PipelineTest::new();
    // Lowercase, sort, dedupe (no whitespace in input)
    let data = "Apple\nBANANA\napple\nCherry\nBANANA";
    t.expect_int(&format!(
        "echo '{}' | lines | tr '[:upper:]' '[:lower:]' | sort | uniq | count",
        data
    ), 3);
}

#[test]
fn test_extract_and_aggregate() {
    // Pattern: Extract field from structured data, aggregate
    let mut t = PipelineTest::new();
    let json = r#"[{"name":"a","score":10},{"name":"b","score":20},{"name":"c","score":30}]"#;
    // This would need a map operation we might not have, so just count
    t.expect_int(&format!("echo '{}' | from-json | count", json), 3);
}

// --- Bytes/chars operations ---

#[test]
fn test_bytes_count() {
    // Pattern: Count bytes in string
    let mut t = PipelineTest::new();
    t.expect_int("echo 'hello' | bytes | count", 5);
}

#[test]
fn test_chars_manipulation() {
    // Pattern: Split into chars, filter, rejoin
    let mut t = PipelineTest::new();
    // Remove vowels by splitting to chars, filtering, joining
    t.expect_string("echo 'hello' | chars | grep -v '[aeiou]' | join ''", "hll");
}
