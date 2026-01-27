//! Agent UI widgets for Iced.
//!
//! Provides widgets for rendering:
//! - Agent blocks (query + response)
//! - Tool invocations
//! - Thinking/reasoning sections
//! - Permission dialogs
//! - Streaming text

use iced::widget::{button, column, container, horizontal_rule, row, scrollable, text, Column, Row};
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

/// Colors for agent UI.
mod colors {
    use iced::Color;

    pub const THINKING_BG: Color = Color::from_rgb(0.15, 0.15, 0.2);
    pub const THINKING_BORDER: Color = Color::from_rgb(0.4, 0.4, 0.6);
    pub const TOOL_BG: Color = Color::from_rgb(0.12, 0.14, 0.16);
    pub const TOOL_BORDER: Color = Color::from_rgb(0.3, 0.35, 0.4);
    pub const TOOL_SUCCESS: Color = Color::from_rgb(0.3, 0.7, 0.4);
    pub const TOOL_ERROR: Color = Color::from_rgb(0.8, 0.3, 0.3);
    pub const TOOL_PENDING: Color = Color::from_rgb(0.6, 0.6, 0.3);
    pub const TOOL_RUNNING: Color = Color::from_rgb(0.3, 0.5, 0.8);
    pub const PERMISSION_BG: Color = Color::from_rgb(0.2, 0.15, 0.1);
    pub const PERMISSION_BORDER: Color = Color::from_rgb(0.8, 0.5, 0.2);
    pub const QUERY_COLOR: Color = Color::from_rgb(0.5, 0.7, 1.0);
    pub const RESPONSE_COLOR: Color = Color::from_rgb(0.9, 0.9, 0.9);
    pub const MUTED: Color = Color::from_rgb(0.5, 0.5, 0.5);
}

/// Render an agent block.
pub fn view_agent_block<'a>(
    block: &'a AgentBlock,
    font_size: f32,
) -> Element<'a, AgentWidgetMessage> {
    let mut content = Column::new().spacing(8).padding(10);

    // Query section
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

    container(content)
        .width(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(Color::from_rgb(0.08, 0.08, 0.1))),
            border: iced::Border {
                color: Color::from_rgb(0.2, 0.2, 0.25),
                width: 1.0,
                radius: 6.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Render the user's query.
fn view_query<'a>(query: &'a str, font_size: f32) -> Element<'a, AgentWidgetMessage> {
    row![
        text("? ").size(font_size).color(colors::QUERY_COLOR),
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
            text(if collapsed { "▶" } else { "▼" }).size(font_size * 0.8),
            text(" Thinking...").size(font_size * 0.9).color(colors::MUTED),
        ]
    )
    .on_press(AgentWidgetMessage::ToggleThinking(block_id))
    .padding(4)
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
                    .size(font_size * 0.9)
                    .color(Color::from_rgb(0.7, 0.7, 0.8))
            )
            .padding(Padding::from([4, 8]))
            .style(|_theme| container::Style {
                background: Some(iced::Background::Color(colors::THINKING_BG)),
                border: iced::Border {
                    color: colors::THINKING_BORDER,
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            })
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
            text(" ").size(font_size * 0.5),
            text(status_icon).size(font_size).color(status_color),
            text(" ").size(font_size * 0.5),
            text(&tool.name).size(font_size * 0.95),
            text(tool.message.as_deref().map(|m| format!(" - {}", m)).unwrap_or_default())
                .size(font_size * 0.85)
                .color(colors::MUTED),
        ]
    )
    .on_press(AgentWidgetMessage::ToggleTool(block_id, tool.id.clone()))
    .padding(4)
    .style(|_theme, _status| button::Style {
        background: Some(iced::Background::Color(Color::TRANSPARENT)),
        text_color: Color::WHITE,
        ..Default::default()
    });

    let mut content = Column::new().push(header);

    if !tool.collapsed {
        let mut details = Column::new().spacing(4).padding(Padding::from([4, 16]));

        // Parameters
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
                            .size(font_size * 0.85)
                            .color(colors::MUTED),
                        text(display_value)
                            .size(font_size * 0.85)
                            .color(Color::from_rgb(0.8, 0.8, 0.7)),
                    ]
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
            details = details.push(horizontal_rule(1));
            details = details.push(
                container(
                    scrollable(
                        text(display_output)
                            .size(font_size * 0.85)
                            .font(iced::Font::MONOSPACE)
                    )
                    .height(Length::Shrink)
                )
                .max_height(200)
                .style(|_theme| container::Style {
                    background: Some(iced::Background::Color(Color::from_rgb(0.05, 0.05, 0.08))),
                    ..Default::default()
                })
            );
        }

        content = content.push(
            container(details)
                .width(Length::Fill)
                .style(|_theme| container::Style {
                    background: Some(iced::Background::Color(colors::TOOL_BG)),
                    border: iced::Border {
                        color: colors::TOOL_BORDER,
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                })
        );
    }

    content.into()
}

/// Render a permission dialog.
fn view_permission_dialog<'a>(
    block_id: nexus_api::BlockId,
    perm: &'a PermissionRequest,
    font_size: f32,
) -> Element<'a, AgentWidgetMessage> {
    let title = row![
        text("⚠ ").size(font_size * 1.2).color(colors::PERMISSION_BORDER),
        text("Permission Required").size(font_size).color(colors::PERMISSION_BORDER),
    ];

    let description = text(&perm.description)
        .size(font_size * 0.95)
        .color(Color::WHITE);

    let action = container(
        text(&perm.action)
            .size(font_size * 0.9)
            .font(iced::Font::MONOSPACE)
            .color(Color::from_rgb(0.9, 0.8, 0.6))
    )
    .padding(8)
    .style(|_theme| container::Style {
        background: Some(iced::Background::Color(Color::from_rgb(0.1, 0.08, 0.05))),
        border: iced::Border {
            color: Color::from_rgb(0.3, 0.25, 0.15),
            width: 1.0,
            radius: 4.0.into(),
        },
        ..Default::default()
    });

    let working_dir = if let Some(ref dir) = perm.working_dir {
        text(format!("in {}", dir))
            .size(font_size * 0.8)
            .color(colors::MUTED)
    } else {
        text("").size(0.0)
    };

    let buttons = row![
        button(text("Deny").size(font_size * 0.9))
            .on_press(AgentWidgetMessage::PermissionDenied(block_id, perm.id.clone()))
            .padding(Padding::from([6, 16]))
            .style(|_theme, _status| button::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.5, 0.2, 0.2))),
                text_color: Color::WHITE,
                border: iced::Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        button(text("Allow Once").size(font_size * 0.9))
            .on_press(AgentWidgetMessage::PermissionGranted(block_id, perm.id.clone()))
            .padding(Padding::from([6, 16]))
            .style(|_theme, _status| button::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.2, 0.4, 0.3))),
                text_color: Color::WHITE,
                border: iced::Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        button(text("Allow Always").size(font_size * 0.9))
            .on_press(AgentWidgetMessage::PermissionGrantedSession(block_id, perm.id.clone()))
            .padding(Padding::from([6, 16]))
            .style(|_theme, _status| button::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.2, 0.5, 0.4))),
                text_color: Color::WHITE,
                border: iced::Border {
                    radius: 4.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
    ]
    .spacing(8);

    container(
        column![title, description, action, working_dir, buttons]
            .spacing(10)
            .padding(12)
    )
    .width(Length::Fill)
    .style(|_theme| container::Style {
        background: Some(iced::Background::Color(colors::PERMISSION_BG)),
        border: iced::Border {
            color: colors::PERMISSION_BORDER,
            width: 2.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// Render the response text.
fn view_response(response: &str, font_size: f32) -> Element<'_, AgentWidgetMessage> {
    // Simple markdown-ish rendering
    // We collect code blocks first, then render them
    let mut elements: Vec<Element<AgentWidgetMessage>> = Vec::new();

    let mut in_code_block = false;
    let mut code_block = String::new();

    for line in response.lines() {
        if line.starts_with("```") {
            if in_code_block {
                // End code block - push it as owned string
                let code_text = std::mem::take(&mut code_block);
                elements.push(
                    container(
                        text(code_text)
                            .size(font_size * 0.9)
                            .font(iced::Font::MONOSPACE)
                            .color(Color::from_rgb(0.8, 0.9, 0.8))
                    )
                    .padding(8)
                    .width(Length::Fill)
                    .style(|_theme| container::Style {
                        background: Some(iced::Background::Color(Color::from_rgb(0.1, 0.12, 0.1))),
                        border: iced::Border {
                            color: Color::from_rgb(0.2, 0.3, 0.2),
                            width: 1.0,
                            radius: 4.0.into(),
                        },
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
                    .size(font_size * 1.3)
                    .color(colors::RESPONSE_COLOR)
                    .into()
            );
        } else if line.starts_with("## ") {
            elements.push(
                text(line[3..].to_string())
                    .size(font_size * 1.15)
                    .color(colors::RESPONSE_COLOR)
                    .into()
            );
        } else if line.starts_with("- ") || line.starts_with("* ") {
            elements.push(
                row![
                    text("  • ").size(font_size).color(colors::MUTED),
                    text(line[2..].to_string()).size(font_size).color(colors::RESPONSE_COLOR),
                ]
                .into()
            );
        } else {
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
                    .size(font_size * 0.9)
                    .font(iced::Font::MONOSPACE)
                    .color(Color::from_rgb(0.8, 0.9, 0.8))
            )
            .padding(8)
            .width(Length::Fill)
            .style(|_theme| container::Style {
                background: Some(iced::Background::Color(Color::from_rgb(0.1, 0.12, 0.1))),
                border: iced::Border {
                    color: Color::from_rgb(0.2, 0.3, 0.2),
                    width: 1.0,
                    radius: 4.0.into(),
                },
                ..Default::default()
            })
            .into()
        );
    }

    Column::with_children(elements).spacing(4).into()
}

/// Render the status footer.
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

    // Stop button when running
    if is_running {
        let stop_btn = button(text("Stop").size(font_size * 0.75))
            .on_press(AgentWidgetMessage::Interrupt)
            .padding(Padding::from([2, 8]))
            .style(|_theme, status| {
                let base = button::Style {
                    background: Some(iced::Background::Color(Color::from_rgb(0.5, 0.2, 0.2))),
                    text_color: Color::WHITE,
                    border: iced::Border {
                        color: Color::from_rgb(0.6, 0.3, 0.3),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                };
                match status {
                    button::Status::Hovered => button::Style {
                        background: Some(iced::Background::Color(Color::from_rgb(0.6, 0.25, 0.25))),
                        ..base
                    },
                    button::Status::Pressed => button::Style {
                        background: Some(iced::Background::Color(Color::from_rgb(0.4, 0.15, 0.15))),
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

    if !block.tools.is_empty() {
        let tool_count = block.tools.len();
        let success_count = block
            .tools
            .iter()
            .filter(|t| t.status == ToolStatus::Success)
            .count();
        status = status.push(
            text(format!("Tools: {}/{}", success_count, tool_count))
                .size(font_size * 0.8)
                .color(colors::MUTED),
        );
    }

    container(status).width(Length::Fill).padding(4).into()
}
