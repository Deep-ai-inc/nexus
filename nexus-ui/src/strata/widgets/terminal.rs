//! Terminal Widget
//!
//! A widget for rendering terminal grid content with cell-based hit-testing.

use crate::strata::content_address::SourceId;
use crate::strata::event_context::{Event, EventContext};
use crate::strata::gpu::StrataPipeline;
use crate::strata::layout_snapshot::{GridLayout, GridRow, LayoutSnapshot, SourceLayout};
use crate::strata::primitives::{Color, Constraints, Rect, Size};
use crate::strata::widget::{EventResult, StrataWidget};

/// Messages that a TerminalWidget can produce.
#[derive(Debug, Clone)]
pub enum TerminalMessage {
    /// User clicked at a cell position (col, row).
    Clicked { col: u16, row: u16 },
}

/// A terminal cell.
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    /// The character in this cell.
    pub ch: char,
    /// Foreground color.
    pub fg: Color,
    /// Background color.
    pub bg: Color,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::WHITE,
            bg: Color::TRANSPARENT,
        }
    }
}

/// A widget for rendering terminal grid content.
///
/// Provides cell-based layout for accurate hit-testing within terminal grids.
pub struct TerminalWidget {
    /// Unique source ID for this widget instance.
    source_id: SourceId,

    /// Grid dimensions.
    cols: u16,
    rows: u16,

    /// Cell dimensions.
    cell_width: f32,
    cell_height: f32,

    /// Grid content (row-major order).
    cells: Vec<Cell>,
}

impl TerminalWidget {
    /// Create a new terminal widget with the given dimensions.
    pub fn new(cols: u16, rows: u16, cell_width: f32, cell_height: f32) -> Self {
        let cell_count = cols as usize * rows as usize;
        Self {
            source_id: SourceId::new(),
            cols,
            rows,
            cell_width,
            cell_height,
            cells: vec![Cell::default(); cell_count],
        }
    }

    /// Create with a specific source ID.
    pub fn with_source_id(
        source_id: SourceId,
        cols: u16,
        rows: u16,
        cell_width: f32,
        cell_height: f32,
    ) -> Self {
        let cell_count = cols as usize * rows as usize;
        Self {
            source_id,
            cols,
            rows,
            cell_width,
            cell_height,
            cells: vec![Cell::default(); cell_count],
        }
    }

    /// Get a cell at (col, row).
    pub fn get_cell(&self, col: u16, row: u16) -> Option<&Cell> {
        if col < self.cols && row < self.rows {
            let idx = row as usize * self.cols as usize + col as usize;
            self.cells.get(idx)
        } else {
            None
        }
    }

    /// Set a cell at (col, row).
    pub fn set_cell(&mut self, col: u16, row: u16, cell: Cell) {
        if col < self.cols && row < self.rows {
            let idx = row as usize * self.cols as usize + col as usize;
            if let Some(c) = self.cells.get_mut(idx) {
                *c = cell;
            }
        }
    }

    /// Write a string starting at (col, row).
    pub fn write_str(&mut self, col: u16, row: u16, s: &str, fg: Color, bg: Color) {
        let mut c = col;
        for ch in s.chars() {
            if c >= self.cols {
                break;
            }
            self.set_cell(c, row, Cell { ch, fg, bg });
            c += 1;
        }
    }

    /// Clear all cells.
    pub fn clear(&mut self) {
        for cell in &mut self.cells {
            *cell = Cell::default();
        }
    }

    /// Get grid dimensions.
    pub fn dimensions(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Get cell dimensions.
    pub fn cell_dimensions(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }
}

impl StrataWidget<TerminalMessage> for TerminalWidget {
    fn source_id(&self) -> SourceId {
        self.source_id
    }

    fn measure(&self, constraints: Constraints) -> Size {
        let width = self.cols as f32 * self.cell_width;
        let height = self.rows as f32 * self.cell_height;
        constraints.constrain(Size::new(width, height))
    }

    fn layout(&mut self, snapshot: &mut LayoutSnapshot, bounds: Rect) {
        // Build row content for rendering
        let mut rows_content = Vec::with_capacity(self.rows as usize);
        for row in 0..self.rows {
            let mut line = String::with_capacity(self.cols as usize);
            let mut row_color = Color::WHITE;

            for col in 0..self.cols {
                if let Some(cell) = self.get_cell(col, row) {
                    if col == 0 {
                        row_color = cell.fg;
                    }
                    line.push(cell.ch);
                }
            }

            rows_content.push(GridRow {
                text: line,
                color: row_color.pack(),
            });
        }

        let grid_layout = GridLayout::with_rows(
            bounds,
            self.cell_width,
            self.cell_height,
            self.cols,
            self.rows,
            rows_content,
        );
        snapshot.register_source(self.source_id, SourceLayout::grid(grid_layout));
    }

    fn event(&mut self, ctx: &EventContext, event: &Event) -> EventResult<TerminalMessage> {
        use crate::strata::event_context::{MouseButton, MouseEvent};

        match event {
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                position,
            }) => {
                if let Some(addr) = ctx.layout.hit_test(*position) {
                    if addr.source_id == self.source_id {
                        // Convert content_offset to (col, row)
                        let col = (addr.content_offset % self.cols as usize) as u16;
                        let row = (addr.content_offset / self.cols as usize) as u16;
                        return EventResult::Message(TerminalMessage::Clicked { col, row });
                    }
                }
                EventResult::Ignored
            }
            _ => EventResult::Ignored,
        }
    }

    fn render(&self, pipeline: &mut StrataPipeline, bounds: Rect) {
        // Render each row
        for row in 0..self.rows {
            let y = bounds.y + row as f32 * self.cell_height;

            // Build a string for this row
            let mut line = String::with_capacity(self.cols as usize);
            let mut current_fg = Color::WHITE;

            for col in 0..self.cols {
                if let Some(cell) = self.get_cell(col, row) {
                    // For now, just use the first cell's color for the whole row
                    // TODO: Support per-cell colors
                    if col == 0 {
                        current_fg = cell.fg;
                    }
                    line.push(cell.ch);
                }
            }

            if !line.trim().is_empty() {
                pipeline.add_text(&line, bounds.x, y, current_fg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_widget_creation() {
        let widget = TerminalWidget::new(80, 24, 8.0, 16.0);
        assert_eq!(widget.dimensions(), (80, 24));
        assert_eq!(widget.cell_dimensions(), (8.0, 16.0));
    }

    #[test]
    fn test_terminal_widget_write() {
        let mut widget = TerminalWidget::new(80, 24, 8.0, 16.0);
        widget.write_str(0, 0, "Hello", Color::WHITE, Color::TRANSPARENT);

        assert_eq!(widget.get_cell(0, 0).unwrap().ch, 'H');
        assert_eq!(widget.get_cell(4, 0).unwrap().ch, 'o');
        assert_eq!(widget.get_cell(5, 0).unwrap().ch, ' '); // Default
    }

    #[test]
    fn test_terminal_widget_measure() {
        let widget = TerminalWidget::new(80, 24, 8.0, 16.0);
        let size = widget.measure(Constraints::UNBOUNDED);

        assert_eq!(size.width, 640.0); // 80 * 8
        assert_eq!(size.height, 384.0); // 24 * 16
    }
}
