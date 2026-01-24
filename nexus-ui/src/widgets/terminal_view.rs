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
        let (cols, total_rows) = self.grid.size();

        // Calculate visible rows
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
        let first_row = first_visible_row.saturating_sub(1);
        let last_row = (last_visible_row + 1).min(total_rows as usize);

        // Build text for visible rows only
        // TODO: Replace fill_text with font atlas rendering for better performance
        let cells = self.grid.cells();
        let cols_usize = cols as usize;

        let mut visible_text = String::with_capacity((last_row - first_row) * (cols_usize + 1));
        for row_idx in first_row..last_row {
            let row_start = row_idx * cols_usize;
            let row_end = (row_start + cols_usize).min(cells.len());
            if row_start >= cells.len() {
                break;
            }
            for cell in &cells[row_start..row_end] {
                visible_text.push(if cell.c == '\0' { ' ' } else { cell.c });
            }
            visible_text.push('\n');
        }

        let text_y = bounds.y + first_row as f32 * cell_height;
        let char_width = self.char_width;
        let clip_bounds = Rectangle {
            width: bounds.width + char_width,
            ..bounds
        };

        renderer.fill_text(
            iced::advanced::text::Text {
                content: visible_text,
                bounds: Size::new(bounds.width, (last_row - first_row) as f32 * cell_height),
                size: iced::Pixels(self.font_size),
                line_height: iced::advanced::text::LineHeight::Relative(self.line_height),
                font: iced::Font::MONOSPACE,
                horizontal_alignment: iced::alignment::Horizontal::Left,
                vertical_alignment: iced::alignment::Vertical::Top,
                shaping: iced::advanced::text::Shaping::Basic,
                wrapping: iced::advanced::text::Wrapping::None,
            },
            iced::Point::new(bounds.x, text_y),
            Color::from_rgb(0.9, 0.9, 0.9),
            clip_bounds,
        );

        // Draw cursor if visible and within viewport
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
