//! Nexus Widget Structs for Strata
//!
//! Production UI components that render real Nexus data (Block, AgentBlock, etc.)
//! using Strata's layout primitives. Each widget takes references to backend
//! data models and builds a layout tree.

use nexus_api::BlockId;
use nexus_kernel::{Completion, CompletionKind};

use crate::agent_block::{AgentBlock, AgentBlockState};
use strata::content_address::SourceId;
use crate::nexus_app::source_ids;
use strata::layout::{
    ButtonElement, Column, CrossAxisAlignment, LayoutChild, Length, Padding, Row,
    ScrollColumn, TextElement, TextInputElement, Widget,
};
use strata::primitives::Color;
use strata::scroll_state::ScrollState;
use crate::blocks::{VisualJob, VisualJobState};

use crate::nexus_app::colors;

// ShellBlockWidget has been moved to widgets/shell_block.rs
pub use crate::widgets::ShellBlockWidget;
// ToolWidget has been moved to widgets/tool.rs
use crate::widgets::ToolWidget;

// =========================================================================
// Agent Block Widget — renders a real AgentBlock
// =========================================================================

pub struct AgentBlockWidget<'a> {
    pub block: &'a AgentBlock,
    pub thinking_toggle_id: SourceId,
    pub stop_id: SourceId,
    /// Text input state for free-form question answers (only set when question is pending).
    pub question_input: Option<&'a strata::TextInputState>,
}

impl<'a> AgentBlockWidget<'a> {
    /// Generate a stable SourceId for a tool toggle button.
    fn tool_toggle_id(block_id: BlockId, tool_index: usize) -> SourceId {
        source_ids::agent_tool_toggle(block_id, tool_index)
    }

    /// Generate a stable SourceId for permission buttons.
    fn perm_deny_id(block_id: BlockId) -> SourceId {
        source_ids::agent_perm_deny(block_id)
    }
    fn perm_allow_id(block_id: BlockId) -> SourceId {
        source_ids::agent_perm_allow(block_id)
    }
    fn perm_always_id(block_id: BlockId) -> SourceId {
        source_ids::agent_perm_always(block_id)
    }
}

impl<'a> Widget<'a> for AgentBlockWidget<'a> {
    fn build(self) -> LayoutChild<'a> {
        let block = self.block;

        let mut content = Column::new()
            .padding(6.0)
            .spacing(3.0)
            .background(colors::BG_BLOCK)
            .corner_radius(4.0)
            .width(Length::Fill);

        // Query line (Claude Code style: > prefix with subtle badge)
        let query_source = source_ids::agent_query(block.id);
        let query_badge = Row::new()
            .padding_custom(Padding::new(2.0, 8.0, 2.0, 8.0))
            .background(Color::rgba(1.0, 1.0, 1.0, 0.06))
            .corner_radius(4.0)
            .spacing(6.0)
            .push(TextElement::new(">").color(colors::TEXT_MUTED).source(query_source))
            .push(TextElement::new(&block.query).color(colors::TEXT_PRIMARY).source(query_source));
        content = content.push(query_badge);

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
                let thinking_source = source_ids::agent_thinking(block.id);
                let thinking_preview = if block.thinking.len() > 500 {
                    format!("{}...", &block.thinking[..500])
                } else {
                    block.thinking.clone()
                };
                for line in thinking_preview.lines() {
                    content = content.push(
                        Row::new()
                            .fixed_spacer(16.0)
                            .push(TextElement::new(line).color(colors::THINKING).source(thinking_source)),
                    );
                }
            }
        }

        // Tool invocations
        for (i, tool) in block.tools.iter().enumerate() {
            let toggle_id = Self::tool_toggle_id(block.id, i);
            let tool_source = source_ids::agent_tool(block.id, i);
            content = content.push(ToolWidget::view(tool, toggle_id, tool_source));
        }

        // Permission dialog
        if let Some(ref perm) = block.pending_permission {
            let perm_source = source_ids::agent_perm_text(block.id);
            content = content.push(build_permission_dialog(
                perm,
                Self::perm_deny_id(block.id),
                Self::perm_allow_id(block.id),
                Self::perm_always_id(block.id),
                perm_source,
            ));
        }

        // User question dialog (AskUserQuestion via MCP permission)
        if let Some(ref question) = block.pending_question {
            let q_source = source_ids::agent_question_text(block.id);
            content = content.push(build_question_dialog(question, block.id, self.question_input, q_source));
        }

        // Response text (Claude Code style: bullet prefix)
        if !block.response.is_empty() {
            let response_source = source_ids::agent_response(block.id);
            content = content.push(
                Row::new()
                    .spacing(6.0)
                    .cross_align(CrossAxisAlignment::Start)
                    .push(TextElement::new("\u{25CF}").color(colors::TEXT_MUTED)) // ●
                    .push(crate::markdown::render(&block.response, response_source)),
            );
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
        let footer_source = source_ids::agent_footer(block.id);
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

        footer = footer.push(TextElement::new(status_text).color(status_color).source(footer_source));

        if let Some(ms) = block.duration_ms {
            let duration = if ms < 1000 {
                format!("{}ms", ms)
            } else {
                format!("{:.1}s", ms as f64 / 1000.0)
            };
            footer = footer.push(TextElement::new(&duration).color(colors::TEXT_MUTED).source(footer_source));
        }

        if let Some(cost) = block.cost_usd {
            footer = footer.push(
                TextElement::new(&format!("${:.4}", cost)).color(colors::TEXT_MUTED).source(footer_source),
            );
        }

        let total_tokens = block.input_tokens.unwrap_or(0) + block.output_tokens.unwrap_or(0);
        if total_tokens > 0 {
            footer = footer.push(
                TextElement::new(&format!("\u{2193} {}", format_tokens(total_tokens)))
                    .color(colors::TEXT_MUTED).source(footer_source),
            );
        }

        content = content.fixed_spacer(4.0).push(footer);

        content.into()
    }
}

/// Format a token count with K/M suffixes.
fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M tokens", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k tokens", n as f64 / 1_000.0)
    } else {
        format!("{} tokens", n)
    }
}

// Tool rendering functions have been moved to widgets/tool.rs

/// Build a permission dialog widget.
fn build_permission_dialog(
    perm: &crate::agent_block::PermissionRequest,
    deny_id: SourceId,
    allow_id: SourceId,
    always_id: SourceId,
    source_id: SourceId,
) -> Column<'static> {
    let code_block = Column::new()
        .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
        .background(Color::rgba(0.0, 0.0, 0.0, 0.3))
        .corner_radius(4.0)
        .push(TextElement::new(&perm.action).color(colors::WARNING).source(source_id));

    // Permission dialog colors from agent_widgets.rs
    let mut dialog = Column::new()
        .padding(8.0)
        .spacing(4.0)
        .background(Color::rgb(0.15, 0.1, 0.05))
        .corner_radius(8.0)
        .border(Color::rgb(0.8, 0.5, 0.2), 1.0)
        .width(Length::Fill)
        .push(TextElement::new("\u{26A0} Permission Required").color(colors::WARNING).source(source_id))
        .push(TextElement::new(&perm.description).color(colors::TEXT_SECONDARY).source(source_id))
        .push(code_block);

    if let Some(ref dir) = perm.working_dir {
        dialog = dialog.push(TextElement::new(format!("in {}", dir)).color(colors::TEXT_MUTED).source(source_id));
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

/// Build a question dialog for AskUserQuestion (via MCP permission).
fn build_question_dialog(
    question: &crate::agent_block::PendingUserQuestion,
    block_id: BlockId,
    question_input: Option<&strata::TextInputState>,
    source_id: SourceId,
) -> Column<'static> {
    let mut dialog = Column::new()
        .padding(8.0)
        .spacing(6.0)
        .background(Color::rgb(0.05, 0.08, 0.15))
        .corner_radius(8.0)
        .border(Color::rgb(0.2, 0.5, 0.8), 1.0)
        .width(Length::Fill)
        .push(TextElement::new("\u{2753} Claude is asking:").color(colors::TOOL_ACTION).source(source_id));

    for (q_idx, q) in question.questions.iter().enumerate() {
        dialog = dialog.push(
            TextElement::new(&q.question).color(colors::TEXT_PRIMARY).source(source_id)
        );

        let mut row = Row::new().spacing(8.0);
        for (o_idx, opt) in q.options.iter().enumerate() {
            let id = source_ids::agent_question_option(block_id, q_idx, o_idx);
            row = row.push(
                ButtonElement::new(id, &opt.label)
                    .background(Color::rgb(0.12, 0.25, 0.45))
                    .corner_radius(4.0),
            );
        }
        dialog = dialog.push(row);
    }

    // Free-form text input (the "Other" option)
    if let Some(input) = question_input {
        let has_options = question.questions.iter().any(|q| !q.options.is_empty());
        let label = if has_options {
            "Or type a custom answer:"
        } else {
            "Type your answer:"
        };
        dialog = dialog.push(
            TextElement::new(label).color(colors::TEXT_SECONDARY).source(source_id)
        );
        let submit_id = source_ids::agent_question_submit(block_id);
        dialog = dialog.push(
            Row::new().spacing(8.0).width(Length::Fill)
                .push(
                    TextInputElement::from_state(input)
                        .placeholder("Type your answer and press Enter...")
                        .background(Color::rgb(0.08, 0.08, 0.12))
                        .border_color(Color::rgb(0.3, 0.3, 0.4))
                        .width(Length::Fill)
                )
                .push(
                    ButtonElement::new(submit_id, "Submit")
                        .background(Color::rgb(0.12, 0.25, 0.45))
                        .corner_radius(4.0)
                )
        );
    }

    dialog
}

// =========================================================================
// Welcome Screen — shown when no blocks exist
// =========================================================================

pub struct WelcomeScreen<'a> {
    pub cwd: &'a str,
}

impl<'a> Widget<'a> for WelcomeScreen<'a> {
    fn build(self) -> LayoutChild<'a> {
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
            .padding(8.0)
            .spacing(2.0)
            .background(colors::CARD_BG)
            .corner_radius(4.0)
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
            .padding(8.0)
            .spacing(2.0)
            .background(colors::CARD_BG)
            .corner_radius(4.0)
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
            .padding(12.0)
            .spacing(20.0)
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

impl JobBar<'_> {
    pub fn job_pill_id(job_id: u32) -> SourceId {
        SourceId::named(&format!("job_{}", job_id))
    }
}

impl<'a> Widget<'a> for JobBar<'a> {
    fn build(self) -> LayoutChild<'a> {
        let mut row = Row::new().spacing(8.0);

        for job in self.jobs {
            let (icon, color, bg) = match job.state {
                VisualJobState::Running => ("\u{25CF}", Color::rgb(0.3, 0.8, 0.3), Color::rgba(0.2, 0.4, 0.2, 0.6)),
                VisualJobState::Stopped => ("\u{23F8}", Color::rgb(0.9, 0.7, 0.2), Color::rgba(0.4, 0.35, 0.1, 0.6)),
            };
            let name = job.display_name();
            let click_id = Self::job_pill_id(job.id);
            row = row.push(
                Row::new()
                    .id(click_id)
                    .padding_custom(Padding::new(2.0, 6.0, 2.0, 6.0))
                    .background(bg)
                    .corner_radius(12.0)
                    .border(Color::rgba(0.5, 0.5, 0.5, 0.3), 1.0)
                    .push(TextElement::new(format!("{} {}", icon, name)).color(color)),
            );
        }

        Row::new()
            .padding_custom(Padding::new(2.0, 4.0, 2.0, 4.0))
            .width(Length::Fill)
            .push(Row::new().spacer(1.0).push(row))
            .into()
    }
}

// =========================================================================
// Input Bar — mode toggle + path + prompt + text input
// =========================================================================

pub struct NexusInputBar<'a> {
    pub input: &'a strata::TextInputState,
    pub mode: crate::blocks::InputMode,
    pub cwd: &'a str,
    pub last_exit_code: Option<i32>,
    pub cursor_visible: bool,
    pub mode_toggle_id: SourceId,
    pub line_count: usize,
}

impl<'a> Widget<'a> for NexusInputBar<'a> {
    fn build(self) -> LayoutChild<'a> {
        use crate::blocks::InputMode;
        use strata::TextInputElement;

        // Mode button
        let (mode_label, mode_color, mode_bg, prompt_char) = match self.mode {
            InputMode::Shell => ("SH", Color::rgb(0.5, 0.9, 0.5), Color::rgb(0.2, 0.3, 0.2), "$"),
            InputMode::Agent => ("AI", Color::rgb(0.7, 0.7, 1.0), Color::rgb(0.25, 0.25, 0.4), "?"),
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

        // Prompt color based on exit code (rgb8 values from input.rs)
        let prompt_color = match self.last_exit_code {
            // rgb8(50, 205, 50) = lime green
            Some(0) | None => Color::rgb(0.196, 0.804, 0.196),
            // rgb8(220, 50, 50) = bright red
            Some(_) => Color::rgb(0.863, 0.196, 0.196),
        };

        Row::new()
            .padding_custom(Padding::new(4.0, 6.0, 4.0, 6.0))
            .spacing(6.0)
            .background(colors::BG_INPUT)
            .corner_radius(6.0)
            .border(colors::BORDER_INPUT, 1.0)
            .width(Length::Fill)
            .cross_align(CrossAxisAlignment::Center)
            .push(mode_btn)
            .push(TextElement::new(display_cwd).color(colors::TEXT_PATH))
            .push(TextElement::new(prompt_char).color(prompt_color))
            .push({
                let mut elem = TextInputElement::from_state(self.input)
                    .placeholder("Type a command...")
                    .background(Color::TRANSPARENT)
                    .border_color(Color::TRANSPARENT)
                    .focus_border_color(Color::TRANSPARENT)
                    .corner_radius(0.0)
                    .padding(Padding::new(0.0, 4.0, 0.0, 4.0))
                    .width(Length::Fill)
                    .cursor_visible(self.cursor_visible);
                if self.line_count > 1 {
                    let line_height = 18.0_f32;
                    let input_height = self.line_count as f32 * line_height + 4.0;
                    elem = elem.multiline(true).height(Length::Fixed(input_height));
                }
                elem
            })
            .into()
    }
}

// =========================================================================
// Completion Popup — shows tab completion results
// =========================================================================

pub struct CompletionPopup<'a> {
    pub completions: &'a [Completion],
    pub selected_index: Option<usize>,
    pub hovered_index: Option<usize>,
    pub scroll: &'a ScrollState,
}

impl CompletionPopup<'_> {
    /// Generate a stable SourceId for clicking a completion item.
    pub fn item_id(index: usize) -> SourceId {
        SourceId::named(&format!("comp_item_{}", index))
    }
}

impl<'a> Widget<'a> for CompletionPopup<'a> {
    fn build(self) -> LayoutChild<'a> {
        // Scrollable list of completions, max 300px tall
        let mut scroll = ScrollColumn::from_state(self.scroll)
            .spacing(0.0)
            .width(Length::Fixed(300.0))
            .height(Length::Fixed(300.0_f32.min(self.completions.len() as f32 * 26.0 + 8.0)))
            .background(Color::rgb(0.12, 0.12, 0.15))
            .corner_radius(4.0)
            .border(Color::rgb(0.3, 0.3, 0.35), 1.0);

        for (i, comp) in self.completions.iter().enumerate() {
            let is_selected = self.selected_index == Some(i);
            let is_hovered = self.hovered_index == Some(i) && !is_selected;

            // Icon from CompletionKind (matches kernel's icon() method)
            let icon = comp.kind.icon();

            // Icon colors matched from old UI input.rs completion_icon_color
            let icon_color = match comp.kind {
                CompletionKind::Directory => Color::rgb(0.4, 0.7, 1.0),
                CompletionKind::Executable | CompletionKind::NativeCommand => Color::rgb(0.4, 0.9, 0.4),
                CompletionKind::Builtin => Color::rgb(1.0, 0.8, 0.4),
                CompletionKind::Function => Color::rgb(0.8, 0.6, 1.0),
                CompletionKind::Variable => Color::rgb(1.0, 0.6, 0.6),
                _ => Color::rgb(0.7, 0.7, 0.7),
            };

            let text_color = if is_selected { Color::WHITE } else { Color::rgb(0.8, 0.8, 0.8) };
            let bg = if is_selected {
                Color::rgb(0.2, 0.4, 0.6)
            } else if is_hovered {
                Color::rgb(0.22, 0.22, 0.28)
            } else {
                Color::rgb(0.15, 0.15, 0.18)
            };

            let click_id = Self::item_id(i);
            scroll = scroll.push(
                Row::new()
                    .id(click_id)
                    .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
                    .spacing(4.0)
                    .background(bg)
                    .corner_radius(3.0)
                    .cross_align(CrossAxisAlignment::Center)
                    .push(TextElement::new(format!("{} ", icon)).color(icon_color))
                    .push(TextElement::new(&comp.display).color(text_color)),
            );
        }

        Column::new()
            .padding_custom(Padding::new(0.0, 4.0, 2.0, 4.0))
            .width(Length::Fill)
            .push(scroll)
            .into()
    }
}

// =========================================================================
// History Search Bar — Ctrl+R reverse-i-search
// =========================================================================

pub struct HistorySearchBar<'a> {
    pub query: &'a str,
    pub results: &'a [String],
    pub result_index: usize,
    pub hovered_index: Option<usize>,
    pub scroll: &'a ScrollState,
}

impl HistorySearchBar<'_> {
    /// Generate a stable SourceId for clicking a history result item.
    pub fn result_id(index: usize) -> SourceId {
        SourceId::named(&format!("hist_result_{}", index))
    }
}

impl<'a> Widget<'a> for HistorySearchBar<'a> {
    fn build(self) -> LayoutChild<'a> {
        // History search overlay matched from old UI input.rs
        let mut container = Column::new()
            .padding(10.0)
            .spacing(6.0)
            .background(Color::rgb(0.1, 0.1, 0.12))
            .corner_radius(6.0)
            .border(Color::rgb(0.3, 0.5, 0.7), 1.0)
            .width(Length::Fill);

        // Search header: label + query display
        let header = Row::new()
            .spacing(8.0)
            .cross_align(CrossAxisAlignment::Center)
            .push(TextElement::new("(reverse-i-search)").color(Color::rgb(0.6, 0.6, 0.6)))
            .push(
                // Styled query input area
                Row::new()
                    .padding_custom(Padding::new(4.0, 8.0, 4.0, 8.0))
                    .background(Color::rgb(0.15, 0.15, 0.18))
                    .corner_radius(4.0)
                    .border(Color::rgb(0.4, 0.6, 0.8), 1.0)
                    .width(Length::Fill)
                    .push(if self.query.is_empty() {
                        TextElement::new("Type to search...").color(Color::rgb(0.4, 0.4, 0.4))
                    } else {
                        TextElement::new(self.query).color(Color::rgb(0.9, 0.9, 0.9))
                    }),
            );

        container = container.push(header);

        // Scrollable results list, max 300px tall
        if !self.results.is_empty() {
            let row_height = 30.0_f32;
            let max_height = 300.0_f32.min(self.results.len() as f32 * row_height + 4.0);

            let mut scroll = ScrollColumn::from_state(self.scroll)
                .spacing(0.0)
                .width(Length::Fill)
                .height(Length::Fixed(max_height));

            for (i, result) in self.results.iter().enumerate() {
                let is_selected = i == self.result_index;
                let is_hovered = self.hovered_index == Some(i) && !is_selected;
                let text_color = if is_selected { Color::WHITE } else { Color::rgb(0.8, 0.8, 0.8) };
                let bg = if is_selected {
                    Color::rgb(0.2, 0.4, 0.6)
                } else if is_hovered {
                    Color::rgb(0.20, 0.20, 0.25)
                } else {
                    Color::rgb(0.12, 0.12, 0.15)
                };

                let click_id = Self::result_id(i);
                scroll = scroll.push(
                    Row::new()
                        .id(click_id)
                        .padding_custom(Padding::new(6.0, 10.0, 6.0, 10.0))
                        .background(bg)
                        .corner_radius(3.0)
                        .width(Length::Fill)
                        .push(TextElement::new(result).color(text_color)),
                );
            }

            container = container.push(scroll);
        } else if !self.query.is_empty() {
            container = container.push(
                Row::new()
                    .padding_custom(Padding::new(4.0, 10.0, 4.0, 10.0))
                    .push(TextElement::new("No matches found").color(colors::TEXT_MUTED)),
            );
        }

        // Status line
        if !self.results.is_empty() {
            let status = format!("{}/{}", self.result_index + 1, self.results.len());
            container = container.push(
                Row::new()
                    .push(TextElement::new("Esc to close, Enter to select, Ctrl+R for next").color(colors::TEXT_MUTED))
                    .spacer(1.0)
                    .push(TextElement::new(status).color(colors::TEXT_MUTED)),
            );
        }

        Column::new()
            .padding_custom(Padding::new(0.0, 4.0, 2.0, 4.0))
            .width(Length::Fill)
            .push(container)
            .into()
    }
}

// Value rendering functions have been moved to widgets/value_renderer.rs
pub(crate) use crate::widgets::{render_native_value, term_color_to_strata};


#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // format_tokens tests
    // =========================================================================

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(format_tokens(0), "0 tokens");
        assert_eq!(format_tokens(1), "1 tokens");
        assert_eq!(format_tokens(100), "100 tokens");
        assert_eq!(format_tokens(999), "999 tokens");
    }

    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(format_tokens(1_000), "1.0k tokens");
        assert_eq!(format_tokens(1_500), "1.5k tokens");
        assert_eq!(format_tokens(10_000), "10.0k tokens");
        assert_eq!(format_tokens(999_999), "1000.0k tokens");
    }

    #[test]
    fn test_format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M tokens");
        assert_eq!(format_tokens(1_500_000), "1.5M tokens");
        assert_eq!(format_tokens(10_000_000), "10.0M tokens");
    }

    // format_eta tests moved to widgets/value_renderer.rs
}
