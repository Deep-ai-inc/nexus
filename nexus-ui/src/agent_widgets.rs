//! Agent UI widgets for Iced.
//!
//! Provides widgets for rendering:
//! - Agent blocks (query + response)
//! - Tool invocations
//! - Thinking/reasoning sections
//! - Permission dialogs
//! - Streaming text

use iced::widget::{button, column, container, row, scrollable, text, Column, Row};
use iced::{Color, Element, Length, Padding};

use crate::agent_block::{AgentBlock, AgentBlockState, PermissionRequest, ToolInvocation, ToolStatus};

/// Messages from agent widgets.
#[derive(Debug, Clone)]
pub enum AgentWidgetMessage {
    /// Toggle thinking section collapsed state.
    ToggleThinking(nexus_api::BlockId),
    /// Toggle tool collapsed state.
    ToggleTool(nexus_api::BlockId, String),
    /// User granted permission.
    PermissionGranted(nexus_api::BlockId, String),
    /// User granted permission for session.
    PermissionGrantedSession(nexus_api::BlockId, String),
    /// User denied permission.
    PermissionDenied(nexus_api::BlockId, String),
    /// Copy text to clipboard.
    CopyText(String),
    /// Interrupt/stop the running agent.
    Interrupt,
}

/// Colors for agent UI - simplified to match shell mode.
mod colors {
    use iced::Color;

    pub const TOOL_SUCCESS: Color = Color::from_rgb(0.3, 0.8, 0.3);
    pub const TOOL_ERROR: Color = Color::from_rgb(0.8, 0.3, 0.3);
    pub const TOOL_PENDING: Color = Color::from_rgb(0.6, 0.6, 0.3);
    pub const TOOL_RUNNING: Color = Color::from_rgb(0.3, 0.7, 1.0);
    pub const PERMISSION_BG: Color = Color::from_rgb(0.15, 0.1, 0.05);
    pub const PERMISSION_BORDER: Color = Color::from_rgb(0.8, 0.5, 0.2);
    pub const QUERY_COLOR: Color = Color::from_rgb(0.5, 0.7, 1.0);
    pub const RESPONSE_COLOR: Color = Color::from_rgb(0.9, 0.9, 0.9);
    pub const MUTED: Color = Color::from_rgb(0.5, 0.5, 0.5);
    pub const CODE_BG: Color = Color::from_rgb(0.06, 0.06, 0.08);
}

/// Render an agent block.
pub fn view_agent_block<'a>(
    block: &'a AgentBlock,
    font_size: f32,
) -> Element<'a, AgentWidgetMessage> {
    let mut content = Column::new().spacing(4);

    // Query section - styled like shell prompt
    content = content.push(view_query(&block.query, font_size));

    // Thinking section (if any)
    if !block.thinking.is_empty() {
        content = content.push(view_thinking(
            block.id,
            &block.thinking,
            block.thinking_collapsed,
            font_size,
        ));
    }

    // Tool invocations
    for tool in &block.tools {
        content = content.push(view_tool(block.id, tool, font_size));
    }

    // Permission dialog (if pending)
    if let Some(ref perm) = block.pending_permission {
        content = content.push(view_permission_dialog(block.id, perm, font_size));
    }

    // Response text
    if !block.response.is_empty() {
        content = content.push(view_response(&block.response, font_size));
    }

    // Images
    for _image in &block.images {
        // TODO: Render image when iced image support is added
        content = content.push(text("[Image]").size(font_size).color(colors::MUTED));
    }

    // Status footer
    content = content.push(view_status(block, font_size));

    // No outer border - matches shell blocks' clean look
    content.width(Length::Fill).into()
}

/// Render the user's query - styled like shell prompt.
fn view_query<'a>(query: &'a str, font_size: f32) -> Element<'a, AgentWidgetMessage> {
    row![
        text("? ")
            .size(font_size)
            .color(colors::QUERY_COLOR)
            .font(iced::Font::MONOSPACE),
        text(query).size(font_size).color(colors::QUERY_COLOR),
    ]
    .into()
}

/// Render the thinking/reasoning section.
fn view_thinking<'a>(
    block_id: nexus_api::BlockId,
    thinking: &'a str,
    collapsed: bool,
    font_size: f32,
) -> Element<'a, AgentWidgetMessage> {
    let header = button(
        row![
            text(if collapsed { "▶" } else { "▼" })
                .size(font_size * 0.8)
                .color(colors::MUTED),
            text(" Thinking...")
                .size(font_size * 0.85)
                .color(colors::MUTED),
        ]
    )
    .on_press(AgentWidgetMessage::ToggleThinking(block_id))
    .padding(2)
    .style(|_theme, _status| button::Style {
        background: Some(iced::Background::Color(Color::TRANSPARENT)),
        text_color: colors::MUTED,
        ..Default::default()
    });

    let mut content = Column::new().push(header);

    if !collapsed {
        content = content.push(
            container(
                text(thinking)
                    .size(font_size * 0.85)
                    .color(Color::from_rgb(0.6, 0.6, 0.7))
            )
            .padding(Padding::from([2, 16]))
        );
    }

    content.into()
}

/// Render a tool invocation.
fn view_tool<'a>(
    block_id: nexus_api::BlockId,
    tool: &'a ToolInvocation,
    font_size: f32,
) -> Element<'a, AgentWidgetMessage> {
    let status_color = match tool.status {
        ToolStatus::Pending => colors::TOOL_PENDING,
        ToolStatus::Running => colors::TOOL_RUNNING,
        ToolStatus::Success => colors::TOOL_SUCCESS,
        ToolStatus::Error => colors::TOOL_ERROR,
    };

    let status_icon = match tool.status {
        ToolStatus::Pending => "◯",
        ToolStatus::Running => "●",
        ToolStatus::Success => "✓",
        ToolStatus::Error => "✗",
    };

    let header = button(
        row![
            text(if tool.collapsed { "▶" } else { "▼" })
                .size(font_size * 0.8)
                .color(colors::MUTED),
            text(status_icon)
                .size(font_size)
                .color(status_color)
                .font(iced::Font::MONOSPACE),
            text(&tool.name)
                .size(font_size * 0.9)
                .color(Color::from_rgb(0.8, 0.8, 0.9)),
            text(tool.message.as_deref().map(|m| format!(" {}", m)).unwrap_or_default())
                .size(font_size * 0.85)
                .color(colors::MUTED),
        ]
        .spacing(4)
    )
    .on_press(AgentWidgetMessage::ToggleTool(block_id, tool.id.clone()))
    .padding(2)
    .style(|_theme, _status| button::Style {
        background: Some(iced::Background::Color(Color::TRANSPARENT)),
        text_color: Color::WHITE,
        ..Default::default()
    });

    let mut content = Column::new().push(header);

    if !tool.collapsed {
        let mut details = Column::new().spacing(2).padding(Padding::from([0, 16]));

        // Parameters - more compact
        if !tool.parameters.is_empty() {
            for (name, value) in &tool.parameters {
                let display_value = if value.len() > 100 {
                    format!("{}...", &value[..100])
                } else {
                    value.clone()
                };
                details = details.push(
                    row![
                        text(format!("{}: ", name))
                            .size(font_size * 0.8)
                            .color(colors::MUTED)
                            .font(iced::Font::MONOSPACE),
                        text(display_value)
                            .size(font_size * 0.8)
                            .color(Color::from_rgb(0.7, 0.7, 0.6))
                            .font(iced::Font::MONOSPACE),
                    ]
                );
            }
        }

        // Output - no border, just indented monospace
        if let Some(ref output) = tool.output {
            let display_output = if output.len() > 500 {
                format!("{}...\n[{} more chars]", &output[..500], output.len() - 500)
            } else {
                output.clone()
            };
            details = details.push(
                container(
                    scrollable(
                        text(display_output)
                            .size(font_size * 0.8)
                            .font(iced::Font::MONOSPACE)
                            .color(Color::from_rgb(0.7, 0.7, 0.7))
                    )
                    .height(Length::Shrink)
                )
                .max_height(200)
                .padding(Padding::from([4, 0]))
            );
        }

        content = content.push(details);
    }

    content.into()
}

/// Render a permission dialog - slightly more prominent but still clean.
fn view_permission_dialog<'a>(
    block_id: nexus_api::BlockId,
    perm: &'a PermissionRequest,
    font_size: f32,
) -> Element<'a, AgentWidgetMessage> {
    let title = row![
        text("⚠ ")
            .size(font_size)
            .color(colors::PERMISSION_BORDER)
            .font(iced::Font::MONOSPACE),
        text("Permission Required")
            .size(font_size * 0.9)
            .color(colors::PERMISSION_BORDER),
    ];

    let description = text(&perm.description)
        .size(font_size * 0.9)
        .color(Color::from_rgb(0.85, 0.85, 0.85));

    let action = container(
        text(&perm.action)
            .size(font_size * 0.85)
            .font(iced::Font::MONOSPACE)
            .color(Color::from_rgb(0.9, 0.8, 0.6))
    )
    .padding(Padding::from([4, 8]))
    .style(|_theme| container::Style {
        background: Some(iced::Background::Color(colors::CODE_BG)),
        ..Default::default()
    });

    let working_dir = if let Some(ref dir) = perm.working_dir {
        text(format!("in {}", dir))
            .size(font_size * 0.8)
            .color(colors::MUTED)
    } else {
        text("").size(0.0)
    };

    // Minimal button styling
    let button_style = |bg_color: Color| {
        move |_theme: &iced::Theme, status: button::Status| {
            let base = button::Style {
                background: Some(iced::Background::Color(bg_color)),
                text_color: Color::WHITE,
                ..Default::default()
            };
            match status {
                button::Status::Hovered => button::Style {
                    background: Some(iced::Background::Color(Color::from_rgb(
                        bg_color.r * 1.2,
                        bg_color.g * 1.2,
                        bg_color.b * 1.2,
                    ))),
                    ..base
                },
                _ => base,
            }
        }
    };

    let buttons = row![
        button(text("Deny").size(font_size * 0.85))
            .on_press(AgentWidgetMessage::PermissionDenied(block_id, perm.id.clone()))
            .padding(Padding::from([4, 12]))
            .style(button_style(Color::from_rgb(0.4, 0.15, 0.15))),
        button(text("Allow Once").size(font_size * 0.85))
            .on_press(AgentWidgetMessage::PermissionGranted(block_id, perm.id.clone()))
            .padding(Padding::from([4, 12]))
            .style(button_style(Color::from_rgb(0.15, 0.3, 0.2))),
        button(text("Allow Always").size(font_size * 0.85))
            .on_press(AgentWidgetMessage::PermissionGrantedSession(block_id, perm.id.clone()))
            .padding(Padding::from([4, 12]))
            .style(button_style(Color::from_rgb(0.15, 0.35, 0.25))),
    ]
    .spacing(6);

    // Keep a subtle background to make it stand out but no border
    container(
        column![title, description, action, working_dir, buttons]
            .spacing(6)
            .padding(8)
    )
    .width(Length::Fill)
    .style(|_theme| container::Style {
        background: Some(iced::Background::Color(colors::PERMISSION_BG)),
        ..Default::default()
    })
    .into()
}

/// Render the response text.
/// Uses proportional font for prose, monospace for code blocks.
fn view_response(response: &str, font_size: f32) -> Element<'_, AgentWidgetMessage> {
    let mut elements: Vec<Element<AgentWidgetMessage>> = Vec::new();

    let mut in_code_block = false;
    let mut code_block = String::new();

    for line in response.lines() {
        if line.starts_with("```") {
            if in_code_block {
                // End code block - minimal styling, just monospace on dark bg
                let code_text = std::mem::take(&mut code_block);
                elements.push(
                    container(
                        text(code_text)
                            .size(font_size * 0.85)
                            .font(iced::Font::MONOSPACE)
                            .color(Color::from_rgb(0.8, 0.85, 0.8))
                    )
                    .padding(Padding::from([6, 12]))
                    .width(Length::Fill)
                    .style(|_theme| container::Style {
                        background: Some(iced::Background::Color(colors::CODE_BG)),
                        ..Default::default()
                    })
                    .into()
                );
                in_code_block = false;
            } else {
                in_code_block = true;
            }
        } else if in_code_block {
            if !code_block.is_empty() {
                code_block.push('\n');
            }
            code_block.push_str(line);
        } else if line.starts_with("# ") {
            elements.push(
                text(line[2..].to_string())
                    .size(font_size * 1.2)
                    .color(colors::RESPONSE_COLOR)
                    .into()
            );
        } else if line.starts_with("## ") {
            elements.push(
                text(line[3..].to_string())
                    .size(font_size * 1.1)
                    .color(colors::RESPONSE_COLOR)
                    .into()
            );
        } else if line.starts_with("**") && line.ends_with("**") {
            // Bold text (headers like **Repository context:**)
            elements.push(
                text(line[2..line.len()-2].to_string())
                    .size(font_size)
                    .color(colors::RESPONSE_COLOR)
                    .into()
            );
        } else if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("· ") {
            elements.push(
                row![
                    text("  · ").size(font_size).color(colors::MUTED),
                    text(line[2..].to_string()).size(font_size).color(colors::RESPONSE_COLOR),
                ]
                .into()
            );
        } else if line.is_empty() {
            // Empty line - add small spacing
            elements.push(text("").size(font_size * 0.5).into());
        } else {
            // Regular prose - proportional font (default)
            elements.push(
                text(line.to_string()).size(font_size).color(colors::RESPONSE_COLOR).into()
            );
        }
    }

    // Handle unclosed code block
    if in_code_block && !code_block.is_empty() {
        elements.push(
            container(
                text(code_block)
                    .size(font_size * 0.85)
                    .font(iced::Font::MONOSPACE)
                    .color(Color::from_rgb(0.8, 0.85, 0.8))
            )
            .padding(Padding::from([6, 12]))
            .width(Length::Fill)
            .style(|_theme| container::Style {
                background: Some(iced::Background::Color(colors::CODE_BG)),
                ..Default::default()
            })
            .into()
        );
    }

    Column::with_children(elements).spacing(2).into()
}

/// Render the status footer - compact like shell block status.
fn view_status<'a>(block: &'a AgentBlock, font_size: f32) -> Element<'a, AgentWidgetMessage> {
    let is_running = matches!(
        block.state,
        AgentBlockState::Pending
            | AgentBlockState::Streaming
            | AgentBlockState::Thinking
            | AgentBlockState::Executing
    );

    let (status_text, status_color) = match &block.state {
        AgentBlockState::Pending => ("Waiting...", colors::MUTED),
        AgentBlockState::Streaming => ("Streaming...", colors::TOOL_RUNNING),
        AgentBlockState::Thinking => ("Thinking...", Color::from_rgb(0.6, 0.5, 0.8)),
        AgentBlockState::Executing => ("Executing...", colors::TOOL_RUNNING),
        AgentBlockState::Completed => ("Completed", colors::TOOL_SUCCESS),
        AgentBlockState::Failed(err) => (err.as_str(), colors::TOOL_ERROR),
        AgentBlockState::AwaitingPermission => ("Awaiting permission...", colors::PERMISSION_BORDER),
        AgentBlockState::Interrupted => ("Interrupted", colors::MUTED),
    };

    let mut status = Row::new().spacing(8).align_y(iced::Alignment::Center);

    // Stop button when running - minimal style
    if is_running {
        let stop_btn = button(
            text("Stop")
                .size(font_size * 0.75)
                .color(Color::from_rgb(0.9, 0.5, 0.5))
        )
            .on_press(AgentWidgetMessage::Interrupt)
            .padding(Padding::from([2, 6]))
            .style(|_theme, status| {
                let base = button::Style {
                    background: Some(iced::Background::Color(Color::TRANSPARENT)),
                    text_color: Color::from_rgb(0.9, 0.5, 0.5),
                    ..Default::default()
                };
                match status {
                    button::Status::Hovered => button::Style {
                        background: Some(iced::Background::Color(Color::from_rgb(0.3, 0.15, 0.15))),
                        ..base
                    },
                    _ => base,
                }
            });
        status = status.push(stop_btn);
    }

    status = status.push(text(status_text).size(font_size * 0.8).color(status_color));

    if let Some(ms) = block.duration_ms {
        let duration = if ms < 1000 {
            format!("{}ms", ms)
        } else {
            format!("{:.1}s", ms as f64 / 1000.0)
        };
        status = status.push(text(duration).size(font_size * 0.8).color(colors::MUTED));
    }

    status.into()
}
