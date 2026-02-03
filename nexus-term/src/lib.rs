//! Nexus Term - Headless terminal state management.
//!
//! This crate wraps alacritty_terminal to parse ANSI escape sequences
//! and maintain a terminal grid state, without any rendering.

mod grid;
mod parser;
mod cell;

pub use grid::TerminalGrid;
pub use parser::TerminalParser;
pub use cell::{Cell, CellFlags, Color, UnderlineStyle};

/// Default terminal dimensions.
pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resize_reflow_preserves_content() {
        // Start with 80 columns
        let mut parser = TerminalParser::new(80, 24);

        // Write text that fits in 80 cols but would wrap at 40
        let long_line = "A".repeat(60);
        parser.feed(format!("{}\r\n", long_line).as_bytes());
        parser.feed(b"Short line\r\n");

        // Verify initial state
        let grid = parser.grid();
        assert_eq!(grid.cols(), 80);

        // Resize to 40 columns - should cause reflow
        parser.resize(40, 24);

        // Content should now span more rows due to wrapping
        let grid = parser.grid();
        assert_eq!(grid.cols(), 40);

        // Verify content is still present after resize
        let text = grid.to_string();
        assert!(text.contains("A"), "Content should be preserved after resize");
        assert!(text.contains("Short line"), "Short line should be preserved");

        // Resize back to 80 - content should still be there
        parser.resize(80, 24);

        let grid = parser.grid();
        assert_eq!(grid.cols(), 80);

        let text = grid.to_string();
        assert!(text.contains("A"), "Content should be preserved after resize back");
    }

    #[test]
    fn test_resize_width_only() {
        let mut parser = TerminalParser::new(120, 24);
        parser.feed(b"Hello World\r\n");

        // Resize width only
        parser.resize(80, 24);

        let grid = parser.grid();
        assert_eq!(grid.cols(), 80);

        // Content should still be there
        let text = grid.to_string();
        assert!(text.contains("Hello World"));
    }

    #[test]
    fn test_content_height_calculation() {
        let mut parser = TerminalParser::new(80, 24);

        // Empty parser should have minimum height
        let height = parser.content_height();
        assert!(height >= 1, "Empty parser should have at least 1 row");

        // Add some lines
        parser.feed(b"Line 1\r\n");
        parser.feed(b"Line 2\r\n");
        parser.feed(b"Line 3\r\n");

        // Height should increase with content
        let height = parser.content_height();
        assert!(height >= 3, "Should have at least 3 content rows, got {}", height);
    }

    #[test]
    fn test_parser_resize_updates_dimensions() {
        let mut parser = TerminalParser::new(80, 24);
        assert_eq!(parser.size(), (80, 24));

        parser.resize(120, 40);
        assert_eq!(parser.size(), (120, 40));

        parser.resize(40, 10);
        assert_eq!(parser.size(), (40, 10));
    }

    #[test]
    fn test_grid_with_scrollback_preserves_content() {
        let mut parser = TerminalParser::new(80, 10); // Small viewport

        // Write more lines than viewport can hold
        for i in 1..=20 {
            parser.feed(format!("Line {}\r\n", i).as_bytes());
        }

        // Some content should be in scrollback now
        let scrollback = parser.scrollback_lines();
        assert!(scrollback > 0, "Expected content in scrollback");

        // grid_with_scrollback should include all content
        let full_grid = parser.grid_with_scrollback();
        let text = full_grid.to_string();

        // Should contain all lines including those in scrollback
        assert!(text.contains("Line 1"), "Line 1 should be in grid_with_scrollback");
        assert!(text.contains("Line 10"), "Line 10 should be in grid_with_scrollback");
        assert!(text.contains("Line 20"), "Line 20 should be in grid_with_scrollback");

        // Regular grid() should only have recent content
        let viewport_grid = parser.grid();
        let viewport_text = viewport_grid.to_string();

        // Line 1 may or may not be visible in viewport depending on scrollback
        // but Line 20 should definitely be there
        assert!(viewport_text.contains("Line 20"), "Line 20 should be visible");
    }

    #[test]
    fn test_scrollback_survives_resize() {
        let mut parser = TerminalParser::new(80, 10);

        // Write content
        for i in 1..=15 {
            parser.feed(format!("Line {}\r\n", i).as_bytes());
        }

        // Resize to fewer rows (content goes to scrollback)
        parser.resize(80, 5);

        // grid_with_scrollback should still have all content
        let full_grid = parser.grid_with_scrollback();
        let text = full_grid.to_string();

        assert!(text.contains("Line 1"), "Line 1 should survive resize in scrollback");
        assert!(text.contains("Line 15"), "Line 15 should survive resize");
    }

    #[test]
    fn test_word_wrap_reflow() {
        // Simulate the "words disappear" scenario
        let mut parser = TerminalParser::new(80, 24);

        // Write a sentence that will need to wrap when narrowed
        parser.feed(b"The quick brown fox jumps over the lazy dog near the river\r\n");

        // Verify initial state
        let grid = parser.grid_with_scrollback();
        let text = grid.to_string();
        assert!(text.contains("quick"), "Initial: should have 'quick'");
        assert!(text.contains("river"), "Initial: should have 'river'");

        // Resize to narrow - words should reflow
        parser.resize(30, 24);

        let grid = parser.grid_with_scrollback();
        let text = grid.to_string();

        // All words should still be present after reflow
        assert!(text.contains("The"), "After narrow: should have 'The'");
        assert!(text.contains("quick"), "After narrow: should have 'quick'");
        assert!(text.contains("brown"), "After narrow: should have 'brown'");
        assert!(text.contains("fox"), "After narrow: should have 'fox'");
        assert!(text.contains("jumps"), "After narrow: should have 'jumps'");
        assert!(text.contains("over"), "After narrow: should have 'over'");
        assert!(text.contains("the"), "After narrow: should have 'the'");
        assert!(text.contains("lazy"), "After narrow: should have 'lazy'");
        assert!(text.contains("dog"), "After narrow: should have 'dog'");
        assert!(text.contains("river"), "After narrow: should have 'river'");

        // Resize back to wide
        parser.resize(80, 24);

        let grid = parser.grid_with_scrollback();
        let text = grid.to_string();

        // All words should still be present
        assert!(text.contains("quick"), "After widen: should have 'quick'");
        assert!(text.contains("river"), "After widen: should have 'river'");
    }

    #[test]
    fn test_columns_only_resize_preserves_content() {
        // Test the exact scenario: resize columns but keep rows
        let mut parser = TerminalParser::new(80, 24);

        parser.feed(b"Hello World this is a test of column resizing behavior\r\n");
        parser.feed(b"Second line of content here\r\n");

        // Get original content
        let _original = parser.grid_with_scrollback().to_string();

        // Resize columns only (what we do for finished blocks)
        let (_, rows) = parser.size();
        parser.resize(40, rows);

        let after_shrink = parser.grid_with_scrollback().to_string();

        // All content should still be accessible
        assert!(after_shrink.contains("Hello"), "Should have 'Hello' after shrink");
        assert!(after_shrink.contains("behavior"), "Should have 'behavior' after shrink");
        assert!(after_shrink.contains("Second"), "Should have 'Second' after shrink");

        // Resize back
        parser.resize(80, rows);

        let after_expand = parser.grid_with_scrollback().to_string();
        assert!(after_expand.contains("Hello"), "Should have 'Hello' after expand");
        assert!(after_expand.contains("behavior"), "Should have 'behavior' after expand");
    }
}
