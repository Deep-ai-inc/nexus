//! Agent block widget — renders an agent conversation turn.
//!
//! Contains:
//! - Query display with badge styling
//! - Collapsible thinking section
//! - Tool invocations (delegated to ToolWidget)
//! - Permission and question dialogs
//! - Response with markdown rendering
//! - Status footer with duration/cost/tokens

use nexus_api::BlockId;
use strata::content_address::SourceId;
use strata::layout::{
    ButtonElement, Column, CrossAxisAlignment, LayoutChild, Length, Padding, Row,
    TextElement, TextInputElement, Widget,
};
use strata::primitives::Color;

use crate::agent_block::{AgentBlock, AgentBlockState, PermissionRequest, PendingUserQuestion};
use crate::nexus_app::colors;
use crate::nexus_app::source_ids;
use crate::widgets::{ToolWidget, ToolMessage};

// =========================================================================
// ID Schema
// =========================================================================

// ID generation uses source_ids helpers which provide stable IDs
// via the IdSpace pattern. Tool toggles and question options
// are dynamic (indexed) so they use source_ids directly.

// =========================================================================
// Message Types
// =========================================================================

/// Messages emitted by AgentBlockWidget interactions.
#[derive(Debug, Clone)]
pub enum AgentBlockMessage {
    /// Toggle the thinking section collapse state.
    ToggleThinking,
    /// Stop the running agent.
    Stop,
    /// Permission response: deny.
    PermissionDeny,
    /// Permission response: allow once.
    PermissionAllow,
    /// Permission response: allow always.
    PermissionAlways,
    /// Tool interaction (delegated to ToolWidget).
    Tool(usize, ToolMessage),
    /// Question option selected.
    QuestionOption { question_idx: usize, option_idx: usize },
    /// Question free-form submit.
    QuestionSubmit,
}

// =========================================================================
// Widget
// =========================================================================

/// Agent block widget — renders an agent conversation turn.
pub struct AgentBlockWidget<'a> {
    pub block: &'a AgentBlock,
    /// Text input state for free-form question answers (only set when question is pending).
    pub question_input: Option<&'a strata::TextInputState>,
}

impl<'a> AgentBlockWidget<'a> {
    /// Build the view for this agent block.
    pub fn view(block: &AgentBlock, question_input: Option<&strata::TextInputState>) -> Column<'static> {
        let block_id = block.id;

        let mut content = Column::new()
            .padding(6.0)
            .spacing(3.0)
            .background(colors::BG_BLOCK)
            .corner_radius(4.0)
            .width(Length::Fill);

        // Query line (Claude Code style: > prefix with subtle badge)
        let query_source = source_ids::agent_query(block_id);
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
            let thinking_toggle_id = source_ids::agent_thinking_toggle(block_id);
            let collapse_icon = if block.thinking_collapsed { "\u{25B6}" } else { "\u{25BC}" };
            content = content.push(
                ButtonElement::new(thinking_toggle_id, &format!("{} Thinking...", collapse_icon))
                    .background(Color::TRANSPARENT)
                    .text_color(colors::TEXT_MUTED)
                    .corner_radius(2.0),
            );

            if !block.thinking_collapsed {
                // Show thinking text indented
                let thinking_source = source_ids::agent_thinking(block_id);
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
            let toggle_id = source_ids::agent_tool_toggle(block_id, i);
            let tool_source = source_ids::agent_tool(block_id, i);
            content = content.push(ToolWidget::view(tool, toggle_id, tool_source));
        }

        // Permission dialog
        if let Some(ref perm) = block.pending_permission {
            let perm_source = source_ids::agent_perm_text(block_id);
            content = content.push(build_permission_dialog(
                perm,
                source_ids::agent_perm_deny(block_id),
                source_ids::agent_perm_allow(block_id),
                source_ids::agent_perm_always(block_id),
                perm_source,
            ));
        }

        // User question dialog (AskUserQuestion via MCP permission)
        if let Some(ref question) = block.pending_question {
            let q_source = source_ids::agent_question_text(block_id);
            content = content.push(build_question_dialog(question, block_id, question_input, q_source));
        }

        // Response text (Claude Code style: bullet prefix)
        if !block.response.is_empty() {
            let response_source = source_ids::agent_response(block_id);
            content = content.push(
                Row::new()
                    .spacing(6.0)
                    .cross_align(CrossAxisAlignment::Start)
                    .push(TextElement::new("\u{25CF}").color(colors::TEXT_MUTED)) // ●
                    .push(crate::markdown::render(&block.response, response_source)),
            );
        }

        // Status footer
        let stop_id = source_ids::agent_stop(block_id);
        content = content.fixed_spacer(4.0).push(build_footer(block, stop_id));

        content
    }

    /// Try to translate a click on the given SourceId into an AgentBlockMessage.
    /// Returns None if the click doesn't belong to this block's widgets.
    pub fn on_click(block: &AgentBlock, click_id: SourceId) -> Option<AgentBlockMessage> {
        let block_id = block.id;

        // Thinking toggle
        if click_id == source_ids::agent_thinking_toggle(block_id) {
            return Some(AgentBlockMessage::ToggleThinking);
        }

        // Stop button
        if click_id == source_ids::agent_stop(block_id) {
            return Some(AgentBlockMessage::Stop);
        }

        // Permission buttons
        if click_id == source_ids::agent_perm_deny(block_id) {
            return Some(AgentBlockMessage::PermissionDeny);
        }
        if click_id == source_ids::agent_perm_allow(block_id) {
            return Some(AgentBlockMessage::PermissionAllow);
        }
        if click_id == source_ids::agent_perm_always(block_id) {
            return Some(AgentBlockMessage::PermissionAlways);
        }

        // Question submit
        if click_id == source_ids::agent_question_submit(block_id) {
            return Some(AgentBlockMessage::QuestionSubmit);
        }

        // Question options (dynamic)
        if let Some(ref question) = block.pending_question {
            for (q_idx, q) in question.questions.iter().enumerate() {
                for (o_idx, _) in q.options.iter().enumerate() {
                    if click_id == source_ids::agent_question_option(block_id, q_idx, o_idx) {
                        return Some(AgentBlockMessage::QuestionOption {
                            question_idx: q_idx,
                            option_idx: o_idx,
                        });
                    }
                }
            }
        }

        // Tool interactions (delegate to ToolWidget)
        for (i, tool) in block.tools.iter().enumerate() {
            let toggle_id = source_ids::agent_tool_toggle(block_id, i);
            if let Some(msg) = ToolWidget::on_click(toggle_id, click_id) {
                return Some(AgentBlockMessage::Tool(i, msg));
            }
        }

        None
    }
}

impl<'a> Widget<'a> for AgentBlockWidget<'a> {
    fn build(self) -> LayoutChild<'a> {
        Self::view(self.block, self.question_input).into()
    }
}

// =========================================================================
// Dialog Helpers
// =========================================================================

/// Build a permission dialog widget.
fn build_permission_dialog(
    perm: &PermissionRequest,
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

    // Permission dialog colors
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
    question: &PendingUserQuestion,
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

/// Build the status footer with stop button, status text, duration, cost, tokens.
fn build_footer(block: &AgentBlock, stop_id: SourceId) -> Row<'static> {
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
            ButtonElement::new(stop_id, "Stop")
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

    footer
}

// =========================================================================
// Helpers
// =========================================================================

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

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_tokens_small() {
        assert_eq!(format_tokens(0), "0 tokens");
        assert_eq!(format_tokens(999), "999 tokens");
    }

    #[test]
    fn test_format_tokens_thousands() {
        assert_eq!(format_tokens(1_000), "1.0k tokens");
        assert_eq!(format_tokens(10_000), "10.0k tokens");
    }

    #[test]
    fn test_format_tokens_millions() {
        assert_eq!(format_tokens(1_000_000), "1.0M tokens");
        assert_eq!(format_tokens(10_000_000), "10.0M tokens");
    }
}
