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
    /// Maintains content_rows cache incrementally for O(1) updates.
    pub fn set(&mut self, col: u16, row: u16, cell: Cell) {
        if col < self.cols && row < self.rows {
            let idx = row as usize * self.cols as usize + col as usize;
            self.cells[idx] = cell;

            // Incremental cache maintenance:
            // If we wrote visible content at or below cached bottom, bump cache upward.
            // If we might be clearing the last content row, drop cache to force rescan.
            if cell.c != '\0' && cell.c != ' ' {
                if let Some(cached) = self.content_rows_cache.get() {
                    let needed = row + 1;
                    if needed > cached {
                        self.content_rows_cache.set(Some(needed));
                    }
                }
            } else {
                // Clearing a cell - if it's on the cached last row, we must rescan
                if let Some(cached) = self.content_rows_cache.get() {
                    if row + 1 == cached {
                        self.content_rows_cache.set(None);
                    }
                }
            }
        }
    }

    /// Invalidate the content rows cache (call after bulk modifications).
    pub fn invalidate_content_cache(&mut self) {
        self.content_rows_cache.set(None);
    }

    /// Set the content rows cache directly (for use after extraction).
    pub fn set_content_rows_cache(&self, rows: u16) {
        self.content_rows_cache.set(Some(rows));
    }

    /// Get the number of rows with actual content (cached).
    /// Scans from bottom-up for O(1) best case (full screens).
    pub fn content_rows(&self) -> u16 {
        if let Some(cached) = self.content_rows_cache.get() {
            return cached;
        }

        // OPTIMIZATION: Scan REVERSE (bottom-up) to find content immediately.
        // For a full screen, this is O(1) instead of O(N).
        let mut last_content_row: u16 = 0;
        let cols = self.cols as usize;

        // Iterate rows from bottom to top using direct indexing
        for row_idx in (0..self.rows as usize).rev() {
            let row_start = row_idx * cols;
            let row_end = row_start + cols;
            let row = &self.cells[row_start..row_end];

            if row.iter().any(|cell| cell.c != '\0' && cell.c != ' ') {
                last_content_row = (row_idx + 1) as u16;
                break; // Found the bottom content row, stop scanning
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
        let total_cells = new_cols as usize * new_rows as usize;

        // Pre-allocate then fill (avoids zero-init + copy overhead)
        let mut new_cells = Vec::with_capacity(total_cells);
        new_cells.resize(total_cells, Cell::default());

        // Copy existing content using row-wise memcpy where possible
        let copy_cols = self.cols.min(new_cols) as usize;
        let copy_rows = self.rows.min(new_rows) as usize;
        let old_cols = self.cols as usize;
        let new_cols_usize = new_cols as usize;

        for row in 0..copy_rows {
            let old_start = row * old_cols;
            let new_start = row * new_cols_usize;
            // Copy entire row slice at once (Cell is Copy)
            new_cells[new_start..new_start + copy_cols]
                .copy_from_slice(&self.cells[old_start..old_start + copy_cols]);
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
