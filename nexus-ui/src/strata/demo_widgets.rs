//! Demo Widget Structs
//!
//! Thin data holders with `build()` methods that return Column/Row layout trees.
//! Each struct encapsulates the visual structure of a Nexus UI element.

use crate::strata::content_address::SourceId;
use crate::strata::layout::containers::{Column, Length, Padding, Row, TextElement, TerminalElement};
use crate::strata::primitives::Color;

use super::demo::colors;

// =========================================================================
// Shell Block
// =========================================================================

pub struct ShellBlock {
    pub cmd: &'static str,
    pub status_icon: &'static str,
    pub status_color: Color,
    pub terminal_source: SourceId,
    pub rows: Vec<(&'static str, Color)>,
    pub cols: u16,
    pub row_count: u16,
}

impl ShellBlock {
    pub fn build(self) -> Column {
        let mut terminal = TerminalElement::new(self.terminal_source, self.cols, self.row_count)
            .cell_size(8.4, 18.0);
        for (text, color) in self.rows {
            terminal = terminal.row(text, color);
        }

        // Header row: status icon + command, spacer, kill button
        let header = Row::new()
            .spacing(8.0)
            .cross_align(crate::strata::layout::containers::CrossAxisAlignment::Center)
            .text(
                TextElement::new(format!("{} $ {}", self.status_icon, self.cmd))
                    .color(self.status_color),
            )
            .spacer(1.0)
            .column(
                Column::new()
                    .padding_custom(Padding::new(2.0, 12.0, 2.0, 12.0))
                    .background(colors::BTN_KILL)
                    .corner_radius(4.0)
                    .text(TextElement::new("Kill").color(Color::WHITE)),
            );

        Column::new()
            .padding(12.0)
            .spacing(6.0)
            .background(colors::BG_BLOCK)
            .corner_radius(6.0)
            .width(Length::Fill)
            .row(header)
            .terminal(terminal)
    }
}

// =========================================================================
// Agent Block
// =========================================================================

pub struct ToolInvocation {
    pub icon: &'static str,
    pub status_icon: &'static str,
    pub label: &'static str,
    pub color: Color,
    pub expanded: bool,
    pub output_source: Option<SourceId>,
    pub output_rows: Vec<(&'static str, Color)>,
    pub output_cols: u16,
}

pub struct AgentBlock {
    pub query: &'static str,
    pub query_source: SourceId,
    pub tools: Vec<ToolInvocation>,
    pub response_lines: Vec<&'static str>,
    pub response_source: SourceId,
    pub status_text: &'static str,
    pub status_color: Color,
}

impl AgentBlock {
    pub fn build(self) -> Column {
        let mut content = Column::new()
            .padding(12.0)
            .spacing(6.0)
            .background(colors::BG_BLOCK)
            .corner_radius(6.0)
            .width(Length::Fill);

        // Query line: "? How do I parse JSON?"
        content = content.row(
            Row::new()
                .spacing(4.0)
                .text(TextElement::new("?").color(colors::TEXT_PURPLE))
                .text(
                    TextElement::new(self.query)
                        .source(self.query_source)
                        .color(colors::TEXT_QUERY),
                ),
        );

        // Tool invocations
        for tool in self.tools {
            let tool_text = format!("{} {} {}", tool.icon, tool.status_icon, tool.label);
            content = content.text(TextElement::new(tool_text).color(tool.color));

            if tool.expanded {
                if let Some(source_id) = tool.output_source {
                    let rows = tool.output_rows.len() as u16;
                    let mut term = TerminalElement::new(source_id, tool.output_cols, rows)
                        .cell_size(8.4, 18.0);
                    for (text, color) in tool.output_rows {
                        term = term.row(text, color);
                    }
                    // Indent the output
                    content = content.row(
                        Row::new()
                            .fixed_spacer(12.0)
                            .column(Column::new().terminal(term)),
                    );
                }
            }
        }

        // Response text
        let mut response = Column::new().spacing(4.0);
        for (i, line) in self.response_lines.iter().enumerate() {
            let elem = if i == 0 {
                TextElement::new(*line)
                    .source(self.response_source)
                    .color(colors::TEXT_PRIMARY)
            } else {
                TextElement::new(*line).color(colors::TEXT_PRIMARY)
            };
            response = response.text(elem);
        }
        content = content.fixed_spacer(4.0).column(response);

        // Status footer with stop button
        content = content.fixed_spacer(4.0).row(
            Row::new()
                .cross_align(crate::strata::layout::containers::CrossAxisAlignment::Center)
                .text(TextElement::new(self.status_text).color(self.status_color))
                .spacer(1.0)
                .column(
                    Column::new()
                        .padding_custom(Padding::new(2.0, 12.0, 2.0, 12.0))
                        .background(Color::rgba(0.5, 0.5, 0.5, 0.3))
                        .corner_radius(4.0)
                        .text(TextElement::new("Stop").color(colors::TEXT_MUTED)),
                ),
        );

        content
    }
}

// =========================================================================
// Permission Dialog
// =========================================================================

pub struct PermissionDialog {
    pub command: &'static str,
}

impl PermissionDialog {
    pub fn build(self) -> Column {
        // Code block
        let code_block = Column::new()
            .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
            .background(Color::rgba(0.0, 0.0, 0.0, 0.3))
            .corner_radius(4.0)
            .text(TextElement::new(self.command).color(colors::ERROR));

        // Buttons
        let buttons = Row::new()
            .spacing(8.0)
            .column(
                Column::new()
                    .padding_custom(Padding::new(3.0, 14.0, 3.0, 14.0))
                    .background(colors::BTN_DENY)
                    .corner_radius(4.0)
                    .text(TextElement::new("Deny").color(Color::WHITE)),
            )
            .column(
                Column::new()
                    .padding_custom(Padding::new(3.0, 14.0, 3.0, 14.0))
                    .background(colors::BTN_ALLOW)
                    .corner_radius(4.0)
                    .text(TextElement::new("Allow Once").color(Color::WHITE)),
            )
            .column(
                Column::new()
                    .padding_custom(Padding::new(3.0, 14.0, 3.0, 14.0))
                    .background(colors::BTN_ALWAYS)
                    .corner_radius(4.0)
                    .text(TextElement::new("Allow Always").color(Color::WHITE)),
            );

        Column::new()
            .padding(14.0)
            .spacing(8.0)
            .background(colors::BG_CARD)
            .corner_radius(8.0)
            .border(colors::BORDER_SUBTLE, 1.0)
            .shadow(16.0, Color::rgba(0.0, 0.0, 0.0, 0.6))
            .width(Length::Fill)
            .text(TextElement::new("\u{26A0} Permission Required").color(colors::WARNING))
            .text(TextElement::new("Allow tool to execute:").color(colors::TEXT_SECONDARY))
            .column(code_block)
            .row(buttons)
    }
}

// =========================================================================
// Input Bar
// =========================================================================

pub struct InputBar {
    pub cwd: &'static str,
    pub mode: &'static str,
    pub mode_color: Color,
    pub mode_bg: Color,
}

impl InputBar {
    pub fn build(self) -> Row {
        Row::new()
            .padding_custom(Padding::new(8.0, 12.0, 8.0, 12.0))
            .spacing(10.0)
            .background(colors::BG_INPUT)
            .corner_radius(6.0)
            .border(colors::BORDER_INPUT, 1.0)
            .width(Length::Fill)
            .cross_align(crate::strata::layout::containers::CrossAxisAlignment::Center)
            .text(TextElement::new(self.cwd).color(colors::TEXT_PATH))
            .column(
                Column::new()
                    .padding_custom(Padding::new(2.0, 10.0, 2.0, 10.0))
                    .background(self.mode_bg)
                    .corner_radius(12.0)
                    .text(TextElement::new(self.mode).color(self.mode_color)),
            )
            .text(TextElement::new("$").color(colors::SUCCESS))
            // Cursor block (rendered inline, moves with the widget)
            .column(
                Column::new()
                    .width(Length::Fixed(8.0))
                    .height(Length::Fixed(18.0))
                    .background(colors::CURSOR)
                    .corner_radius(1.0),
            )
    }
}

// =========================================================================
// Status Panel
// =========================================================================

pub struct StatusIndicator {
    pub icon: &'static str,
    pub label: &'static str,
    pub color: Color,
}

pub struct StatusPanel {
    pub indicators: Vec<StatusIndicator>,
}

impl StatusPanel {
    pub fn build(self) -> Column {
        let mut row = Row::new().spacing(16.0);
        for ind in self.indicators {
            row = row.text(
                TextElement::new(format!("{} {}", ind.icon, ind.label)).color(ind.color),
            );
        }

        Column::new()
            .padding(10.0)
            .spacing(6.0)
            .background(colors::BG_BLOCK)
            .corner_radius(6.0)
            .width(Length::Fill)
            .text(TextElement::new("Status Indicators").color(colors::TEXT_SECONDARY))
            .row(row)
    }
}

// =========================================================================
// Job Panel
// =========================================================================

pub struct JobPill {
    pub name: &'static str,
    pub prefix: &'static str,
    pub text_color: Color,
    pub bg_color: Color,
}

pub struct JobPanel {
    pub jobs: Vec<JobPill>,
}

impl JobPanel {
    pub fn build(self) -> Column {
        let mut row = Row::new().spacing(10.0);
        for job in self.jobs {
            row = row.column(
                Column::new()
                    .padding_custom(Padding::new(2.0, 12.0, 2.0, 12.0))
                    .background(job.bg_color)
                    .corner_radius(10.0)
                    .text(
                        TextElement::new(format!("{}{}", job.prefix, job.name))
                            .color(job.text_color),
                    ),
            );
        }

        Column::new()
            .padding(10.0)
            .spacing(6.0)
            .background(colors::BG_BLOCK)
            .corner_radius(6.0)
            .width(Length::Fill)
            .text(TextElement::new("Job Status").color(colors::TEXT_SECONDARY))
            .row(row)
    }
}
