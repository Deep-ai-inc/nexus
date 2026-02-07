//! Shell block widget.
//!
//! Renders a shell command block with terminal output, native values, and viewers.

use std::cell::RefCell;
use std::collections::HashMap;

use nexus_api::BlockState;

use crate::blocks::Block;
use crate::nexus_app::shell::ClickAction;
use crate::nexus_app::source_ids;
use crate::nexus_app::colors;
use crate::nexus_widgets::{render_native_value, term_color_to_strata};
use strata::content_address::SourceId;
use strata::gpu::ImageHandle;
use strata::layout::{
    ButtonElement, Column, CrossAxisAlignment, LayoutChild, Length, Row,
    TerminalElement, TextElement, Widget,
};
use strata::layout_snapshot::{RunStyle, TextRun, UnderlineStyle};
use strata::primitives::Color;

// Schema for shell block source IDs.
// These match the constants in source_ids.rs for consistency.
pub mod id {
    pub const HEADER: u64 = 1;
    pub const TERMINAL: u64 = 2;
    pub const NATIVE_OUTPUT: u64 = 3;
    pub const KILL_BUTTON: u64 = 4;
    pub const EXIT_BUTTON: u64 = 5;
    pub const DURATION: u64 = 6;
}

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
}

impl<'a> Widget<'a> for ShellBlockWidget<'a> {
    fn build(self) -> LayoutChild<'a> {
        let block = self.block;

        // Status icon and color
        let (status_icon, status_color) = match block.state {
            BlockState::Running => ("\u{25CF}", colors::RUNNING),    // ●
            BlockState::Success => ("\u{2713}", colors::SUCCESS),    // ✓
            BlockState::Failed(_) => ("\u{2717}", colors::ERROR),    // ✗
            BlockState::Killed(_) => ("\u{2717}", colors::ERROR),    // ✗
        };

        // Header row: status + command + [Kill/duration]
        let header_source = source_ids::shell_header(block.id);
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
            // Kill button
            header = header.push(
                ButtonElement::new(self.kill_id, "Kill")
                    .background(colors::BTN_KILL)
                    .corner_radius(4.0),
            );
        } else if block.view_state.is_some() {
            // Exit button for active viewers (top, less, man, tree)
            let exit_id = source_ids::viewer_exit(block.id);
            header = header.push(
                ButtonElement::new(exit_id, "Exit")
                    .background(colors::BTN_KILL)
                    .corner_radius(4.0),
            );
        } else if let Some(ms) = block.duration_ms {
            let duration = if ms < 1000 {
                format!("{}ms", ms)
            } else {
                format!("{:.1}s", ms as f64 / 1000.0)
            };
            header = header.push(TextElement::new(duration).color(colors::TEXT_MUTED));
        }

        // Extract terminal content from parser.
        // Alt-screen apps (vim, htop) get viewport only; normal-screen apps
        // (including running ones like Claude Code) get full scrollback.
        let grid = if block.parser.is_alternate_screen() {
            block.parser.grid()
        } else {
            block.parser.grid_with_scrollback()
        };
        let content_rows = grid.content_rows();

        // Debounce shrink for running non-alt-screen blocks to mask
        // clear+reprint flicker (e.g. Claude Code doing \x1b[3J + \x1b[2J).
        let content_rows = if block.is_running() && !block.parser.is_alternate_screen() {
            let peak = block.peak_content_rows.load(std::sync::atomic::Ordering::Relaxed);
            if content_rows >= peak {
                block.peak_content_rows.store(content_rows, std::sync::atomic::Ordering::Relaxed);
                content_rows
            } else if content_rows < peak / 2 {
                // Dramatic shrink (clear+reprint mid-cycle): hold at peak
                peak
            } else {
                // Moderate shrink (real content reduction): follow it
                block.peak_content_rows.store(content_rows, std::sync::atomic::Ordering::Relaxed);
                content_rows
            }
        } else {
            block.peak_content_rows.store(0, std::sync::atomic::Ordering::Relaxed);
            content_rows
        };

        let cols = grid.cols();

        let mut content = Column::new()
            .id(source_ids::block_container(block.id))
            .padding(6.0)
            .spacing(4.0)
            .background(colors::BG_BLOCK)
            .corner_radius(4.0)
            .width(Length::Fill);

        if self.is_focused {
            content = content.border(Color::rgb(0.3, 0.7, 1.0), 2.0);
        }

        content = content.push(header);

        // Render output: stream_latest replaces native_output when present (e.g. top),
        // otherwise show native_output (e.g. ls, git status).
        if let Some(ref latest) = block.stream_latest {
            content = render_native_value(content, latest, block, self.image_info, self.click_registry);
        } else if let Some(value) = &block.native_output {
            content = render_native_value(content, value, block, self.image_info, self.click_registry);
        }

        // Render stream log: collapse history into a single text block for performance,
        // only render the latest entry as a full widget.
        if !block.stream_log.is_empty() {
            let source_id = source_ids::native(block.id);
            let visible_count = 50.min(block.stream_log.len());
            let start = block.stream_log.len() - visible_count;

            // History entries → single pre-rendered text element (cheap to layout)
            if visible_count > 1 {
                let mut history_text = String::new();
                for entry in block.stream_log.iter().skip(start).take(visible_count - 1) {
                    if !history_text.is_empty() {
                        history_text.push('\n');
                    }
                    history_text.push_str(&entry.to_text());
                }
                content = content.push(
                    TextElement::new(history_text)
                        .color(colors::TEXT_MUTED)
                        .source(source_id),
                );
            }

            // Latest entry → full widget rendering (may have colors, structure)
            if let Some(latest) = block.stream_log.back() {
                content = render_native_value(content, latest, block, self.image_info, self.click_registry);
            }
        }

        if block.native_output.is_none() && block.stream_latest.is_none() && block.stream_log.is_empty() && content_rows > 0 {
            let source_id = source_ids::shell_term(block.id);
            let mut term = TerminalElement::new(source_id, cols, content_rows)
                .cell_size(8.4, 18.0);

            // Extract styled text runs from the grid
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

                // Flush helper: pushes current run if non-empty
                macro_rules! flush_run {
                    ($runs:expr, $text:expr, $fg:expr, $bg:expr, $col:expr, $cells:expr, $style:expr) => {
                        if !$text.is_empty() {
                            $runs.push(TextRun {
                                text: std::mem::take(&mut $text),
                                fg: $fg,
                                bg: $bg,
                                col_offset: $col,
                                cell_len: $cells,
                                style: $style,
                            });
                            #[allow(unused_assignments)]
                            { $cells = 0; }
                        }
                    };
                }

                for cell in row {
                    if cell.flags.wide_char_spacer {
                        continue;
                    }

                    // Hidden cells: flush current run and skip (creates a gap)
                    if cell.flags.hidden {
                        flush_run!(runs, run_text, run_fg, run_bg, run_col, run_cells, run_style);
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

                    // Check if this cell continues the current run (packed u32 comparison)
                    let same_attrs = fg_packed == run_fg && bg_packed == run_bg && style == run_style;

                    if !same_attrs {
                        flush_run!(runs, run_text, run_fg, run_bg, run_col, run_cells, run_style);
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

                // Flush last run
                flush_run!(runs, run_text, run_fg, run_bg, run_col, run_cells, run_style);

                term = term.row(runs);
            }

            content = content.terminal(term);
        }

        // Exit code indicator for failed commands
        match block.state {
            BlockState::Failed(code) | BlockState::Killed(code) => {
                content = content.push(
                    TextElement::new(format!("exit {}", code)).color(colors::ERROR)
                        .source(header_source),
                );
            }
            _ => {}
        }

        content.into()
    }
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
        if block.is_running() && id == source_ids::kill(block.id) {
            return Some(ShellBlockMessage::Kill);
        }
        None
    }
}
