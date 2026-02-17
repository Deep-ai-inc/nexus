//! Terminal parser - feeds bytes through alacritty_terminal to update grid state.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config, Term, test::TermSize};
use alacritty_terminal::vte::ansi::Processor;

use crate::cell::{Cell, CellFlags, Color};
use crate::grid::TerminalGrid;

/// A terminal parser that maintains grid state.
///
/// Uses interior mutability (RefCell) for grid caching to allow
/// cache population during `&self` view calls.
pub struct TerminalParser {
    /// The alacritty terminal state.
    term: Term<EventProxy>,
    /// ANSI parser/processor.
    processor: Processor,
    /// Cached viewport grid (for running blocks).
    cached_viewport: RefCell<Option<Rc<TerminalGrid>>>,
    /// Cached full grid with scrollback (for finished blocks).
    cached_scrollback: RefCell<Option<Rc<TerminalGrid>>>,
    /// Shared storage for the latest OSC title set by the child process.
    title_slot: Arc<Mutex<Option<String>>>,
}

impl std::fmt::Debug for TerminalParser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalParser")
            .field("size", &self.size())
            .finish_non_exhaustive()
    }
}

/// Event listener that captures title changes from the terminal.
struct EventProxy {
    title_slot: Arc<Mutex<Option<String>>>,
}

impl EventListener for EventProxy {
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
        let title_slot = Arc::new(Mutex::new(None));
        let proxy = EventProxy { title_slot: title_slot.clone() };
        let term = Term::new(config, &size, proxy);
        let processor = Processor::new();

        Self {
            term,
            processor,
            cached_viewport: RefCell::new(None),
            cached_scrollback: RefCell::new(None),
            title_slot,
        }
    }

    /// Feed bytes into the parser. Invalidates cached grids.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.term, bytes);
        // Invalidate caches - new content means grids need regeneration
        *self.cached_viewport.borrow_mut() = None;
        *self.cached_scrollback.borrow_mut() = None;
    }

    /// Take the latest OSC title set by the child process, if any.
    /// Returns `Some(title)` if the child set a title since the last call,
    /// or `None` if no title was set (or it was reset).
    pub fn take_title(&self) -> Option<String> {
        self.title_slot.lock().unwrap().take()
    }

    /// Peek at the current OSC title without consuming it.
    pub fn osc_title(&self) -> Option<String> {
        self.title_slot.lock().unwrap().clone()
    }

    /// Invalidate all cached grids (call after resize).
    pub fn invalidate_cache(&self) {
        *self.cached_viewport.borrow_mut() = None;
        *self.cached_scrollback.borrow_mut() = None;
    }

    /// Extract the current grid state (viewport only).
    /// Uses caching - only regenerates when cache is invalid.
    pub fn grid(&self) -> Rc<TerminalGrid> {
        // Check if cache is valid
        if let Some(ref cached) = *self.cached_viewport.borrow() {
            return Rc::clone(cached);
        }

        // Cache miss - extract grid
        let grid = Rc::new(self.extract_viewport());
        *self.cached_viewport.borrow_mut() = Some(Rc::clone(&grid));
        grid
    }

    /// Internal: extract viewport without caching.
    fn extract_viewport(&self) -> TerminalGrid {
        let term_content = self.term.renderable_content();
        let cols = self.term.columns();
        let rows = self.term.screen_lines();

        let mut grid = TerminalGrid::new(cols as u16, rows as u16);

        // Copy cells from alacritty's grid
        for indexed_cell in term_content.display_iter {
            let col = indexed_cell.point.column.0 as u16;
            let row = indexed_cell.point.line.0 as u16;

            let zerowidth = indexed_cell.zerowidth()
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

        // Set cursor position and shape.
        let cursor = term_content.cursor;
        grid.set_cursor(cursor.point.column.0 as u16, cursor.point.line.0 as u16);
        let shape = match cursor.shape {
            alacritty_terminal::vte::ansi::CursorShape::Block => crate::grid::CursorShape::Block,
            alacritty_terminal::vte::ansi::CursorShape::HollowBlock => crate::grid::CursorShape::HollowBlock,
            alacritty_terminal::vte::ansi::CursorShape::Beam => crate::grid::CursorShape::Beam,
            alacritty_terminal::vte::ansi::CursorShape::Underline => crate::grid::CursorShape::Underline,
            alacritty_terminal::vte::ansi::CursorShape::Hidden => crate::grid::CursorShape::Hidden,
        };
        grid.set_cursor_shape(shape);

        grid
    }

    /// Extract ALL content including scrollback history.
    /// Returns a grid sized to fit all content (scrollback + visible).
    /// Used for finished blocks that need to show complete output.
    /// Uses caching - only regenerates when cache is invalid.
    pub fn grid_with_scrollback(&self) -> Rc<TerminalGrid> {
        // Check if cache is valid
        if let Some(ref cached) = *self.cached_scrollback.borrow() {
            return Rc::clone(cached);
        }

        // Cache miss - extract full grid
        let grid = Rc::new(self.extract_scrollback());
        *self.cached_scrollback.borrow_mut() = Some(Rc::clone(&grid));
        grid
    }

    /// Internal: extract full grid with scrollback without caching.
    fn extract_scrollback(&self) -> TerminalGrid {
        let grid = self.term.grid();
        let cols = self.term.columns();
        let screen_lines = self.term.screen_lines();
        let history_lines = grid.history_size();
        let total_lines = screen_lines + history_lines;

        // Calculate actual content height to avoid huge empty grids
        let content_rows = self.compute_content_height();
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
                let zerowidth = cell.zerowidth()
                    .filter(|zw| !zw.is_empty())
                    .map(|zw| zw.to_vec().into_boxed_slice());
                let our_cell = Cell {
                    c: cell.c,
                    fg: Color::from(cell.fg),
                    bg: Color::from(cell.bg),
                    flags: CellFlags::from(cell.flags),
                    zerowidth,
                };
                result.set(col_idx as u16, line_idx as u16, our_cell);
            }
        }

        // Position cursor relative to full content
        let cursor_point = grid.cursor.point;
        let cursor_row = (cursor_point.line.0 + history_lines as i32) as u16;
        result.set_cursor(cursor_point.column.0 as u16, cursor_row.min(total_to_render.saturating_sub(1) as u16));

        // Set cursor shape (considering visibility via renderable_content).
        let cursor_shape = self.term.renderable_content().cursor.shape;
        let shape = match cursor_shape {
            alacritty_terminal::vte::ansi::CursorShape::Block => crate::grid::CursorShape::Block,
            alacritty_terminal::vte::ansi::CursorShape::HollowBlock => crate::grid::CursorShape::HollowBlock,
            alacritty_terminal::vte::ansi::CursorShape::Beam => crate::grid::CursorShape::Beam,
            alacritty_terminal::vte::ansi::CursorShape::Underline => crate::grid::CursorShape::Underline,
            alacritty_terminal::vte::ansi::CursorShape::Hidden => crate::grid::CursorShape::Hidden,
        };
        result.set_cursor_shape(shape);

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

    /// Resize the terminal. Invalidates cached grids.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let size = TermSize::new(cols as usize, rows as usize);
        self.term.resize(size);
        // Invalidate caches - size change means grids need regeneration
        *self.cached_viewport.borrow_mut() = None;
        *self.cached_scrollback.borrow_mut() = None;
    }

    /// Get terminal dimensions.
    pub fn size(&self) -> (u16, u16) {
        (self.term.columns() as u16, self.term.screen_lines() as u16)
    }

    /// Check if in alternate screen mode (for TUI apps).
    pub fn is_alternate_screen(&self) -> bool {
        self.term.mode().contains(alacritty_terminal::term::TermMode::ALT_SCREEN)
    }

    /// DECCKM: Application Cursor Keys mode.  When true, unmodified arrow
    /// keys should emit SS3 sequences (`\x1bOA`) instead of CSI (`\x1b[A`).
    pub fn app_cursor(&self) -> bool {
        self.term.mode().contains(alacritty_terminal::term::TermMode::APP_CURSOR)
    }

    /// Whether the terminal has enabled Bracketed Paste mode (`\x1b[?2004h`).
    /// When true, pasted text must be wrapped in `\x1b[200~` / `\x1b[201~`.
    pub fn bracketed_paste(&self) -> bool {
        self.term.mode().contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE)
    }

    /// Calculate the number of rows that have actual content.
    /// Includes both visible screen AND scrollback history.
    /// Used for sizing finished blocks to show all their content.
    pub fn content_height(&self) -> usize {
        self.compute_content_height()
    }

    /// Internal: compute content height by scanning grid.
    fn compute_content_height(&self) -> usize {
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
