//! Word/line boundary detection for multi-click selection.
//!
//! Pure functions operating on owned content snapshots. `SnapContent` avoids
//! lifetime issues with `Rc<TerminalGrid>` by owning the character data.

use strata::content_address::ContentAddress;

/// Owned content snapshot for snap calculations.
pub(crate) enum SnapContent {
    /// Terminal grid: one char per cell, fixed column count.
    Grid { chars: Vec<char>, cols: usize },
    /// Multi-line text (agent blocks, shell headers).
    Text { lines: Vec<String> },
}

// =========================================================================
// Word-char predicates
// =========================================================================

/// Terminal word characters — path-friendly, excludes `:`.
fn is_word_char_terminal(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '~' | '@')
}

/// Agent/text word characters — standard programmer word.
fn is_word_char_text(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

// =========================================================================
// URL detection
// =========================================================================

/// Characters allowed in URLs (ASCII graphic minus a few).
fn is_url_char(c: char) -> bool {
    c.is_ascii_graphic() && !matches!(c, '<' | '>' | '{' | '}' | '|' | '\\' | '^' | '`')
}

/// If the word region is part of a URL, expand to the full URL.
///
/// Strategy: expand the word region outward through all URL chars first,
/// then check if the expanded region contains a URL scheme. This handles
/// double-clicking any word within a URL (e.g., "example" in "https://example.com").
///
/// After expansion:
/// - Strips trailing punctuation greedily (`.` `,` `;` `:` `!` `?` `)`)
/// - Balances parentheses: if `)` count > `(` count, remove trailing `)` until balanced
fn try_expand_url(chars: &[char], start: usize, end: usize) -> (usize, usize) {
    if start >= end || end > chars.len() {
        return (start, end);
    }

    // First, expand outward to the full URL-char region
    let mut url_start = start;
    while url_start > 0 && is_url_char(chars[url_start - 1]) {
        url_start -= 1;
    }

    let mut url_end = end;
    while url_end < chars.len() && is_url_char(chars[url_end]) {
        url_end += 1;
    }

    // Check if the expanded region actually contains a URL scheme
    let expanded: String = chars[url_start..url_end].iter().collect();
    let has_scheme = expanded.contains("://")
        || expanded.starts_with("http")
        || expanded.starts_with("https")
        || expanded.starts_with("ftp")
        || expanded.starts_with("ssh");

    if !has_scheme {
        return (start, end);
    }

    // Strip trailing punctuation greedily
    while url_end > url_start && matches!(chars[url_end - 1], '.' | ',' | ';' | ':' | '!' | '?') {
        url_end -= 1;
    }

    // Strip leading opening parens/brackets that are likely not part of the URL
    while url_start < url_end && matches!(chars[url_start], '(' | '[') {
        url_start += 1;
    }

    // Balance parentheses: strip trailing `)` if unmatched
    loop {
        if url_end <= url_start {
            break;
        }
        if chars[url_end - 1] != ')' {
            break;
        }
        let region = &chars[url_start..url_end];
        let open_count = region.iter().filter(|&&c| c == '(').count();
        let close_count = region.iter().filter(|&&c| c == ')').count();
        if close_count <= open_count {
            break;
        }
        url_end -= 1;
    }

    (url_start, url_end)
}

// =========================================================================
// Public API
// =========================================================================

/// Snap an address to word boundaries. Returns `(word_start, word_end)`.
pub(crate) fn snap_word(
    addr: &ContentAddress,
    content: &SnapContent,
) -> (ContentAddress, ContentAddress) {
    match content {
        SnapContent::Grid { chars, cols } => {
            let offset = addr.content_offset;
            if offset >= chars.len() {
                return (addr.clone(), addr.clone());
            }
            let c = chars[offset];
            let (mut start, mut end) = if is_word_char_terminal(c) {
                let mut s = offset;
                // Scan left within the same row
                let row_start = (offset / cols) * cols;
                while s > row_start && is_word_char_terminal(chars[s - 1]) {
                    s -= 1;
                }
                let row_end = row_start + cols;
                let mut e = offset + 1;
                while e < row_end && e < chars.len() && is_word_char_terminal(chars[e]) {
                    e += 1;
                }
                (s, e)
            } else {
                // Non-word char: select single char
                (offset, offset + 1)
            };

            // Try URL expansion for terminal content
            let (us, ue) = try_expand_url(chars, start, end);
            start = us;
            end = ue;

            (
                ContentAddress::new(addr.source_id, addr.item_index, start),
                ContentAddress::new(addr.source_id, addr.item_index, end),
            )
        }
        SnapContent::Text { lines } => {
            let line_idx = addr.item_index;
            if line_idx >= lines.len() {
                return (addr.clone(), addr.clone());
            }
            let line_chars: Vec<char> = lines[line_idx].chars().collect();
            let offset = addr.content_offset;
            if offset >= line_chars.len() {
                return (addr.clone(), addr.clone());
            }
            let c = line_chars[offset];
            let (start, end) = if is_word_char_text(c) {
                let mut s = offset;
                while s > 0 && is_word_char_text(line_chars[s - 1]) {
                    s -= 1;
                }
                let mut e = offset + 1;
                while e < line_chars.len() && is_word_char_text(line_chars[e]) {
                    e += 1;
                }
                (s, e)
            } else {
                (offset, offset + 1)
            };
            (
                ContentAddress::new(addr.source_id, line_idx, start),
                ContentAddress::new(addr.source_id, line_idx, end),
            )
        }
    }
}

/// Snap an address to line boundaries. Returns `(line_start, line_end)`.
pub(crate) fn snap_line(
    addr: &ContentAddress,
    content: &SnapContent,
) -> (ContentAddress, ContentAddress) {
    match content {
        SnapContent::Grid { chars, cols } => {
            let cols = *cols;
            if cols == 0 {
                return (addr.clone(), addr.clone());
            }
            let row = addr.content_offset / cols;
            let line_start = row * cols;
            let line_end = ((row + 1) * cols).min(chars.len());
            (
                ContentAddress::new(addr.source_id, addr.item_index, line_start),
                ContentAddress::new(addr.source_id, addr.item_index, line_end),
            )
        }
        SnapContent::Text { lines } => {
            let line_idx = addr.item_index;
            let char_count = lines
                .get(line_idx)
                .map(|l| l.chars().count())
                .unwrap_or(0);
            (
                ContentAddress::new(addr.source_id, line_idx, 0),
                ContentAddress::new(addr.source_id, line_idx, char_count),
            )
        }
    }
}

/// Extract text between two addresses in the same source.
pub(crate) fn extract_snap_text(
    start: &ContentAddress,
    end: &ContentAddress,
    content: &SnapContent,
) -> String {
    match content {
        SnapContent::Grid { chars, .. } => {
            let s = start.content_offset.min(chars.len());
            let e = end.content_offset.min(chars.len());
            if s >= e {
                return String::new();
            }
            chars[s..e].iter().collect()
        }
        SnapContent::Text { lines } => {
            if start.item_index != end.item_index {
                // Cross-line: extract from start offset to end of start line,
                // then full intermediate lines, then start of end line to end offset.
                let mut parts = Vec::new();
                for idx in start.item_index..=end.item_index.min(lines.len().saturating_sub(1)) {
                    let line_chars: Vec<char> = lines[idx].chars().collect();
                    let from = if idx == start.item_index {
                        start.content_offset.min(line_chars.len())
                    } else {
                        0
                    };
                    let to = if idx == end.item_index {
                        end.content_offset.min(line_chars.len())
                    } else {
                        line_chars.len()
                    };
                    if from <= to {
                        parts.push(line_chars[from..to].iter().collect::<String>());
                    }
                }
                parts.join("\n")
            } else {
                let line_chars: Vec<char> = lines
                    .get(start.item_index)
                    .map(|l| l.chars().collect())
                    .unwrap_or_default();
                let s = start.content_offset.min(line_chars.len());
                let e = end.content_offset.min(line_chars.len());
                if s >= e {
                    return String::new();
                }
                line_chars[s..e].iter().collect()
            }
        }
    }
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use strata::content_address::SourceId;

    fn grid_content(text: &str, cols: usize) -> SnapContent {
        let mut chars: Vec<char> = text.chars().collect();
        // Pad to full rows
        let remainder = chars.len() % cols;
        if remainder != 0 {
            chars.resize(chars.len() + cols - remainder, ' ');
        }
        SnapContent::Grid { chars, cols }
    }

    fn text_content(lines: &[&str]) -> SnapContent {
        SnapContent::Text {
            lines: lines.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn addr(item: usize, offset: usize) -> ContentAddress {
        ContentAddress::new(SourceId::from_raw(1), item, offset)
    }

    // ============ Word snap (Grid) ============

    #[test]
    fn word_snap_grid_simple_word() {
        let content = grid_content("hello world", 80);
        let (start, end) = snap_word(&addr(0, 2), &content); // 'l' in "hello"
        assert_eq!(start.content_offset, 0);
        assert_eq!(end.content_offset, 5);
    }

    #[test]
    fn word_snap_grid_path() {
        let content = grid_content("/usr/local/bin", 80);
        let (start, end) = snap_word(&addr(0, 5), &content); // 'l' in "local"
        assert_eq!(start.content_offset, 0);
        assert_eq!(end.content_offset, 14); // whole path
    }

    #[test]
    fn word_snap_grid_non_word_char() {
        let content = grid_content("a + b", 80);
        let (start, end) = snap_word(&addr(0, 2), &content); // '+'
        assert_eq!(start.content_offset, 2);
        assert_eq!(end.content_offset, 3);
    }

    #[test]
    fn word_snap_grid_url() {
        let content = grid_content("see https://example.com/path done", 80);
        let (start, end) = snap_word(&addr(0, 10), &content); // in URL
        assert_eq!(start.content_offset, 4);
        assert_eq!(end.content_offset, 28); // "https://example.com/path"
    }

    #[test]
    fn word_snap_grid_url_trailing_punct() {
        let content = grid_content("visit https://example.com. ok", 80);
        let (start, end) = snap_word(&addr(0, 12), &content);
        assert_eq!(start.content_offset, 6);
        assert_eq!(end.content_offset, 25); // strips trailing '.'
    }

    #[test]
    fn word_snap_grid_url_balanced_parens() {
        let content = grid_content("(https://en.wikipedia.org/wiki/Test_(thing))", 80);
        let (start, end) = snap_word(&addr(0, 10), &content);
        let text = extract_snap_text(&start, &end, &content);
        // Inner parens are balanced, outer trailing ')' is stripped
        assert_eq!(text, "https://en.wikipedia.org/wiki/Test_(thing)");
    }

    // ============ Word snap (Text) ============

    #[test]
    fn word_snap_text_simple() {
        let content = text_content(&["hello world", "second line"]);
        let (start, end) = snap_word(&addr(0, 7), &content); // 'o' in "world"
        assert_eq!(start.content_offset, 6);
        assert_eq!(end.content_offset, 11);
    }

    #[test]
    fn word_snap_text_underscore() {
        let content = text_content(&["my_variable_name = 42"]);
        let (start, end) = snap_word(&addr(0, 5), &content);
        assert_eq!(start.content_offset, 0);
        assert_eq!(end.content_offset, 16); // "my_variable_name"
    }

    #[test]
    fn word_snap_text_no_hyphen() {
        // In text mode, hyphen is NOT a word char
        let content = text_content(&["foo-bar"]);
        let (start, end) = snap_word(&addr(0, 1), &content);
        assert_eq!(start.content_offset, 0);
        assert_eq!(end.content_offset, 3); // just "foo"
    }

    // ============ Line snap (Grid) ============

    #[test]
    fn line_snap_grid() {
        let content = grid_content("aaabbbccc", 3); // 3 rows of 3
        let (start, end) = snap_line(&addr(0, 4), &content); // middle of row 1
        assert_eq!(start.content_offset, 3);
        assert_eq!(end.content_offset, 6);
    }

    // ============ Line snap (Text) ============

    #[test]
    fn line_snap_text() {
        let content = text_content(&["hello world", "second line"]);
        let (start, end) = snap_line(&addr(1, 3), &content);
        assert_eq!(start.item_index, 1);
        assert_eq!(start.content_offset, 0);
        assert_eq!(end.item_index, 1);
        assert_eq!(end.content_offset, 11); // "second line".len()
    }

    // ============ extract_snap_text ============

    #[test]
    fn extract_grid_text() {
        let content = grid_content("hello world", 80);
        let text = extract_snap_text(&addr(0, 0), &addr(0, 5), &content);
        assert_eq!(text, "hello");
    }

    #[test]
    fn extract_text_single_line() {
        let content = text_content(&["hello world"]);
        let text = extract_snap_text(&addr(0, 6), &addr(0, 11), &content);
        assert_eq!(text, "world");
    }

    #[test]
    fn extract_text_cross_line() {
        let content = text_content(&["hello", "world"]);
        let text = extract_snap_text(&addr(0, 3), &addr(1, 3), &content);
        assert_eq!(text, "lo\nwor");
    }
}
