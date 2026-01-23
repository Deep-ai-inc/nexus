//! Terminal parser - feeds bytes through alacritty_terminal to update grid state.

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
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

impl TerminalParser {
    /// Create a new parser with the given dimensions.
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = TermSize::new(cols as usize, rows as usize);
        let config = Config::default();
        let term = Term::new(config, &size, EventProxy);
        let processor = Processor::new();

        Self { term, processor }
    }

    /// Feed bytes into the parser.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
    }

    /// Extract the current grid state.
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
