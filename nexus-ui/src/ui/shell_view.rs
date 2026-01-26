//! Shell block rendering.

use iced::widget::{column, row, text};
use iced::Element;

use crate::blocks::Block;
use crate::glyph_cache::get_cell_metrics;
use crate::msg::Message;
use crate::ui::value_view::render_value;
use crate::widgets::terminal_shader::TerminalShader;

/// Render a shell command block.
pub fn view_block(block: &Block, font_size: f32) -> Element<'_, Message> {
    let prompt_color = iced::Color::from_rgb(0.3, 0.8, 0.5);
    let command_color = iced::Color::from_rgb(0.9, 0.9, 0.9);

    let prompt_line = row![
        text("$ ")
            .size(font_size)
            .color(prompt_color)
            .font(iced::Font::MONOSPACE),
        text(&block.command)
            .size(font_size)
            .color(command_color)
            .font(iced::Font::MONOSPACE),
    ]
    .spacing(0);

    // Check for native output first
    let output: Element<Message> = if block.collapsed {
        column![].into()
    } else if let Some(value) = &block.native_output {
        // Render structured value from native command
        render_value(value, block.id, &block.table_sort, font_size)
    } else {
        // Terminal output - only show cursor for running commands
        // For RUNNING blocks: use viewport-only grid (O(1) extraction)
        // For FINISHED blocks: use full scrollback (cached, O(1) after first extraction)
        // For alternate screen (TUI apps): always viewport only
        let grid = if block.parser.is_alternate_screen() || block.is_running() {
            // Running or alternate screen: viewport only (fast, O(1))
            block.parser.grid()
        } else {
            // Finished blocks: show all content including scrollback
            // This is cached after first extraction
            block.parser.grid_with_scrollback()
        };

        // Use GPU shader renderer for performance
        let content_rows = grid.content_rows() as usize;
        let (_cell_width, cell_height) = get_cell_metrics(font_size);
        TerminalShader::<Message>::new(&grid, font_size, 0, content_rows, cell_height)
            .widget()
            .into()
    };

    column![prompt_line, output].spacing(0).into()
}
