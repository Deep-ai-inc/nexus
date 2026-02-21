//! Shell block widget.
//!
//! Renders a shell command block with terminal output, native values, and viewers.

use std::cell::RefCell;
use std::collections::HashMap;

use nexus_api::BlockState;

use crate::data::Block;
use crate::features::shell::ClickAction;
use crate::utils::ids;
use crate::ui::theme;
use super::{render_native_value, term_color_to_strata, TableLayoutCache};
use strata::content_address::SourceId;
use strata::gpu::ImageHandle;
use strata::layout::{
    ButtonElement, Column, CrossAxisAlignment, LayoutChild, Length, Row,
    TerminalElement, TextElement, Widget,
};
use strata::layout_snapshot::{RunStyle, TextRun, UnderlineStyle};
use strata::primitives::Color;

/// Message type for shell block interactions.
#[derive(Debug, Clone)]
pub enum ShellBlockMessage {
    Kill,
    ExitViewer,
    ToggleCollapse,
    AnchorClick(SourceId),
    TreeToggle(std::path::PathBuf),
}

/// Shell block widget — renders a command block with terminal output.
pub struct ShellBlockWidget<'a> {
    pub block: &'a Block,
    pub kill_id: SourceId,
    pub image_info: Option<(ImageHandle, u32, u32)>,
    pub is_focused: bool,
    /// Unified click registry — populated during rendering so click/drag
    /// handling can do O(1) lookups without re-iterating the Value tree.
    pub(crate) click_registry: &'a RefCell<HashMap<SourceId, ClickAction>>,
    /// Table geometry cache — populated during rendering for cell hit-testing.
    pub(crate) table_layout_cache: &'a TableLayoutCache,
    /// Per-cell image handles for table cells with image media.
    pub(crate) table_cell_images: &'a HashMap<(nexus_api::BlockId, usize, usize), (ImageHandle, u32, u32)>,
}

impl<'a> Widget<'a> for ShellBlockWidget<'a> {
    fn build(self) -> LayoutChild<'a> {
        let block = self.block;
        let header_source = ids::shell_header(block.id);

        // Extract terminal content from parser.
        let grid = if block.parser.is_alternate_screen() {
            block.parser.grid()
        } else {
            block.parser.grid_with_scrollback()
        };
        let content_rows = debounced_content_rows(block, &grid);
        let cols = grid.cols();

        let mut content = Column::new()
            .id(ids::block_container(block.id))
            .padding(6.0)
            .spacing(4.0)
            .background(theme::BG_BLOCK)
            .corner_radius(4.0)
            .width(Length::Fill);

        if self.is_focused {
            content = content.border(Color::rgb(0.3, 0.7, 1.0), 2.0);
        }

        content = content.push(build_header(block, self.kill_id, header_source));

        // Render output: live_value replaces structured_output when present (e.g. top),
        // otherwise show structured_output (e.g. ls, git status).
        if let Some(ref latest) = block.live_value {
            content = render_native_value(content, latest, block, self.image_info, self.click_registry, self.table_layout_cache, self.table_cell_images);
        } else if let Some(value) = &block.structured_output {
            content = render_native_value(content, value, block, self.image_info, self.click_registry, self.table_layout_cache, self.table_cell_images);
        }

        content = build_event_log(content, block, self.image_info, self.click_registry, self.table_layout_cache, self.table_cell_images);

        if block.structured_output.is_none() && block.live_value.is_none() && block.event_log.is_empty() && content_rows > 0 {
            content = build_terminal_content(content, block, &grid, cols, content_rows);
        }

        // Exit code indicator for failed commands
        match block.state {
            BlockState::Failed(code) => {
                content = content.push(
                    TextElement::new(format!("exit {}", code)).color(theme::ERROR)
                        .source(header_source),
                );
            }
            _ => {}
        }

        content.into()
    }
}

// ---------------------------------------------------------------------------
// Build helpers
// ---------------------------------------------------------------------------

/// Header row: status icon + command + [Kill/Exit/duration].
fn build_header<'a>(block: &Block, kill_id: SourceId, header_source: SourceId) -> Row<'a> {
    let (status_icon, status_color) = match block.state {
        BlockState::Running => ("\u{25CF}", theme::RUNNING),
        BlockState::Success => ("\u{2713}", theme::SUCCESS),
        BlockState::Failed(_) => ("\u{2717}", theme::ERROR),
    };

    let mut header = Row::new()
        .spacing(8.0)
        .cross_align(CrossAxisAlignment::Center)
        .push(
            TextElement::new(format!("{} $ {}", status_icon, block.command))
                .color(status_color)
                .source(header_source),
        )
        .spacer(1.0);

    if block.is_running() {
        header = header.push(
            ButtonElement::new(kill_id, "Kill")
                .background(theme::BTN_KILL)
                .corner_radius(4.0),
        );
    } else if block.view_state.is_some() {
        let exit_id = ids::viewer_exit(block.id);
        header = header.push(
            ButtonElement::new(exit_id, "Exit")
                .background(theme::BTN_KILL)
                .corner_radius(4.0),
        );
    } else if let Some(ms) = block.duration_ms {
        let duration = if ms < 1000 {
            format!("{}ms", ms)
        } else {
            format!("{:.1}s", ms as f64 / 1000.0)
        };
        header = header.push(TextElement::new(duration).color(theme::TEXT_MUTED));
    }

    header
}

/// Debounce shrink for running non-alt-screen blocks to mask clear+reprint flicker.
fn debounced_content_rows(block: &Block, grid: &nexus_term::TerminalGrid) -> u16 {
    let content_rows = grid.content_rows();
    if block.is_running() && !block.parser.is_alternate_screen() {
        let peak = block.peak_content_rows.load(std::sync::atomic::Ordering::Relaxed);
        if content_rows >= peak {
            block.peak_content_rows.store(content_rows, std::sync::atomic::Ordering::Relaxed);
            content_rows
        } else if content_rows < peak / 2 {
            // Cap at grid's actual row count — the PTY app may have cleared
            // scrollback, shrinking the grid below the old peak.
            let capped = peak.min(grid.rows());
            block.peak_content_rows.store(capped, std::sync::atomic::Ordering::Relaxed);
            capped
        } else {
            block.peak_content_rows.store(content_rows, std::sync::atomic::Ordering::Relaxed);
            content_rows
        }
    } else {
        block.peak_content_rows.store(0, std::sync::atomic::Ordering::Relaxed);
        content_rows
    }
}

/// Render stream log: collapse history into a single text block, render latest as full widget.
fn build_event_log<'a>(
    mut content: Column<'a>,
    block: &'a Block,
    image_info: Option<(ImageHandle, u32, u32)>,
    click_registry: &'a RefCell<HashMap<SourceId, ClickAction>>,
    table_layout_cache: &'a TableLayoutCache,
    table_cell_images: &'a HashMap<(nexus_api::BlockId, usize, usize), (ImageHandle, u32, u32)>,
) -> Column<'a> {
    if block.event_log.is_empty() {
        return content;
    }

    let source_id = ids::native(block.id);
    let visible_count = 50.min(block.event_log.len());
    let start = block.event_log.len() - visible_count;

    if visible_count > 1 {
        let mut history_text = String::new();
        for entry in block.event_log.iter().skip(start).take(visible_count - 1) {
            if !history_text.is_empty() {
                history_text.push('\n');
            }
            history_text.push_str(&entry.to_text());
        }
        content = content.push(
            TextElement::new(history_text)
                .color(theme::TEXT_MUTED)
                .source(source_id),
        );
    }

    if let Some(latest) = block.event_log.back() {
        content = render_native_value(content, latest, block, image_info, click_registry, table_layout_cache, table_cell_images);
    }

    content
}

/// Flush a pending text run into the runs vector.
fn flush_run(
    runs: &mut Vec<TextRun>,
    text: &mut String,
    fg: u32,
    bg: u32,
    col_offset: u16,
    cells: &mut u16,
    style: RunStyle,
) {
    if !text.is_empty() {
        runs.push(TextRun {
            text: std::mem::take(text),
            fg,
            bg,
            col_offset,
            cell_len: *cells,
            style,
        });
        *cells = 0;
    }
}

/// Build terminal output element from the parser grid.
fn build_terminal_content<'a>(
    content: Column<'a>,
    block: &'a Block,
    grid: &nexus_term::TerminalGrid,
    cols: u16,
    content_rows: u16,
) -> Column<'a> {
    let source_id = ids::shell_term(block.id);
    let cursor_info = if block.is_running() && grid.cursor_shape() != nexus_term::CursorShape::Hidden {
        let (c, r) = grid.cursor();
        if r < content_rows {
            let cell = grid.get(c, r);
            let (ch, fg, bg) = if let Some(cell) = cell {
                let fg = term_color_to_strata(cell.fg).pack();
                let bg = if matches!(cell.bg, nexus_term::Color::Default) {
                    0u32
                } else {
                    term_color_to_strata(cell.bg).pack()
                };
                (cell.c, fg, bg)
            } else {
                (' ', Color::rgb(0.9, 0.9, 0.9).pack(), 0u32)
            };
            use strata::layout_snapshot::{GridCursor, GridCursorShape};
            let shape = match grid.cursor_shape() {
                nexus_term::CursorShape::Block => GridCursorShape::Block,
                nexus_term::CursorShape::HollowBlock => GridCursorShape::HollowBlock,
                nexus_term::CursorShape::Beam => GridCursorShape::Beam,
                nexus_term::CursorShape::Underline => GridCursorShape::Underline,
                nexus_term::CursorShape::Hidden => unreachable!(),
            };
            Some(GridCursor { col: c, row: r, shape, ch, fg, bg })
        } else {
            None
        }
    } else {
        None
    };
    let mut term = TerminalElement::new(source_id, cols, content_rows)
        .cell_size(8.4, 18.0)
        .cursor(cursor_info);

    let default_fg_packed = Color::rgb(0.9, 0.9, 0.9).pack();
    let default_bg_packed: u32 = 0;

    for row in grid.rows_iter() {
        let mut runs: Vec<TextRun> = Vec::new();
        let mut run_text = String::new();
        let mut run_fg: u32 = default_fg_packed;
        let mut run_bg: u32 = default_bg_packed;
        let mut run_style = RunStyle::default();
        let mut run_col: u16 = 0;
        let mut run_cells: u16 = 0;
        let mut col: u16 = 0;

        for cell in row {
            if cell.flags.wide_char_spacer {
                continue;
            }

            if cell.flags.hidden {
                flush_run(&mut runs, &mut run_text, run_fg, run_bg, run_col, &mut run_cells, run_style);
                col += if cell.flags.wide_char { 2 } else { 1 };
                run_col = col;
                continue;
            }

            let cell_width: u16 = if cell.flags.wide_char { 2 } else { 1 };

            let (fg_packed, bg_packed) = if cell.flags.inverse {
                let resolved_fg = if matches!(cell.fg, nexus_term::Color::Default) {
                    Color::rgb(0.9, 0.9, 0.9)
                } else {
                    term_color_to_strata(cell.fg)
                };
                let resolved_bg = if matches!(cell.bg, nexus_term::Color::Default) {
                    Color::rgb(0.12, 0.12, 0.12)
                } else {
                    term_color_to_strata(cell.bg)
                };
                (resolved_bg.pack(), resolved_fg.pack())
            } else {
                let fg = term_color_to_strata(cell.fg).pack();
                let bg = if matches!(cell.bg, nexus_term::Color::Default) {
                    0u32
                } else {
                    term_color_to_strata(cell.bg).pack()
                };
                (fg, bg)
            };
            let style = RunStyle {
                bold: cell.flags.bold,
                italic: cell.flags.italic,
                underline: match cell.flags.underline {
                    nexus_term::UnderlineStyle::None => UnderlineStyle::None,
                    nexus_term::UnderlineStyle::Single => UnderlineStyle::Single,
                    nexus_term::UnderlineStyle::Double => UnderlineStyle::Double,
                    nexus_term::UnderlineStyle::Curly => UnderlineStyle::Curly,
                    nexus_term::UnderlineStyle::Dotted => UnderlineStyle::Dotted,
                    nexus_term::UnderlineStyle::Dashed => UnderlineStyle::Dashed,
                },
                strikethrough: cell.flags.strikethrough,
                dim: cell.flags.dim,
            };

            let same_attrs = fg_packed == run_fg && bg_packed == run_bg && style == run_style;

            if !same_attrs {
                flush_run(&mut runs, &mut run_text, run_fg, run_bg, run_col, &mut run_cells, run_style);
                run_col = col;
                run_fg = fg_packed;
                run_bg = bg_packed;
                run_style = style;
            } else if run_text.is_empty() {
                run_col = col;
                run_fg = fg_packed;
                run_bg = bg_packed;
                run_style = style;
            }

            cell.push_grapheme(&mut run_text);
            run_cells += cell_width;
            col += cell_width;
        }

        flush_run(&mut runs, &mut run_text, run_fg, run_bg, run_col, &mut run_cells, run_style);
        term = term.row(runs);
    }

    content.terminal(term)
}

// =========================================================================
// Click handling
// =========================================================================

impl ShellBlockWidget<'_> {
    /// Try to translate a click on the given SourceId into a ShellBlockMessage.
    /// Returns None if the click doesn't belong to this block's widgets.
    ///
    /// Note: Viewer exit buttons are handled separately in event_routing.rs
    /// as they map to ViewerMsg, not ShellMsg.
    pub fn on_click(block: &Block, id: SourceId) -> Option<ShellBlockMessage> {
        // Kill button
        if block.is_running() && id == ids::kill(block.id) {
            return Some(ShellBlockMessage::Kill);
        }
        None
    }
}
