//! Shadow terminal parser — a `Send`-safe variant for agent-side state tracking.
//!
//! Unlike `TerminalParser`, this does not use `Rc`/`RefCell` caching, making
//! it safe to share across tokio tasks via `Arc<std::sync::Mutex<>>`.

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::{Config, Term, TermMode, test::TermSize};
use alacritty_terminal::vte::ansi::Processor;

use crate::cell::{Cell, CellFlags, Color};
use crate::grid::{CursorShape, TerminalGrid};

/// A Send-safe terminal parser for agent-side shadow tracking.
///
/// Feeds PTY output bytes to maintain an accurate terminal grid state.
/// On reconnect, the grid can be extracted and sent to the UI as a snapshot.
pub struct ShadowParser {
    term: Term<ShadowEventProxy>,
    processor: Processor,
    title_slot: Arc<Mutex<Option<String>>>,
}

// SAFETY: Term<ShadowEventProxy> and Processor are Send (no Rc/RefCell).
// The only shared state is Arc<Mutex<>> which is Send.
unsafe impl Send for ShadowParser {}

struct ShadowEventProxy {
    title_slot: Arc<Mutex<Option<String>>>,
}

impl EventListener for ShadowEventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::Title(title) => {
                *self.title_slot.lock().unwrap() = Some(title);
            }
            Event::ResetTitle => {
                *self.title_slot.lock().unwrap() = None;
            }
            _ => {}
        }
    }
}

/// Scrollback history for shadow parser. Generous enough to reconstruct a
/// meaningful scroll buffer on reconnect, but bounded to limit agent memory.
const SHADOW_SCROLLBACK: usize = 10_000;

impl ShadowParser {
    /// Create a new shadow parser with the given dimensions.
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = TermSize::new(cols as usize, rows as usize);
        let config = Config {
            scrolling_history: SHADOW_SCROLLBACK,
            ..Config::default()
        };
        let title_slot = Arc::new(Mutex::new(None));
        let proxy = ShadowEventProxy {
            title_slot: title_slot.clone(),
        };
        let term = Term::new(config, &size, proxy);
        let processor = Processor::new();

        Self {
            term,
            processor,
            title_slot,
        }
    }

    /// Feed bytes from PTY output.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
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

    /// Extract the current viewport as a TerminalGrid.
    pub fn extract_grid(&self) -> TerminalGrid {
        let term_content = self.term.renderable_content();
        let cols = self.term.columns();
        let rows = self.term.screen_lines();

        let mut grid = TerminalGrid::new(cols as u16, rows as u16);

        for indexed_cell in term_content.display_iter {
            let col = indexed_cell.point.column.0 as u16;
            let row = indexed_cell.point.line.0 as u16;

            let zerowidth = indexed_cell
                .zerowidth()
                .filter(|zw| !zw.is_empty())
                .map(|zw| zw.to_vec().into_boxed_slice());

            let cell = Cell {
                c: indexed_cell.c,
                fg: Color::from(indexed_cell.fg),
                bg: Color::from(indexed_cell.bg),
                flags: CellFlags::from(indexed_cell.flags),
                zerowidth,
            };

            grid.set(col, row, cell);
        }

        let cursor = term_content.cursor;
        grid.set_cursor(
            cursor.point.column.0 as u16,
            cursor.point.line.0 as u16,
        );
        let shape = match cursor.shape {
            alacritty_terminal::vte::ansi::CursorShape::Block => CursorShape::Block,
            alacritty_terminal::vte::ansi::CursorShape::HollowBlock => CursorShape::HollowBlock,
            alacritty_terminal::vte::ansi::CursorShape::Beam => CursorShape::Beam,
            alacritty_terminal::vte::ansi::CursorShape::Underline => CursorShape::Underline,
            alacritty_terminal::vte::ansi::CursorShape::Hidden => CursorShape::Hidden,
        };
        grid.set_cursor_shape(shape);

        grid
    }

    /// Check if in alternate screen mode.
    pub fn is_alternate_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// Application Cursor Keys mode (DECCKM).
    pub fn app_cursor(&self) -> bool {
        self.term.mode().contains(TermMode::APP_CURSOR)
    }

    /// Bracketed paste mode.
    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(TermMode::BRACKETED_PASTE)
    }

    /// Extract scrollback history as a flat `Vec<Cell>` and the column count.
    ///
    /// Returns `(cells, cols)` where `cells` contains `history_lines * cols`
    /// cells in row-major order, oldest row first. Only rows with actual
    /// content are included (trailing empty rows are trimmed).
    pub fn extract_scrollback(&self) -> (Vec<Cell>, u16) {
        use alacritty_terminal::index::{Column, Line};

        let grid = self.term.grid();
        let cols = self.term.columns();
        let history_lines = grid.history_size();

        if history_lines == 0 {
            return (Vec::new(), cols as u16);
        }

        // Find last non-empty row (scan bottom-up)
        let mut content_rows = 0usize;
        for line_idx in (0..history_lines).rev() {
            // Scrollback lines: Line(-history_lines) is oldest, Line(-1) is newest
            let actual_line = Line(-((history_lines - line_idx) as i32));
            let row = &grid[actual_line];
            let has_content = (0..cols).any(|c| {
                let cell = &row[Column(c)];
                cell.c != ' ' && cell.c != '\0'
            });
            if has_content {
                content_rows = line_idx + 1;
                break;
            }
        }

        if content_rows == 0 {
            return (Vec::new(), cols as u16);
        }

        let mut cells = Vec::with_capacity(content_rows * cols);
        for line_idx in 0..content_rows {
            // Line(-history_lines) = oldest, Line(-1) = newest scrollback row
            let term_line = Line(-((history_lines - line_idx) as i32));
            let row = &grid[term_line];
            for col_idx in 0..cols {
                let cell = &row[Column(col_idx)];
                let zerowidth = cell
                    .zerowidth()
                    .filter(|zw| !zw.is_empty())
                    .map(|zw| zw.to_vec().into_boxed_slice());
                cells.push(Cell {
                    c: cell.c,
                    fg: Color::from(cell.fg),
                    bg: Color::from(cell.bg),
                    flags: CellFlags::from(cell.flags),
                    zerowidth,
                });
            }
        }

        (cells, cols as u16)
    }

    /// Take the latest OSC title.
    pub fn take_title(&self) -> Option<String> {
        self.title_slot.lock().unwrap().take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_parser_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<ShadowParser>();
    }

    #[test]
    fn feed_and_extract() {
        let mut parser = ShadowParser::new(80, 24);
        parser.feed(b"Hello, world!\r\n");
        let grid = parser.extract_grid();
        let text = grid.to_string();
        assert!(text.contains("Hello, world!"));
    }

    #[test]
    fn alternate_screen_detection() {
        let mut parser = ShadowParser::new(80, 24);
        assert!(!parser.is_alternate_screen());
        // Enter alt screen
        parser.feed(b"\x1b[?1049h");
        assert!(parser.is_alternate_screen());
        // Exit alt screen
        parser.feed(b"\x1b[?1049l");
        assert!(!parser.is_alternate_screen());
    }

    #[test]
    fn resize_works() {
        let mut parser = ShadowParser::new(80, 24);
        parser.feed(b"test content\r\n");
        parser.resize(120, 40);
        assert_eq!(parser.size(), (120, 40));
    }

    #[test]
    fn extract_scrollback_empty() {
        let parser = ShadowParser::new(80, 24);
        let (cells, cols) = parser.extract_scrollback();
        assert!(cells.is_empty());
        assert_eq!(cols, 80);
    }

    #[test]
    fn extract_scrollback_with_content() {
        let mut parser = ShadowParser::new(80, 5);
        // Write enough lines to push some into scrollback
        for i in 0..10 {
            parser.feed(format!("line {i}\r\n").as_bytes());
        }
        let (cells, cols) = parser.extract_scrollback();
        assert_eq!(cols, 80);
        // Should have some scrollback rows
        assert!(!cells.is_empty());
        // First scrollback row should start with 'l' (from "line N")
        assert_eq!(cells[0].c, 'l');
    }
}
