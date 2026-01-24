//! Terminal view widget - renders a terminal grid using Iced primitives.

use std::rc::Rc;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Widget};
use iced::mouse;
use iced::{Color, Element, Length, Rectangle, Size, Theme};

use nexus_term::{Color as TermColor, TerminalGrid};

use crate::app::{CHAR_WIDTH_RATIO, LINE_HEIGHT_FACTOR};

/// A widget that renders a terminal grid.
/// Uses Rc<TerminalGrid> for cheap cloning - the grid is cached in the parser.
pub struct TerminalView {
    grid: Rc<TerminalGrid>,
    font_size: f32,
    line_height: f32,
    char_width: f32,
    show_cursor: bool,
}

impl TerminalView {
    /// Create a new terminal view with the given font size.
    pub fn new(grid: Rc<TerminalGrid>, font_size: f32) -> Self {
        Self {
            grid,
            font_size,
            line_height: LINE_HEIGHT_FACTOR,
            char_width: font_size * CHAR_WIDTH_RATIO,
            show_cursor: true,
        }
    }

    /// Set whether to show the cursor.
    pub fn show_cursor(mut self, show: bool) -> Self {
        self.show_cursor = show;
        self
    }

    fn cell_height(&self) -> f32 {
        self.font_size * self.line_height
    }

    #[allow(dead_code)]
    fn term_color_to_iced(&self, color: &TermColor, is_fg: bool) -> Color {
        match color {
            TermColor::Default => {
                if is_fg {
                    Color::from_rgb(0.9, 0.9, 0.9)
                } else {
                    Color::TRANSPARENT
                }
            }
            TermColor::Named(n) | TermColor::Indexed(n) => ansi_index_to_color(*n),
            TermColor::Rgb(r, g, b) => {
                Color::from_rgb(*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0)
            }
        }
    }
}

fn ansi_index_to_color(index: u8) -> Color {
    match index {
        0 => Color::from_rgb(0.0, 0.0, 0.0),
        1 => Color::from_rgb(0.8, 0.2, 0.2),
        2 => Color::from_rgb(0.05, 0.74, 0.47),
        3 => Color::from_rgb(0.9, 0.9, 0.06),
        4 => Color::from_rgb(0.14, 0.45, 0.78),
        5 => Color::from_rgb(0.74, 0.25, 0.74),
        6 => Color::from_rgb(0.07, 0.66, 0.8),
        7 => Color::from_rgb(0.9, 0.9, 0.9),
        8 => Color::from_rgb(0.4, 0.4, 0.4),
        9 => Color::from_rgb(0.95, 0.3, 0.3),
        10 => Color::from_rgb(0.14, 0.82, 0.55),
        11 => Color::from_rgb(0.96, 0.96, 0.26),
        12 => Color::from_rgb(0.23, 0.56, 0.92),
        13 => Color::from_rgb(0.84, 0.44, 0.84),
        14 => Color::from_rgb(0.16, 0.72, 0.86),
        15 => Color::from_rgb(1.0, 1.0, 1.0),
        // 216 color cube
        16..=231 => {
            let n = index - 16;
            let r = (n / 36) % 6;
            let g = (n / 6) % 6;
            let b = n % 6;
            let to_val = |v: u8| if v == 0 { 0.0 } else { (55.0 + v as f32 * 40.0) / 255.0 };
            Color::from_rgb(to_val(r), to_val(g), to_val(b))
        }
        // Grayscale
        232..=255 => {
            let gray = (8 + (index - 232) * 10) as f32 / 255.0;
            Color::from_rgb(gray, gray, gray)
        }
    }
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for TerminalView
where
    Renderer: renderer::Renderer + iced::advanced::text::Renderer<Font = iced::Font>,
{
    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Shrink,
        }
    }

    fn layout(
        &self,
        _tree: &mut widget::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let (cols, _rows) = self.grid.size();
        let width = cols as f32 * self.char_width;

        // Use cached content rows from grid (no re-scanning)
        let content_rows = self.grid.content_rows() as usize;
        let height = content_rows as f32 * self.cell_height();

        let size = limits
            .width(Length::Fill)
            .height(Length::Shrink)
            .resolve(width, height, Size::new(width, height));

        layout::Node::new(Size::new(size.width, height))
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let cell_height = self.cell_height();
        let char_width = self.char_width;
        let (cols, total_rows) = self.grid.size();

        // Expand clip bounds slightly to prevent chopping last character
        let clip_bounds = Rectangle {
            width: bounds.width + char_width,
            ..bounds
        };

        // === VIEWPORT VIRTUALIZATION ===
        // Calculate which rows are actually visible in the viewport.
        let first_visible_row = if viewport.y > bounds.y {
            ((viewport.y - bounds.y) / cell_height).floor() as usize
        } else {
            0
        };

        let viewport_bottom = viewport.y + viewport.height;
        let last_visible_row = if viewport_bottom > bounds.y {
            ((viewport_bottom - bounds.y) / cell_height).ceil() as usize
        } else {
            0
        };

        // Clamp to actual grid bounds with 1 row buffer
        let first_row = first_visible_row.saturating_sub(1);
        let last_row = (last_visible_row + 1).min(total_rows as usize);

        // === HOISTED ALLOCATION ===
        // Single string buffer reused across ALL rows and style runs
        let mut text_buffer = String::with_capacity(cols as usize);

        // Draw only visible rows with STYLE BATCHING
        for (row_idx, row) in self.grid.rows_iter().enumerate() {
            if row_idx < first_row || row_idx > last_row {
                continue;
            }

            let y = bounds.y + row_idx as f32 * cell_height;

            // Skip entirely empty rows (quick check)
            if row.iter().all(|c| c.c == '\0' || c.c == ' ') {
                continue;
            }

            // === STYLE BATCHING (RLE) ===
            // Track current style and batch contiguous cells with same style.
            // Only issue a draw call when style changes.
            text_buffer.clear();
            let mut current_fg = row[0].fg;
            let mut run_start_col: usize = 0;

            for (col_idx, cell) in row.iter().enumerate() {
                // Check if style changed (we batch by foreground color)
                let style_changed = cell.fg != current_fg;

                if style_changed && !text_buffer.is_empty() {
                    // FLUSH: Draw the accumulated run
                    let run_x = bounds.x + (run_start_col as f32 * char_width);
                    let fg_color = self.term_color_to_iced(&current_fg, true);

                    renderer.fill_text(
                        iced::advanced::text::Text {
                            content: text_buffer.clone(),
                            bounds: Size::new(text_buffer.len() as f32 * char_width + char_width, cell_height),
                            size: iced::Pixels(self.font_size),
                            line_height: iced::advanced::text::LineHeight::Relative(self.line_height),
                            font: iced::Font::MONOSPACE,
                            horizontal_alignment: iced::alignment::Horizontal::Left,
                            vertical_alignment: iced::alignment::Vertical::Top,
                            shaping: iced::advanced::text::Shaping::Basic,
                            wrapping: iced::advanced::text::Wrapping::None,
                        },
                        iced::Point::new(run_x, y),
                        fg_color,
                        clip_bounds,
                    );

                    // Reset for new run
                    text_buffer.clear();
                    current_fg = cell.fg;
                    run_start_col = col_idx;
                }

                // Append char to current run
                let c = if cell.c == '\0' { ' ' } else { cell.c };
                text_buffer.push(c);
            }

            // FLUSH FINAL RUN (don't forget the last chunk)
            if !text_buffer.is_empty() {
                // Trim trailing spaces for the final run
                let trimmed = text_buffer.trim_end();
                if !trimmed.is_empty() {
                    let run_x = bounds.x + (run_start_col as f32 * char_width);
                    let fg_color = self.term_color_to_iced(&current_fg, true);

                    renderer.fill_text(
                        iced::advanced::text::Text {
                            content: trimmed.to_string(),
                            bounds: Size::new(trimmed.len() as f32 * char_width + char_width, cell_height),
                            size: iced::Pixels(self.font_size),
                            line_height: iced::advanced::text::LineHeight::Relative(self.line_height),
                            font: iced::Font::MONOSPACE,
                            horizontal_alignment: iced::alignment::Horizontal::Left,
                            vertical_alignment: iced::alignment::Vertical::Top,
                            shaping: iced::advanced::text::Shaping::Basic,
                            wrapping: iced::advanced::text::Wrapping::None,
                        },
                        iced::Point::new(run_x, y),
                        fg_color,
                        clip_bounds,
                    );
                }
            }
        }

        // Draw cursor if visible, enabled, and within viewport
        if self.show_cursor && self.grid.cursor_visible() {
            let (cursor_col, cursor_row) = self.grid.cursor();
            let cursor_row = cursor_row as usize;

            if cursor_row >= first_row && cursor_row <= last_row {
                let cursor_x = bounds.x + cursor_col as f32 * char_width;
                let cursor_y = bounds.y + cursor_row as f32 * cell_height;

                renderer.fill_quad(
                    renderer::Quad {
                        bounds: Rectangle {
                            x: cursor_x,
                            y: cursor_y,
                            width: char_width,
                            height: cell_height,
                        },
                        border: iced::Border::default(),
                        shadow: iced::Shadow::default(),
                    },
                    Color::from_rgba(0.9, 0.9, 0.9, 0.7),
                );
            }
        }
    }
}

impl<'a, Message, Renderer> From<TerminalView> for Element<'a, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer + iced::advanced::text::Renderer<Font = iced::Font>,
{
    fn from(view: TerminalView) -> Self {
        Self::new(view)
    }
}
