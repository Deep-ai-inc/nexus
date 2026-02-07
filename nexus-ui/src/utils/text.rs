//! Shared text utilities.

use std::sync::OnceLock;

/// Returns the user's home directory, cached after the first call.
pub fn home_dir() -> &'static str {
    static HOME: OnceLock<String> = OnceLock::new();
    HOME.get_or_init(|| std::env::var("HOME").unwrap_or_default())
}

/// Replace the home directory prefix with `~` for display.
pub fn display_path(path: &str) -> String {
    let home = home_dir();
    if !home.is_empty() && path.starts_with(home) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    }
}

/// Truncate a string to `max_chars` characters, adding an ellipsis if truncated.
///
/// Unlike byte-slicing (`&s[..n]`), this is safe for multi-byte UTF-8 strings
/// because it operates on char boundaries.
pub fn truncate_str(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let end = s
            .char_indices()
            .nth(max_chars)
            .map(|(i, _)| i)
            .unwrap_or(s.len());
        format!("{}\u{2026}", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_string_unchanged() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn exact_length_unchanged() {
        assert_eq!(truncate_str("hello", 5), "hello");
    }

    #[test]
    fn truncates_with_ellipsis() {
        assert_eq!(truncate_str("hello world", 5), "hello\u{2026}");
    }

    #[test]
    fn empty_string() {
        assert_eq!(truncate_str("", 5), "");
    }

    #[test]
    fn multibyte_chars_safe() {
        // Japanese characters are 3 bytes each in UTF-8
        let s = "あいうえお"; // 5 chars, 15 bytes
        assert_eq!(truncate_str(s, 3), "あいう\u{2026}");
    }

    #[test]
    fn emoji_safe() {
        let s = "😀😁😂😃😄";
        assert_eq!(truncate_str(s, 2), "😀😁\u{2026}");
    }

    #[test]
    fn zero_max() {
        assert_eq!(truncate_str("hello", 0), "\u{2026}");
    }
}
