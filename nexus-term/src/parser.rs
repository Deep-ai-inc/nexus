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

    /// Get the raw cursor position from alacritty's grid (not renderable_content).
    pub fn raw_cursor(&self) -> (u16, u16) {
        let cursor = &self.term.grid().cursor;
        (cursor.point.column.0 as u16, cursor.point.line.0 as u16)
    }

    /// Whether DECTCEM (show cursor mode) is currently on.
    pub fn cursor_visible_mode(&self) -> bool {
        self.term.mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR)
    }

    /// Feed bytes and track DECTCEM transitions.
    /// Returns `Some((col, row))` if the cursor became visible during this
    /// feed (via `ESC[?25h`), giving the position where the app revealed
    /// the cursor — typically the input position for TUI apps.
    ///
    /// Scans raw bytes for the DECTCEM-on sequence and splits processing
    /// to capture the cursor position at that exact moment, even if the
    /// cursor is hidden again later in the same chunk.
    pub fn feed_tracking_cursor(&mut self, bytes: &[u8]) -> Option<(u16, u16)> {
        // ESC[?25h = show cursor (DECTCEM on)
        const SHOW_CURSOR_SEQ: &[u8] = b"\x1b[?25h";
        let mut last_visible_pos: Option<(u16, u16)> = None;
        let mut start = 0;

        // Scan for all occurrences of ESC[?25h in the byte stream.
        // Process up to and including each occurrence, then capture cursor.
        while start < bytes.len() {
            if let Some(offset) = find_subsequence(&bytes[start..], SHOW_CURSOR_SEQ) {
                let end = start + offset + SHOW_CURSOR_SEQ.len();
                // Process bytes up through the show-cursor sequence
                self.processor.advance(&mut self.term, &bytes[start..end]);
                // Capture cursor position at the moment it became visible
                let cursor = &self.term.grid().cursor;
                last_visible_pos = Some((
                    cursor.point.column.0 as u16,
                    cursor.point.line.0 as u16,
                ));
                start = end;
            } else {
                // No more show-cursor sequences — process the rest
                self.processor.advance(&mut self.term, &bytes[start..]);
                break;
            }
        }

        // Invalidate caches
        *self.cached_viewport.borrow_mut() = None;
        *self.cached_scrollback.borrow_mut() = None;

        last_visible_pos
    }

    /// Feed bytes and find the last grid position where a character was written.
    ///
    /// Snapshots the viewport before feeding, then scans bottom-right to
    /// top-left for the last cell that changed. This reveals where TUI apps
    /// (like Claude Code) actually drew their input text, even when the
    /// terminal cursor is parked at a meaningless position with DECTCEM off.
    ///
    /// Also performs DECTCEM tracking. Returns:
    /// - `last_write_pos`: last cell that changed (bottom-right-most)
    /// - `dectcem_pos`: cursor position at DECTCEM-on transition (if any)
    pub fn feed_tracking_writes(
        &mut self,
        bytes: &[u8],
    ) -> (Option<(u16, u16)>, Option<(u16, u16)>) {
        let cols = self.term.columns() as u16;
        let rows = self.term.screen_lines() as u16;

        // Snapshot current viewport cell characters
        let grid = self.term.grid();
        let mut snapshot: Vec<char> = Vec::with_capacity((cols as usize) * (rows as usize));
        for row in 0..rows as i32 {
            let term_line = Line(row);
            let grid_row = &grid[term_line];
            for col in 0..cols as usize {
                snapshot.push(grid_row[Column(col)].c);
            }
        }

        // Feed with DECTCEM tracking
        let dectcem_pos = self.feed_tracking_cursor_inner(bytes);

        // Scan for last changed cell (bottom-right to top-left)
        let grid = self.term.grid();
        let mut last_write: Option<(u16, u16)> = None;
        'outer: for row in (0..rows).rev() {
            let term_line = Line(row as i32);
            let grid_row = &grid[term_line];
            for col in (0..cols).rev() {
                let new_ch = grid_row[Column(col as usize)].c;
                let old_ch = snapshot[(row as usize) * (cols as usize) + (col as usize)];
                if new_ch != old_ch && new_ch != ' ' && new_ch != '\0' {
                    last_write = Some((col, row));
                    break 'outer;
                }
            }
        }

        (last_write, dectcem_pos)
    }

    /// Internal: feed with DECTCEM tracking, no cache invalidation.
    fn feed_tracking_cursor_inner(&mut self, bytes: &[u8]) -> Option<(u16, u16)> {
        const SHOW_CURSOR_SEQ: &[u8] = b"\x1b[?25h";
        let mut last_visible_pos: Option<(u16, u16)> = None;
        let mut start = 0;

        while start < bytes.len() {
            if let Some(offset) = find_subsequence(&bytes[start..], SHOW_CURSOR_SEQ) {
                let end = start + offset + SHOW_CURSOR_SEQ.len();
                self.processor.advance(&mut self.term, &bytes[start..end]);
                let cursor = &self.term.grid().cursor;
                last_visible_pos = Some((
                    cursor.point.column.0 as u16,
                    cursor.point.line.0 as u16,
                ));
                start = end;
            } else {
                self.processor.advance(&mut self.term, &bytes[start..]);
                break;
            }
        }

        *self.cached_viewport.borrow_mut() = None;
        *self.cached_scrollback.borrow_mut() = None;

        last_visible_pos
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

        // Calculate actual content height to avoid huge empty grids.
        // Always include the cursor row so predictions align with the cursor position.
        let content_rows = self.compute_content_height();
        let cursor_point = grid.cursor.point;
        let cursor_row_in_grid = (cursor_point.line.0 + history_lines as i32 + 1) as usize;
        let total_to_render = content_rows.max(cursor_row_in_grid).min(total_lines).max(1);

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

    /// Replace the cached viewport with an externally-provided grid snapshot.
    ///
    /// Used on reconnect: the agent sends a `TerminalSnapshot` with the
    /// definitive screen state. The next `feed()` call will invalidate
    /// this snapshot, so any subsequent PTY output overwrites it naturally.
    pub fn set_viewport_snapshot(&mut self, grid: TerminalGrid) {
        *self.cached_viewport.borrow_mut() = Some(Rc::new(grid));
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

/// Find the first occurrence of `needle` in `haystack`.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
