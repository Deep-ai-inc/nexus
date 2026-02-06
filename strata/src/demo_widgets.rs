//! Demo Widget Structs
//!
//! Reusable UI components implementing the `Widget` trait. Each struct builds
//! a layout tree from existing primitives (Column, Row, etc.) with zero heap
//! allocation. Use `.push(MyWidget { ... })` on any container.

use crate::content_address::SourceId;
use crate::layout::{
    ButtonElement, Column, LayoutChild, Length, Padding, Row, TextElement, TerminalElement, Widget,
};
use crate::primitives::Color;

use super::demo::colors;

// =========================================================================
// Card — reusable styled panel with title
// =========================================================================

/// A titled card with consistent padding, background, and corner radius.
///
/// Wraps a `Column` internally — accepts any child via `.push()`.
///
/// # Example
/// ```ignore
/// Card::new("Settings")
///     .push(TextElement::new("Some setting"))
///     .push(toggle_button)
///     .id(SourceId::named("settings_card"))
/// ```
pub struct Card<'a> {
    inner: Column<'a>,
}

impl<'a> Card<'a> {
    pub fn new(title: &str) -> Self {
        Card {
            inner: Column::new()
                .padding(10.0)
                .spacing(6.0)
                .background(colors::BG_BLOCK)
                .corner_radius(6.0)
                .width(Length::Fill)
                .push(TextElement::new(title).color(colors::TEXT_SECONDARY)),
        }
    }

    pub fn id(mut self, id: SourceId) -> Self {
        self.inner = self.inner.id(id);
        self
    }

    pub fn push(mut self, child: impl Into<LayoutChild<'a>>) -> Self {
        self.inner = self.inner.push(child);
        self
    }
}

impl<'a> Widget<'a> for Card<'a> {
    fn build(self) -> LayoutChild<'a> {
        self.inner.into()
    }
}

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

impl Widget<'static> for ShellBlock {
    fn build(self) -> LayoutChild<'static> {
        let mut terminal = TerminalElement::new(self.terminal_source, self.cols, self.row_count)
            .cell_size(8.4, 18.0);
        for (text, color) in self.rows {
            terminal = terminal.row(vec![crate::layout_snapshot::TextRun {
                text: text.to_string(),
                fg: color.pack(),
                bg: 0,
                col_offset: 0,
                cell_len: text.len() as u16,
                style: crate::layout_snapshot::RunStyle::default(),
            }]);
        }

        let header = Row::new()
            .spacing(8.0)
            .cross_align(crate::layout::CrossAxisAlignment::Center)
            .push(
                TextElement::new(format!("{} $ {}", self.status_icon, self.cmd))
                    .color(self.status_color),
            )
            .spacer(1.0)
            .push(
                Column::new()
                    .padding_custom(Padding::new(2.0, 12.0, 2.0, 12.0))
                    .background(colors::BTN_KILL)
                    .corner_radius(4.0)
                    .push(TextElement::new("Kill").color(Color::WHITE)),
            );

        Column::new()
            .padding(12.0)
            .spacing(6.0)
            .background(colors::BG_BLOCK)
            .corner_radius(6.0)
            .width(Length::Fill)
            .push(header)
            .terminal(terminal)
            .into()
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

impl Widget<'static> for AgentBlock {
    fn build(self) -> LayoutChild<'static> {
        let mut content = Column::new()
            .padding(12.0)
            .spacing(6.0)
            .background(colors::BG_BLOCK)
            .corner_radius(6.0)
            .width(Length::Fill);

        // Query line
        content = content.push(
            Row::new()
                .spacing(4.0)
                .push(TextElement::new("?").color(colors::TEXT_PURPLE))
                .push(
                    TextElement::new(self.query)
                        .source(self.query_source)
                        .color(colors::TEXT_QUERY),
                ),
        );

        // Tool invocations
        for tool in self.tools {
            let tool_text = format!("{} {} {}", tool.icon, tool.status_icon, tool.label);
            content = content.push(TextElement::new(tool_text).color(tool.color));

            if tool.expanded {
                if let Some(source_id) = tool.output_source {
                    let rows = tool.output_rows.len() as u16;
                    let mut term = TerminalElement::new(source_id, tool.output_cols, rows)
                        .cell_size(8.4, 18.0);
                    for (text, color) in tool.output_rows {
                        term = term.row(vec![crate::layout_snapshot::TextRun {
                            text: text.to_string(),
                            fg: color.pack(),
                            bg: 0,
                            col_offset: 0,
                            cell_len: text.len() as u16,
                            style: crate::layout_snapshot::RunStyle::default(),
                        }]);
                    }
                    content = content.push(
                        Row::new()
                            .fixed_spacer(12.0)
                            .push(Column::new().terminal(term)),
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
            response = response.push(elem);
        }
        content = content.fixed_spacer(4.0).push(response);

        // Status footer
        content = content.fixed_spacer(4.0).push(
            Row::new()
                .cross_align(crate::layout::CrossAxisAlignment::Center)
                .push(TextElement::new(self.status_text).color(self.status_color))
                .spacer(1.0)
                .push(
                    Column::new()
                        .padding_custom(Padding::new(2.0, 12.0, 2.0, 12.0))
                        .background(Color::rgba(0.5, 0.5, 0.5, 0.3))
                        .corner_radius(4.0)
                        .push(TextElement::new("Stop").color(colors::TEXT_MUTED)),
                ),
        );

        content.into()
    }
}

// =========================================================================
// Permission Dialog
// =========================================================================

pub struct PermissionDialog {
    pub command: &'static str,
    pub deny_id: SourceId,
    pub allow_id: SourceId,
    pub always_id: SourceId,
}

impl Widget<'static> for PermissionDialog {
    fn build(self) -> LayoutChild<'static> {
        let code_block = Column::new()
            .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
            .background(Color::rgba(0.0, 0.0, 0.0, 0.3))
            .corner_radius(4.0)
            .push(TextElement::new(self.command).color(colors::ERROR));

        let buttons = Row::new()
            .spacing(8.0)
            .push(
                ButtonElement::new(self.deny_id, "Deny")
                    .background(colors::BTN_DENY)
                    .corner_radius(4.0),
            )
            .push(
                ButtonElement::new(self.allow_id, "Allow Once")
                    .background(colors::BTN_ALLOW)
                    .corner_radius(4.0),
            )
            .push(
                ButtonElement::new(self.always_id, "Allow Always")
                    .background(colors::BTN_ALWAYS)
                    .corner_radius(4.0),
            );

        Column::new()
            .padding(14.0)
            .spacing(8.0)
            .background(colors::BG_CARD)
            .corner_radius(8.0)
            .border(colors::BORDER_SUBTLE, 1.0)
            .shadow(16.0, Color::rgba(0.0, 0.0, 0.0, 0.6))
            .width(Length::Fill)
            .push(TextElement::new("\u{26A0} Permission Required").color(colors::WARNING))
            .push(TextElement::new("Allow tool to execute:").color(colors::TEXT_SECONDARY))
            .push(code_block)
            .push(buttons)
            .into()
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

impl Widget<'static> for InputBar {
    fn build(self) -> LayoutChild<'static> {
        Row::new()
            .padding_custom(Padding::new(8.0, 12.0, 8.0, 12.0))
            .spacing(10.0)
            .background(colors::BG_INPUT)
            .corner_radius(6.0)
            .border(colors::BORDER_INPUT, 1.0)
            .width(Length::Fill)
            .cross_align(crate::layout::CrossAxisAlignment::Center)
            .push(TextElement::new(self.cwd).color(colors::TEXT_PATH))
            .push(
                Column::new()
                    .padding_custom(Padding::new(2.0, 10.0, 2.0, 10.0))
                    .background(self.mode_bg)
                    .corner_radius(12.0)
                    .push(TextElement::new(self.mode).color(self.mode_color)),
            )
            .push(TextElement::new("$").color(colors::SUCCESS))
            .push(
                Column::new()
                    .width(Length::Fixed(8.0))
                    .height(Length::Fixed(18.0))
                    .background(colors::CURSOR)
                    .corner_radius(1.0),
            )
            .into()
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
    indicators: Vec<StatusIndicator>,
    uptime_seconds: u32,
    id: Option<SourceId>,
}

impl StatusPanel {
    pub fn new(indicators: Vec<StatusIndicator>, uptime_seconds: u32) -> Self {
        StatusPanel { indicators, uptime_seconds, id: None }
    }

    pub fn id(mut self, id: SourceId) -> Self {
        self.id = Some(id);
        self
    }
}

impl Widget<'static> for StatusPanel {
    fn build(self) -> LayoutChild<'static> {
        let mut row = Row::new().spacing(16.0);
        for ind in self.indicators {
            row = row.push(
                TextElement::new(format!("{} {}", ind.icon, ind.label)).color(ind.color),
            );
        }

        let uptime = format!("Uptime: {}s", self.uptime_seconds);

        let mut col = Column::new()
            .padding(10.0)
            .spacing(6.0)
            .background(colors::BG_BLOCK)
            .corner_radius(6.0)
            .width(Length::Fill)
            .push(TextElement::new("Status Indicators").color(colors::TEXT_SECONDARY))
            .push(row)
            .push(TextElement::new(uptime).color(colors::TEXT_MUTED));

        if let Some(id) = self.id {
            col = col.id(id);
        }

        col.into()
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

impl Widget<'static> for JobPanel {
    fn build(self) -> LayoutChild<'static> {
        let mut row = Row::new().spacing(10.0);
        for job in self.jobs {
            row = row.push(
                Column::new()
                    .padding_custom(Padding::new(2.0, 12.0, 2.0, 12.0))
                    .background(job.bg_color)
                    .corner_radius(10.0)
                    .push(
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
            .push(TextElement::new("Job Status").color(colors::TEXT_SECONDARY))
            .push(row)
            .into()
    }
}
