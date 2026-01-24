//! Terminal parser - feeds bytes through alacritty_terminal to update grid state.

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, Term, test::TermSize};
use alacritty_terminal::vte::ansi::Processor;

use crate::cell::{Cell, CellFlags, Color};
use crate::grid::TerminalGrid;

/// A terminal parser that maintains grid state.
pub struct TerminalParser {
    /// The alacritty terminal state.
    term: Term<EventProxy>,
    /// ANSI parser/processor.
    processor: Processor,
}

impl std::fmt::Debug for TerminalParser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalParser")
            .field("size", &self.size())
            .finish_non_exhaustive()
    }
}

/// Dummy event listener for headless operation.
struct EventProxy;

impl EventListener for EventProxy {
    fn send_event(&self, _event: Event) {
        // We don't need to handle events in headless mode
    }
}

/// Default scrollback history (10k lines).
const SCROLLBACK_LINES: usize = 10_000;

impl TerminalParser {
    /// Create a new parser with the given dimensions.
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = TermSize::new(cols as usize, rows as usize);
        // Configure with explicit scrollback history
        let config = Config {
            scrolling_history: SCROLLBACK_LINES,
            ..Config::default()
        };
        let term = Term::new(config, &size, EventProxy);
        let processor = Processor::new();

        Self { term, processor }
    }

    /// Feed bytes into the parser.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    /// Extract the current grid state (viewport only).
    pub fn grid(&self) -> TerminalGrid {
        let term_content = self.term.renderable_content();
        let cols = self.term.columns();
        let rows = self.term.screen_lines();

        let mut grid = TerminalGrid::new(cols as u16, rows as u16);

        // Copy cells from alacritty's grid
        for indexed_cell in term_content.display_iter {
            let col = indexed_cell.point.column.0 as u16;
            let row = indexed_cell.point.line.0 as u16;

            let cell = Cell {
                c: indexed_cell.c,
                fg: Color::from(indexed_cell.fg),
                bg: Color::from(indexed_cell.bg),
                flags: CellFlags::from(indexed_cell.flags),
            };

            grid.set(col, row, cell);
        }

        // Set cursor
        let cursor = term_content.cursor;
        grid.set_cursor(cursor.point.column.0 as u16, cursor.point.line.0 as u16);

        grid
    }

    /// Extract ALL content including scrollback history.
    /// Returns a grid sized to fit all content (scrollback + visible).
    /// Used for finished blocks that need to show complete output.
    pub fn grid_with_scrollback(&self) -> TerminalGrid {
        let grid = self.term.grid();
        let cols = self.term.columns();
        let screen_lines = self.term.screen_lines();
        let history_lines = grid.history_size();
        let total_lines = screen_lines + history_lines;

        // Calculate actual content height to avoid huge empty grids
        let content_rows = self.content_height();
        let total_to_render = content_rows.min(total_lines).max(1);

        // Create a grid sized to actual content
        let mut result = TerminalGrid::new(cols as u16, total_to_render as u16);

        // Iterate from top of scrollback to bottom of content
        // alacritty line indices: negative = scrollback, 0+ = visible screen
        // Line(-history_lines) is the oldest line in scrollback
        // Line(screen_lines - 1) is the bottom of the visible screen
        let start_line = -(history_lines as i32);

        for line_idx in 0..total_to_render {
            let term_line = Line(start_line + line_idx as i32);
            let row = &grid[term_line];
            for col_idx in 0..cols {
                let cell = &row[Column(col_idx)];
                let our_cell = Cell {
                    c: cell.c,
                    fg: Color::from(cell.fg),
                    bg: Color::from(cell.bg),
                    flags: CellFlags::from(cell.flags),
                };
                result.set(col_idx as u16, line_idx as u16, our_cell);
            }
        }

        // Position cursor relative to full content
        let cursor_point = grid.cursor.point;
        let cursor_row = (cursor_point.line.0 + history_lines as i32) as u16;
        result.set_cursor(cursor_point.column.0 as u16, cursor_row.min(total_to_render.saturating_sub(1) as u16));

        result
    }

    /// Get the number of lines in scrollback history.
    pub fn scrollback_lines(&self) -> usize {
        self.term.grid().history_size()
    }

    /// Get total lines (screen + scrollback).
    pub fn total_lines(&self) -> usize {
        self.term.screen_lines() + self.term.grid().history_size()
    }

    /// Resize the terminal.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = TermSize::new(cols as usize, rows as usize);
        self.term.resize(size);
    }

    /// Get terminal dimensions.
    pub fn size(&self) -> (u16, u16) {
        (self.term.columns() as u16, self.term.screen_lines() as u16)
    }

    /// Check if in alternate screen mode (for TUI apps).
    pub fn is_alternate_screen(&self) -> bool {
        self.term.mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN)
    }

    /// Calculate the number of rows that have actual content.
    /// Includes both visible screen AND scrollback history.
    /// Used for sizing finished blocks to show all their content.
    pub fn content_height(&self) -> usize {
        let grid = self.term.grid();
        let cols = self.term.columns();
        let screen_lines = self.term.screen_lines();
        let history_lines = grid.history_size();
        let total_lines = screen_lines + history_lines;

        if total_lines == 0 {
            return 1;
        }

        // Scan from bottom to top to find last row with content
        let start_line = -(history_lines as i32);

        for line_idx in (0..total_lines).rev() {
            let term_line = Line(start_line + line_idx as i32);
            let row = &grid[term_line];

            // Check if this row has any non-empty content
            for col_idx in 0..cols {
                let cell = &row[Column(col_idx)];
                if cell.c != ' ' && cell.c != '\0' {
                    return line_idx + 1;
                }
            }
        }

        1 // At least 1 row
    }

    /// Clear the terminal.
    pub fn clear(&mut self) {
        // Send clear screen escape sequence
        self.feed(b"\x1b[2J\x1b[H");
    }
}

impl Default for TerminalParser {
    fn default() -> Self {
        Self::new(crate::DEFAULT_COLS, crate::DEFAULT_ROWS)
    }
}
