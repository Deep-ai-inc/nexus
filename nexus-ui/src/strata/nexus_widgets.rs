//! Nexus Widget Structs for Strata
//!
//! Production UI components that render real Nexus data (Block, AgentBlock, etc.)
//! using Strata's layout primitives. Each widget takes references to backend
//! data models and builds a layout tree.

use nexus_api::{BlockId, BlockState, FileType, Value};

use crate::agent_block::{AgentBlock, AgentBlockState, ToolInvocation, ToolStatus};
use crate::blocks::Block;
use crate::strata::content_address::SourceId;
use crate::strata::layout::containers::{
    ButtonElement, Column, CrossAxisAlignment, LayoutChild, Length, Padding, Row, TableCell,
    TableElement, TerminalElement, TextElement, Widget,
};
use crate::strata::primitives::Color;
use crate::widgets::job_indicator::{VisualJob, VisualJobState};

use super::nexus_app::colors;

// =========================================================================
// Shell Block Widget — renders a real Block with TerminalParser data
// =========================================================================

pub struct ShellBlockWidget<'a> {
    pub block: &'a Block,
    pub kill_id: SourceId,
}

impl Widget for ShellBlockWidget<'_> {
    fn build(self) -> LayoutChild {
        let block = self.block;

        // Status icon and color
        let (status_icon, status_color) = match block.state {
            BlockState::Running => ("\u{25CF}", colors::RUNNING),    // ●
            BlockState::Success => ("\u{2713}", colors::SUCCESS),    // ✓
            BlockState::Failed(_) => ("\u{2717}", colors::ERROR),    // ✗
            BlockState::Killed(_) => ("\u{2717}", colors::ERROR),   // ✗
        };

        // Header row: status + command + [Kill/duration]
        let mut header = Row::new()
            .spacing(8.0)
            .cross_align(CrossAxisAlignment::Center)
            .push(
                TextElement::new(format!("{} $ {}", status_icon, block.command))
                    .color(status_color),
            )
            .spacer(1.0);

        if block.is_running() {
            // Kill button
            header = header.push(
                ButtonElement::new(self.kill_id, "Kill")
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

        // Extract terminal content from parser
        let grid = if block.parser.is_alternate_screen() || block.is_running() {
            block.parser.grid()
        } else {
            block.parser.grid_with_scrollback()
        };
        let content_rows = grid.content_rows();
        let cols = grid.cols();

        let mut content = Column::new()
            .padding(12.0)
            .spacing(6.0)
            .background(colors::BG_BLOCK)
            .corner_radius(6.0)
            .width(Length::Fill)
            .push(header);

        // Render output: native structured data takes priority over terminal
        if let Some(value) = &block.native_output {
            content = render_native_value(content, value, block.id);
        } else if content_rows > 0 {
            let source_id = SourceId::named(&format!("shell_term_{}", block.id.0));
            let mut term = TerminalElement::new(source_id, cols, content_rows)
                .cell_size(8.4, 18.0);

            // Extract text rows from the grid
            for row in grid.rows_iter() {
                let text: String = row.iter().map(|cell| cell.c).collect();
                // Use cell foreground color from first non-default cell, or default
                let fg = row.iter()
                    .find(|c| !matches!(c.fg, nexus_term::Color::Default))
                    .map(|c| term_color_to_strata(c.fg))
                    .unwrap_or(Color::rgb(0.8, 0.8, 0.8));
                term = term.row(&text, fg);
            }

            content = content.terminal(term);
        }

        // Exit code indicator for failed commands
        match block.state {
            BlockState::Failed(code) | BlockState::Killed(code) => {
                content = content.push(
                    TextElement::new(format!("exit {}", code)).color(colors::ERROR),
                );
            }
            _ => {}
        }

        content.into()
    }
}

// =========================================================================
// Agent Block Widget — renders a real AgentBlock
// =========================================================================

pub struct AgentBlockWidget<'a> {
    pub block: &'a AgentBlock,
    pub thinking_toggle_id: SourceId,
    pub stop_id: SourceId,
}

impl<'a> AgentBlockWidget<'a> {
    /// Generate a stable SourceId for a tool toggle button.
    fn tool_toggle_id(block_id: BlockId, tool_index: usize) -> SourceId {
        SourceId::named(&format!("tool_toggle_{}_{}", block_id.0, tool_index))
    }

    /// Generate a stable SourceId for permission buttons.
    fn perm_deny_id(block_id: BlockId) -> SourceId {
        SourceId::named(&format!("perm_deny_{}", block_id.0))
    }
    fn perm_allow_id(block_id: BlockId) -> SourceId {
        SourceId::named(&format!("perm_allow_{}", block_id.0))
    }
    fn perm_always_id(block_id: BlockId) -> SourceId {
        SourceId::named(&format!("perm_always_{}", block_id.0))
    }
}

impl Widget for AgentBlockWidget<'_> {
    fn build(self) -> LayoutChild {
        let block = self.block;

        let mut content = Column::new()
            .padding(12.0)
            .spacing(4.0)
            .background(colors::BG_BLOCK)
            .corner_radius(6.0)
            .width(Length::Fill);

        // Query line
        content = content.push(
            Row::new()
                .spacing(4.0)
                .push(TextElement::new("? ").color(colors::TEXT_PURPLE))
                .push(TextElement::new(&block.query).color(colors::TEXT_QUERY)),
        );

        // Thinking section
        if !block.thinking.is_empty() {
            let collapse_icon = if block.thinking_collapsed { "\u{25B6}" } else { "\u{25BC}" };
            content = content.push(
                ButtonElement::new(self.thinking_toggle_id, &format!("{} Thinking...", collapse_icon))
                    .background(Color::TRANSPARENT)
                    .text_color(colors::TEXT_MUTED)
                    .corner_radius(2.0),
            );

            if !block.thinking_collapsed {
                // Show thinking text indented
                let thinking_preview = if block.thinking.len() > 500 {
                    format!("{}...", &block.thinking[..500])
                } else {
                    block.thinking.clone()
                };
                for line in thinking_preview.lines() {
                    content = content.push(
                        Row::new()
                            .fixed_spacer(16.0)
                            .push(TextElement::new(line).color(colors::THINKING)),
                    );
                }
            }
        }

        // Tool invocations
        for (i, tool) in block.tools.iter().enumerate() {
            let toggle_id = Self::tool_toggle_id(block.id, i);
            content = content.push(build_tool_widget(tool, toggle_id));
        }

        // Permission dialog
        if let Some(ref perm) = block.pending_permission {
            content = content.push(build_permission_dialog(
                perm,
                Self::perm_deny_id(block.id),
                Self::perm_allow_id(block.id),
                Self::perm_always_id(block.id),
            ));
        }

        // Response text
        if !block.response.is_empty() {
            content = content.push(build_response_text(&block.response));
        }

        // Status footer
        let (status_text, status_color) = match &block.state {
            AgentBlockState::Pending => ("Waiting...", colors::TEXT_MUTED),
            AgentBlockState::Streaming => ("Streaming...", colors::RUNNING),
            AgentBlockState::Thinking => ("Thinking...", colors::THINKING),
            AgentBlockState::Executing => ("Executing...", colors::RUNNING),
            AgentBlockState::Completed => ("Completed", colors::SUCCESS),
            AgentBlockState::Failed(err) => (err.as_str(), colors::ERROR),
            AgentBlockState::AwaitingPermission => ("Awaiting permission...", colors::WARNING),
            AgentBlockState::Interrupted => ("Interrupted", colors::TEXT_MUTED),
        };

        let is_running = block.is_running();
        let mut footer = Row::new()
            .spacing(8.0)
            .cross_align(CrossAxisAlignment::Center);

        if is_running {
            footer = footer.push(
                ButtonElement::new(self.stop_id, "Stop")
                    .background(Color::rgba(0.5, 0.5, 0.5, 0.3))
                    .text_color(Color::rgb(0.9, 0.5, 0.5))
                    .corner_radius(4.0),
            );
        }

        footer = footer.push(TextElement::new(status_text).color(status_color));

        if let Some(ms) = block.duration_ms {
            let duration = if ms < 1000 {
                format!("{}ms", ms)
            } else {
                format!("{:.1}s", ms as f64 / 1000.0)
            };
            footer = footer.push(TextElement::new(duration).color(colors::TEXT_MUTED));
        }

        content = content.fixed_spacer(4.0).push(footer);

        content.into()
    }
}

/// Build a tool invocation widget.
fn build_tool_widget(tool: &ToolInvocation, toggle_id: SourceId) -> Column {
    let (status_icon, status_color) = match tool.status {
        ToolStatus::Pending => ("\u{25CB}", colors::TOOL_PENDING),   // ◯
        ToolStatus::Running => ("\u{25CF}", colors::RUNNING),        // ●
        ToolStatus::Success => ("\u{2713}", colors::SUCCESS),        // ✓
        ToolStatus::Error => ("\u{2717}", colors::ERROR),            // ✗
    };

    let collapse_icon = if tool.collapsed { "\u{25B6}" } else { "\u{25BC}" };
    let label = if let Some(ref msg) = tool.message {
        format!("{} {} {} {}", collapse_icon, status_icon, tool.name, msg)
    } else {
        format!("{} {} {}", collapse_icon, status_icon, tool.name)
    };

    let mut col = Column::new().spacing(2.0);

    col = col.push(
        ButtonElement::new(toggle_id, &label)
            .background(Color::TRANSPARENT)
            .text_color(status_color)
            .corner_radius(2.0),
    );

    if !tool.collapsed {
        // Parameters
        if !tool.parameters.is_empty() {
            for (name, value) in &tool.parameters {
                let display_value = if value.len() > 100 {
                    format!("{}...", &value[..100])
                } else {
                    value.clone()
                };
                col = col.push(
                    Row::new()
                        .fixed_spacer(16.0)
                        .push(TextElement::new(format!("{}: {}", name, display_value)).color(colors::TEXT_MUTED)),
                );
            }
        }

        // Output
        if let Some(ref output) = tool.output {
            let display_output = if output.len() > 500 {
                format!("{}...\n[{} more chars]", &output[..500], output.len() - 500)
            } else {
                output.clone()
            };
            for line in display_output.lines().take(20) {
                col = col.push(
                    Row::new()
                        .fixed_spacer(16.0)
                        .push(TextElement::new(line).color(colors::TOOL_OUTPUT)),
                );
            }
        }
    }

    col
}

/// Build a permission dialog widget.
fn build_permission_dialog(
    perm: &crate::agent_block::PermissionRequest,
    deny_id: SourceId,
    allow_id: SourceId,
    always_id: SourceId,
) -> Column {
    let code_block = Column::new()
        .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
        .background(Color::rgba(0.0, 0.0, 0.0, 0.3))
        .corner_radius(4.0)
        .push(TextElement::new(&perm.action).color(colors::WARNING));

    let mut dialog = Column::new()
        .padding(14.0)
        .spacing(8.0)
        .background(colors::BG_CARD)
        .corner_radius(8.0)
        .border(colors::BORDER_SUBTLE, 1.0)
        .width(Length::Fill)
        .push(TextElement::new("\u{26A0} Permission Required").color(colors::WARNING))
        .push(TextElement::new(&perm.description).color(colors::TEXT_SECONDARY))
        .push(code_block);

    if let Some(ref dir) = perm.working_dir {
        dialog = dialog.push(TextElement::new(format!("in {}", dir)).color(colors::TEXT_MUTED));
    }

    dialog = dialog.push(
        Row::new()
            .spacing(8.0)
            .push(
                ButtonElement::new(deny_id, "Deny")
                    .background(colors::BTN_DENY)
                    .corner_radius(4.0),
            )
            .push(
                ButtonElement::new(allow_id, "Allow Once")
                    .background(colors::BTN_ALLOW)
                    .corner_radius(4.0),
            )
            .push(
                ButtonElement::new(always_id, "Allow Always")
                    .background(colors::BTN_ALWAYS)
                    .corner_radius(4.0),
            ),
    );

    dialog
}

/// Build response text with basic markdown rendering.
fn build_response_text(response: &str) -> Column {
    let mut col = Column::new().spacing(2.0);

    let mut in_code_block = false;
    let mut code_lines: Vec<String> = Vec::new();

    for line in response.lines() {
        if line.starts_with("```") {
            if in_code_block {
                // End code block
                let code_col = Column::new()
                    .padding_custom(Padding::new(6.0, 12.0, 6.0, 12.0))
                    .background(colors::CODE_BG)
                    .corner_radius(4.0)
                    .width(Length::Fill);
                let mut code_inner = code_col;
                for code_line in code_lines.drain(..) {
                    code_inner = code_inner.push(TextElement::new(code_line).color(colors::CODE_TEXT));
                }
                col = col.push(code_inner);
                in_code_block = false;
            } else {
                in_code_block = true;
            }
        } else if in_code_block {
            code_lines.push(line.to_string());
        } else if line.starts_with("# ") {
            col = col.push(TextElement::new(&line[2..]).color(colors::TEXT_PRIMARY).size(16.0));
        } else if line.starts_with("## ") {
            col = col.push(TextElement::new(&line[3..]).color(colors::TEXT_PRIMARY).size(15.0));
        } else if line.starts_with("**") && line.ends_with("**") && line.len() > 4 {
            col = col.push(TextElement::new(&line[2..line.len()-2]).color(colors::TEXT_PRIMARY));
        } else if line.starts_with("- ") || line.starts_with("* ") {
            col = col.push(
                Row::new()
                    .push(TextElement::new("  \u{00B7} ").color(colors::TEXT_MUTED))
                    .push(TextElement::new(&line[2..]).color(colors::TEXT_PRIMARY)),
            );
        } else if line.is_empty() {
            col = col.fixed_spacer(4.0);
        } else {
            col = col.push(TextElement::new(line).color(colors::TEXT_PRIMARY));
        }
    }

    // Handle unclosed code block
    if in_code_block && !code_lines.is_empty() {
        let code_col = Column::new()
            .padding_custom(Padding::new(6.0, 12.0, 6.0, 12.0))
            .background(colors::CODE_BG)
            .corner_radius(4.0)
            .width(Length::Fill);
        let mut code_inner = code_col;
        for code_line in code_lines {
            code_inner = code_inner.push(TextElement::new(code_line).color(colors::CODE_TEXT));
        }
        col = col.push(code_inner);
    }

    col
}

// =========================================================================
// Welcome Screen — shown when no blocks exist
// =========================================================================

pub struct WelcomeScreen<'a> {
    pub cwd: &'a str,
}

impl Widget for WelcomeScreen<'_> {
    fn build(self) -> LayoutChild {
        // Shorten home directory
        let home = std::env::var("HOME").unwrap_or_default();
        let display_cwd = if self.cwd.starts_with(&home) {
            self.cwd.replacen(&home, "~", 1)
        } else {
            self.cwd.to_string()
        };

        let logo = r#" ███╗   ██╗███████╗██╗  ██╗██╗   ██╗███████╗
 ████╗  ██║██╔════╝╚██╗██╔╝██║   ██║██╔════╝
 ██╔██╗ ██║█████╗   ╚███╔╝ ██║   ██║███████╗
 ██║╚██╗██║██╔══╝   ██╔██╗ ██║   ██║╚════██║
 ██║ ╚████║███████╗██╔╝ ██╗╚██████╔╝███████║
 ╚═╝  ╚═══╝╚══════╝╚═╝  ╚═╝ ╚═════╝ ╚══════╝"#;

        // Left column: logo + welcome
        let mut logo_col = Column::new().spacing(0.0);
        for line in logo.lines() {
            logo_col = logo_col.push(TextElement::new(line).color(colors::WELCOME_TITLE));
        }

        let left = Column::new()
            .spacing(4.0)
            .width(Length::Fill)
            .push(logo_col)
            .fixed_spacer(8.0)
            .push(
                Row::new()
                    .spacing(8.0)
                    .push(TextElement::new("Welcome to Nexus Shell").color(colors::WELCOME_TITLE).size(16.0))
                    .push(TextElement::new("v0.1.0").color(colors::TEXT_MUTED)),
            )
            .fixed_spacer(4.0)
            .push(TextElement::new(format!("  {}", display_cwd)).color(colors::TEXT_PATH));

        // Tips card
        let tips = Column::new()
            .padding(12.0)
            .spacing(2.0)
            .background(colors::CARD_BG)
            .corner_radius(6.0)
            .border(colors::CARD_BORDER, 1.0)
            .width(Length::Fill)
            .push(TextElement::new("Getting Started").color(colors::WELCOME_HEADING))
            .fixed_spacer(8.0)
            .push(TextElement::new("\u{2022} Type any command and press Enter").color(colors::TEXT_SECONDARY))
            .push(TextElement::new("\u{2022} Use Tab for completions").color(colors::TEXT_SECONDARY))
            .fixed_spacer(8.0)
            .push(TextElement::new("\u{2022} Click [SH] to switch to AI mode").color(colors::TEXT_PURPLE))
            .push(TextElement::new("\u{2022} Prefix with \"? \" for one-shot AI queries").color(colors::TEXT_PURPLE))
            .fixed_spacer(8.0)
            .push(TextElement::new("Try: ? what files are in this directory?").color(colors::TEXT_PURPLE));

        // Shortcuts card
        let shortcuts = Column::new()
            .padding(12.0)
            .spacing(2.0)
            .background(colors::CARD_BG)
            .corner_radius(6.0)
            .border(colors::CARD_BORDER, 1.0)
            .width(Length::Fill)
            .push(TextElement::new("Shortcuts").color(colors::WELCOME_HEADING))
            .fixed_spacer(8.0)
            .push(TextElement::new("Cmd+K     Clear screen").color(colors::TEXT_SECONDARY))
            .push(TextElement::new("Cmd++/-   Zoom in/out").color(colors::TEXT_SECONDARY))
            .push(TextElement::new("Ctrl+R    Search history").color(colors::TEXT_SECONDARY))
            .push(TextElement::new("Up/Down   Navigate history").color(colors::TEXT_SECONDARY));

        // Right column: tips + shortcuts
        let right = Column::new()
            .spacing(12.0)
            .width(Length::Fill)
            .push(tips)
            .push(shortcuts);

        Row::new()
            .padding(20.0)
            .spacing(40.0)
            .width(Length::Fill)
            .push(left)
            .push(right)
            .into()
    }
}

// =========================================================================
// Job Bar — shows background job pills
// =========================================================================

pub struct JobBar<'a> {
    pub jobs: &'a [VisualJob],
}

impl Widget for JobBar<'_> {
    fn build(self) -> LayoutChild {
        let mut row = Row::new().spacing(8.0);

        for job in self.jobs {
            let (icon, color, bg) = match job.state {
                VisualJobState::Running => ("\u{25CF}", Color::rgb(0.3, 0.8, 0.3), Color::rgba(0.2, 0.4, 0.2, 0.6)),
                VisualJobState::Stopped => ("\u{23F8}", Color::rgb(0.9, 0.7, 0.2), Color::rgba(0.4, 0.35, 0.1, 0.6)),
            };
            let name = job.display_name();
            row = row.push(
                Column::new()
                    .padding_custom(Padding::new(4.0, 10.0, 4.0, 10.0))
                    .background(bg)
                    .corner_radius(12.0)
                    .border(Color::rgba(0.5, 0.5, 0.5, 0.3), 1.0)
                    .push(TextElement::new(format!("{} {}", icon, name)).color(color)),
            );
        }

        Row::new()
            .padding_custom(Padding::new(4.0, 15.0, 4.0, 15.0))
            .width(Length::Fill)
            .push(Row::new().spacer(1.0).push(row))
            .into()
    }
}

// =========================================================================
// Input Bar — mode toggle + path + prompt + text input
// =========================================================================

pub struct NexusInputBar<'a> {
    pub input: &'a crate::strata::TextInputState,
    pub mode: crate::blocks::InputMode,
    pub cwd: &'a str,
    pub last_exit_code: Option<i32>,
    pub cursor_visible: bool,
    pub mode_toggle_id: SourceId,
}

impl Widget for NexusInputBar<'_> {
    fn build(self) -> LayoutChild {
        use crate::blocks::InputMode;
        use crate::strata::TextInputElement;

        // Mode button
        let (mode_label, mode_color, mode_bg, prompt_char) = match self.mode {
            InputMode::Shell => ("SH", colors::SUCCESS, Color::rgba(0.2, 0.5, 0.3, 0.4), "$"),
            InputMode::Agent => ("AI", colors::TEXT_PURPLE, Color::rgba(0.4, 0.3, 0.7, 0.4), "?"),
        };

        let mode_btn = ButtonElement::new(self.mode_toggle_id, mode_label)
            .background(mode_bg)
            .text_color(mode_color)
            .corner_radius(4.0);

        // Shorten cwd for display
        let home = std::env::var("HOME").unwrap_or_default();
        let display_cwd = if self.cwd.starts_with(&home) {
            self.cwd.replacen(&home, "~", 1)
        } else {
            self.cwd.to_string()
        };

        // Prompt color based on exit code
        let prompt_color = match self.last_exit_code {
            Some(0) | None => colors::SUCCESS,
            Some(_) => colors::ERROR,
        };

        Row::new()
            .padding_custom(Padding::new(8.0, 12.0, 8.0, 12.0))
            .spacing(10.0)
            .background(colors::BG_INPUT)
            .corner_radius(6.0)
            .border(colors::BORDER_INPUT, 1.0)
            .width(Length::Fill)
            .cross_align(CrossAxisAlignment::Center)
            .push(mode_btn)
            .push(TextElement::new(display_cwd).color(colors::TEXT_PATH))
            .push(TextElement::new(prompt_char).color(prompt_color))
            .push(
                TextInputElement::from_state(self.input)
                    .placeholder("Type a command...")
                    .background(Color::TRANSPARENT)
                    .border_color(Color::TRANSPARENT)
                    .focus_border_color(Color::TRANSPARENT)
                    .corner_radius(0.0)
                    .padding(Padding::new(0.0, 4.0, 0.0, 4.0))
                    .width(Length::Fill)
                    .cursor_visible(self.cursor_visible),
            )
            .into()
    }
}

// =========================================================================
// Helpers
// =========================================================================

/// Convert nexus-term color to Strata color.
fn term_color_to_strata(c: nexus_term::Color) -> Color {
    match c {
        nexus_term::Color::Default => Color::rgb(0.8, 0.8, 0.8),
        nexus_term::Color::Named(n) => match n {
            0  => Color::rgb(0.0, 0.0, 0.0),      // Black
            1  => Color::rgb(0.8, 0.3, 0.3),      // Red
            2  => Color::rgb(0.3, 0.8, 0.3),      // Green
            3  => Color::rgb(0.8, 0.8, 0.3),      // Yellow
            4  => Color::rgb(0.4, 0.5, 0.9),      // Blue
            5  => Color::rgb(0.8, 0.3, 0.8),      // Magenta
            6  => Color::rgb(0.3, 0.8, 0.8),      // Cyan
            7  => Color::rgb(0.8, 0.8, 0.8),      // White
            8  => Color::rgb(0.5, 0.5, 0.5),      // Bright Black
            9  => Color::rgb(1.0, 0.4, 0.4),      // Bright Red
            10 => Color::rgb(0.4, 1.0, 0.4),      // Bright Green
            11 => Color::rgb(1.0, 1.0, 0.4),      // Bright Yellow
            12 => Color::rgb(0.5, 0.6, 1.0),      // Bright Blue
            13 => Color::rgb(1.0, 0.4, 1.0),      // Bright Magenta
            14 => Color::rgb(0.4, 1.0, 1.0),      // Bright Cyan
            15 => Color::rgb(1.0, 1.0, 1.0),      // Bright White
            _ => Color::rgb(0.8, 0.8, 0.8),
        },
        nexus_term::Color::Rgb(r, g, b) => Color::rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0),
        nexus_term::Color::Indexed(n) => match n {
            0  => Color::rgb(0.0, 0.0, 0.0),
            1  => Color::rgb(0.8, 0.3, 0.3),
            2  => Color::rgb(0.3, 0.8, 0.3),
            3  => Color::rgb(0.8, 0.8, 0.3),
            4  => Color::rgb(0.4, 0.5, 0.9),
            5  => Color::rgb(0.8, 0.3, 0.8),
            6  => Color::rgb(0.3, 0.8, 0.8),
            7  => Color::rgb(0.8, 0.8, 0.8),
            8  => Color::rgb(0.5, 0.5, 0.5),
            9  => Color::rgb(1.0, 0.4, 0.4),
            10 => Color::rgb(0.4, 1.0, 0.4),
            11 => Color::rgb(1.0, 1.0, 0.4),
            12 => Color::rgb(0.5, 0.6, 1.0),
            13 => Color::rgb(1.0, 0.4, 1.0),
            14 => Color::rgb(0.4, 1.0, 1.0),
            15 => Color::rgb(1.0, 1.0, 1.0),
            _ => Color::rgb(0.7, 0.7, 0.7),
        },
    }
}

/// Render a structured Value from a native (kernel) command into the layout.
fn render_native_value(mut parent: Column, value: &Value, block_id: BlockId) -> Column {
    match value {
        Value::Unit => parent,

        Value::Table { columns, rows } => {
            let source_id = SourceId::named(&format!("table_{}", block_id.0));
            let mut table = TableElement::new(source_id);

            // Estimate column widths from data
            let col_widths = estimate_column_widths(columns, rows);

            // Add column headers
            for (i, col) in columns.iter().enumerate() {
                table = table.column(&col.name, col_widths[i]);
            }

            // Add data rows
            for row in rows {
                let cells: Vec<TableCell> = row.iter().map(|cell| {
                    TableCell {
                        text: cell.to_text(),
                        color: value_text_color(cell),
                    }
                }).collect();
                table = table.row(cells);
            }

            parent.push(table)
        }

        Value::List(items) => {
            // Check for file entries
            let file_entries: Vec<&nexus_api::FileEntry> = items
                .iter()
                .filter_map(|v| match v {
                    Value::FileEntry(entry) => Some(entry.as_ref()),
                    _ => None,
                })
                .collect();

            if file_entries.len() == items.len() && !file_entries.is_empty() {
                // Render as file list with colors
                for entry in &file_entries {
                    let color = file_entry_color(entry);
                    let display = if let Some(target) = &entry.symlink_target {
                        format!("{} -> {}", entry.name, target.display())
                    } else {
                        entry.name.clone()
                    };
                    parent = parent.push(TextElement::new(display).color(color));
                }
                parent
            } else {
                // Generic list
                for item in items {
                    parent = parent.push(
                        TextElement::new(item.to_text()).color(colors::TEXT_PRIMARY),
                    );
                }
                parent
            }
        }

        Value::FileEntry(entry) => {
            let color = file_entry_color(entry);
            let display = if let Some(target) = &entry.symlink_target {
                format!("{} -> {}", entry.name, target.display())
            } else {
                entry.name.clone()
            };
            parent.push(TextElement::new(display).color(color))
        }

        Value::Record(fields) => {
            for (key, val) in fields {
                parent = parent.push(
                    Row::new()
                        .spacing(8.0)
                        .push(TextElement::new(format!("{}:", key)).color(colors::TEXT_SECONDARY))
                        .push(TextElement::new(val.to_text()).color(colors::TEXT_PRIMARY)),
                );
            }
            parent
        }

        Value::Error { message, .. } => {
            parent.push(TextElement::new(message).color(colors::ERROR))
        }

        // All other types: render as text
        _ => {
            let text = value.to_text();
            if text.is_empty() {
                parent
            } else {
                for line in text.lines() {
                    parent = parent.push(TextElement::new(line).color(colors::TEXT_PRIMARY));
                }
                parent
            }
        }
    }
}

/// Get text color for a Value cell in a table.
fn value_text_color(value: &Value) -> Color {
    match value {
        Value::Int(_) | Value::Float(_) => Color::rgb(0.6, 0.8, 1.0),
        Value::Bool(true) => colors::SUCCESS,
        Value::Bool(false) => colors::ERROR,
        Value::Path(_) => colors::TEXT_PATH,
        Value::FileEntry(e) => file_entry_color(e),
        Value::Error { .. } => colors::ERROR,
        _ => colors::TEXT_PRIMARY,
    }
}

/// Estimate column widths based on header names and data content.
fn estimate_column_widths(columns: &[nexus_api::TableColumn], rows: &[Vec<Value>]) -> Vec<f32> {
    let char_w = 8.4; // approximate monospace character width
    let padding = 16.0;

    columns.iter().enumerate().map(|(i, col)| {
        let header_len = col.name.len();
        let max_data_len = rows.iter()
            .filter_map(|row| row.get(i))
            .map(|v| v.to_text().len())
            .max()
            .unwrap_or(0);
        let max_len = header_len.max(max_data_len).max(4);
        (max_len as f32 * char_w + padding).min(400.0)
    }).collect()
}

/// Get display color for a file entry.
fn file_entry_color(entry: &nexus_api::FileEntry) -> Color {
    match entry.file_type {
        FileType::Directory => Color::rgb(0.4, 0.6, 1.0),
        FileType::Symlink => Color::rgb(0.4, 0.9, 0.9),
        _ if entry.permissions & 0o111 != 0 => Color::rgb(0.4, 0.9, 0.4),
        _ => Color::rgb(0.8, 0.8, 0.8),
    }
}
