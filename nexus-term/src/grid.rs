//! Terminal grid - a 2D array of cells.

use std::cell::Cell as StdCell;

use crate::cell::Cell;

/// A terminal grid containing rows of cells.
#[derive(Debug, Clone)]
pub struct TerminalGrid {
    /// The cells, stored row-major.
    cells: Vec<Cell>,
    /// Number of columns.
    cols: u16,
    /// Number of rows.
    rows: u16,
    /// Cursor position (column).
    cursor_col: u16,
    /// Cursor position (row).
    cursor_row: u16,
    /// Whether cursor is visible.
    cursor_visible: bool,
    /// Cached content height (last non-empty row + 1).
    /// Uses Cell for interior mutability since this is computed lazily.
    content_rows_cache: StdCell<Option<u16>>,
}

impl TerminalGrid {
    /// Create a new grid with the given dimensions.
    pub fn new(cols: u16, rows: u16) -> Self {
        let size = cols as usize * rows as usize;
        Self {
            cells: vec![Cell::default(); size],
            cols,
            rows,
            cursor_col: 0,
            cursor_row: 0,
            cursor_visible: true,
            content_rows_cache: StdCell::new(None),
        }
    }

    /// Get the grid dimensions.
    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Get the number of columns.
    pub fn cols(&self) -> u16 {
        self.cols
    }

    /// Get the number of rows.
    pub fn rows(&self) -> u16 {
        self.rows
    }

    /// Get a cell at the given position.
    pub fn get(&self, col: u16, row: u16) -> Option<&Cell> {
        if col < self.cols && row < self.rows {
            let idx = row as usize * self.cols as usize + col as usize;
            self.cells.get(idx)
        } else {
            None
        }
    }

    /// Get a mutable cell at the given position.
    pub fn get_mut(&mut self, col: u16, row: u16) -> Option<&mut Cell> {
        if col < self.cols && row < self.rows {
            let idx = row as usize * self.cols as usize + col as usize;
            self.cells.get_mut(idx)
        } else {
            None
        }
    }

    /// Set a cell at the given position.
    pub fn set(&mut self, col: u16, row: u16, cell: Cell) {
        if col < self.cols && row < self.rows {
            let idx = row as usize * self.cols as usize + col as usize;
            self.cells[idx] = cell;
            // Invalidate content rows cache when cells change
            self.content_rows_cache.set(None);
        }
    }

    /// Get the number of rows with actual content (cached).
    /// This scans the grid once and caches the result.
    pub fn content_rows(&self) -> u16 {
        if let Some(cached) = self.content_rows_cache.get() {
            return cached;
        }

        // Scan from bottom to top to find last row with content
        let mut last_content_row: u16 = 0;
        for (row_idx, row) in self.rows_iter().enumerate() {
            let has_content = row.iter().any(|cell| cell.c != '\0' && cell.c != ' ');
            if has_content {
                last_content_row = (row_idx + 1) as u16;
            }
        }

        // Include cursor row if visible
        if self.cursor_visible {
            let cursor_row = self.cursor_row + 1;
            if cursor_row > last_content_row {
                last_content_row = cursor_row;
            }
        }

        let result = last_content_row.max(1);
        self.content_rows_cache.set(Some(result));
        result
    }

    /// Get the cursor position.
    pub fn cursor(&self) -> (u16, u16) {
        (self.cursor_col, self.cursor_row)
    }

    /// Set the cursor position.
    pub fn set_cursor(&mut self, col: u16, row: u16) {
        self.cursor_col = col.min(self.cols.saturating_sub(1));
        self.cursor_row = row.min(self.rows.saturating_sub(1));
    }

    /// Check if cursor is visible.
    pub fn cursor_visible(&self) -> bool {
        self.cursor_visible
    }

    /// Set cursor visibility.
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.cursor_visible = visible;
    }

    /// Clear the entire grid.
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            *cell = Cell::default();
        }
        self.cursor_col = 0;
        self.cursor_row = 0;
    }

    /// Resize the grid, preserving content where possible.
    pub fn resize(&mut self, new_cols: u16, new_rows: u16) {
        let mut new_cells = vec![Cell::default(); new_cols as usize * new_rows as usize];

        // Copy existing content (Cell is Copy, so no clone needed)
        let copy_cols = self.cols.min(new_cols) as usize;
        let copy_rows = self.rows.min(new_rows) as usize;

        for row in 0..copy_rows {
            for col in 0..copy_cols {
                let old_idx = row * self.cols as usize + col;
                let new_idx = row * new_cols as usize + col;
                new_cells[new_idx] = self.cells[old_idx];
            }
        }

        self.cells = new_cells;
        self.cols = new_cols;
        self.rows = new_rows;
        self.cursor_col = self.cursor_col.min(new_cols.saturating_sub(1));
        self.cursor_row = self.cursor_row.min(new_rows.saturating_sub(1));
        // Invalidate content rows cache
        self.content_rows_cache.set(None);
    }

    /// Iterate over rows.
    pub fn rows_iter(&self) -> impl Iterator<Item = &[Cell]> {
        self.cells.chunks(self.cols as usize)
    }

    /// Get all cells as a slice.
    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Extract visible text content (for debugging/search).
    pub fn to_string(&self) -> String {
        let mut result = String::new();
        for row in self.rows_iter() {
            for cell in row {
                result.push(if cell.c == '\0' { ' ' } else { cell.c });
            }
            result.push('\n');
        }
        result
    }
}

impl Default for TerminalGrid {
    fn default() -> Self {
        Self::new(crate::DEFAULT_COLS, crate::DEFAULT_ROWS)
    }
}
