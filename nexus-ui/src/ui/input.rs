//! Input area rendering including completions and popups.

use iced::keyboard::key::Named;
use iced::keyboard::Key;
use iced::widget::{button, column, container, row, text, text_editor, text_input, Column};
use iced::{Element, Length};

use nexus_kernel::CompletionKind;

use crate::blocks::{Focus, InputMode};
use crate::msg::{InputMessage, Message, TerminalMessage};
use crate::state::InputState;
use crate::utils::{format_relative_time, shorten_path};

/// Render the input area (prompt, input field, and any popups).
pub fn view_input<'a>(
    input: &'a InputState,
    font_size: f32,
    cwd: &'a str,
    last_exit_code: Option<i32>,
    permission_denied_command: Option<&'a str>,
    focus: Focus,
) -> Element<'a, Message> {
    // Check if input should receive keyboard input
    let input_has_focus = matches!(focus, Focus::Input);
    // Cornflower blue for path
    let path_color = iced::Color::from_rgb8(100, 149, 237);
    // Green for success, red for failure
    let prompt_color = match last_exit_code {
        Some(code) if code != 0 => iced::Color::from_rgb8(220, 50, 50), // Red
        _ => iced::Color::from_rgb8(50, 205, 50),                       // Lime green
    };

    // Shorten path (replace home with ~)
    let display_path = shorten_path(cwd);

    // Calculate line height for consistent alignment between prompt and editor.
    // Using the same line_height ensures the prompt "$" aligns with the first line of text.
    let line_height = font_size * 1.4;
    let line_height_iced = iced::widget::text::LineHeight::Absolute(line_height.into());

    let path_text = text(format!("{} ", display_path))
        .size(font_size)
        .line_height(line_height_iced)
        .color(path_color)
        .font(iced::Font::MONOSPACE);

    // Mode indicator - shows SHELL or AGENT mode
    let (mode_label, mode_color) = match input.mode {
        InputMode::Shell => ("$", prompt_color),
        InputMode::Agent => ("?", iced::Color::from_rgb(0.5, 0.6, 1.0)),
    };

    let prompt = text(format!("{} ", mode_label))
        .size(font_size)
        .line_height(line_height_iced)
        .color(mode_color)
        .font(iced::Font::MONOSPACE);

    // Dynamic height: grows with content, caps at 10 lines.
    // Beyond 10 lines, the text_editor scrolls internally (keyboard/mouse wheel).
    // Note: No visible scrollbar is styled - this matches terminal UX expectations.
    let editor_height = (input.line_count() as f32 * line_height)
        .max(line_height)
        .min(line_height * 10.0);

    // Capture cursor position for history boundary detection.
    // History navigation (Up/Down) only triggers when cursor is at text boundaries.
    let (cursor_line, _) = input.cursor_position();
    let line_count = input.line_count();
    let is_on_first_line = cursor_line == 0;
    let is_on_last_line = cursor_line >= line_count.saturating_sub(1);

    // Capture completion state for key binding decisions
    let completion_visible = input.completion_visible;
    let completion_index = input.completion_index;
    let completions_len = input.completions.len();

    let input_field = text_editor(&input.content)
        .on_action(|action| Message::Input(InputMessage::EditorAction(action)))
        .key_binding(move |key_press| {
            // When a PTY block has focus, ignore all keys in the text_editor.
            // The PTY receives keyboard input through the window event handler.
            if !input_has_focus {
                return None;
            }

            // Ignore Cmd/Ctrl+key combinations - let the window handler process them.
            // This prevents the editor from inserting characters for shortcuts like Cmd+K.
            if key_press.modifiers.command() || key_press.modifiers.control() {
                // Allow Ctrl+C/V/X for clipboard operations (handled by iced internally)
                // and Ctrl+A for select all, but ignore everything else
                if let Key::Character(c) = &key_press.key {
                    match c.to_lowercase().as_str() {
                        "a" | "c" | "v" | "x" | "z" => {
                            return text_editor::Binding::from_key_press(key_press);
                        }
                        _ => return None, // Ignore other Cmd/Ctrl shortcuts
                    }
                }
                return None;
            }

            match &key_press.key {
                Key::Named(Named::Enter) => {
                    if key_press.modifiers.shift() {
                        // Shift+Enter: insert newline
                        Some(text_editor::Binding::Enter)
                    } else if completion_visible {
                        // Enter with completion visible: select current completion
                        Some(text_editor::Binding::Custom(Message::Input(
                            InputMessage::SelectCompletion(completion_index),
                        )))
                    } else {
                        // Enter: submit command
                        Some(text_editor::Binding::Custom(Message::Input(
                            InputMessage::Submit,
                        )))
                    }
                }
                Key::Named(Named::Escape) if completion_visible => {
                    // Escape: cancel completion
                    Some(text_editor::Binding::Custom(Message::Input(
                        InputMessage::CancelCompletion,
                    )))
                }
                Key::Named(Named::Tab) => {
                    if key_press.modifiers.shift() {
                        // Shift+Tab: cycle completion backwards
                        Some(text_editor::Binding::Custom(Message::Input(
                            InputMessage::TabCompletionPrev,
                        )))
                    } else {
                        // Tab: trigger/cycle completion forward
                        Some(text_editor::Binding::Custom(Message::Input(
                            InputMessage::TabCompletion,
                        )))
                    }
                }
                Key::Named(Named::ArrowUp) if !key_press.modifiers.shift() => {
                    if completion_visible && completions_len > 0 {
                        // Up with completion visible: navigate completion up
                        let new_index = if completion_index == 0 {
                            completions_len - 1
                        } else {
                            completion_index - 1
                        };
                        Some(text_editor::Binding::Custom(Message::Input(
                            InputMessage::CompletionNavigate(new_index),
                        )))
                    } else if is_on_first_line {
                        // Up on first line: navigate history
                        Some(text_editor::Binding::Custom(Message::Input(
                            InputMessage::HistoryKey(key_press.key.clone(), key_press.modifiers),
                        )))
                    } else {
                        text_editor::Binding::from_key_press(key_press)
                    }
                }
                Key::Named(Named::ArrowDown) if !key_press.modifiers.shift() => {
                    if completion_visible && completions_len > 0 {
                        // Down with completion visible: navigate completion down
                        let new_index = if completion_index >= completions_len - 1 {
                            0
                        } else {
                            completion_index + 1
                        };
                        Some(text_editor::Binding::Custom(Message::Input(
                            InputMessage::CompletionNavigate(new_index),
                        )))
                    } else if is_on_last_line {
                        // Down on last line: navigate history
                        Some(text_editor::Binding::Custom(Message::Input(
                            InputMessage::HistoryKey(key_press.key.clone(), key_press.modifiers),
                        )))
                    } else {
                        text_editor::Binding::from_key_press(key_press)
                    }
                }
                _ => text_editor::Binding::from_key_press(key_press),
            }
        })
        .height(Length::Fixed(editor_height))
        .padding(0)
        .size(font_size)
        .line_height(line_height_iced)
        .font(iced::Font::MONOSPACE)
        .style(|_theme, _status| text_editor::Style {
            background: iced::Background::Color(iced::Color::TRANSPARENT),
            border: iced::Border {
                width: 0.0,
                ..Default::default()
            },
            icon: iced::Color::from_rgb(0.5, 0.5, 0.5),
            placeholder: iced::Color::from_rgb(0.4, 0.4, 0.4),
            value: iced::Color::from_rgb(0.9, 0.9, 0.9),
            selection: iced::Color::from_rgb(0.3, 0.5, 0.8),
        });

    let input_row = row![path_text, prompt, input_field]
        .spacing(0)
        .align_y(iced::Alignment::Start);

    // Display attachments if any (Mathematica-style rich input)
    let attachments_view: Option<Element<'_, Message>> = if input.attachments.is_empty() {
        None
    } else {
        let attachment_items: Vec<Element<'_, Message>> = input
            .attachments
            .iter()
            .enumerate()
            .map(|(i, value)| match value {
                nexus_api::Value::Media {
                    data,
                    content_type,
                    metadata,
                } => {
                    let is_image = content_type.starts_with("image/");
                    let label = if is_image {
                        format!(
                            "Image {}x{}",
                            metadata.width.unwrap_or(0),
                            metadata.height.unwrap_or(0)
                        )
                    } else {
                        metadata
                            .filename
                            .clone()
                            .unwrap_or_else(|| "File".to_string())
                    };

                    // Small thumbnail for images, icon for others
                    let preview: Element<'_, Message> = if is_image {
                        // Create thumbnail preview
                        let handle = iced::widget::image::Handle::from_bytes(data.clone());
                        iced::widget::image(handle)
                            .width(Length::Fixed(60.0))
                            .height(Length::Fixed(60.0))
                            .into()
                    } else {
                        // File icon placeholder
                        text("📎").size(24.0).into()
                    };

                    let remove_btn = button(text("×").size(14.0).color(iced::Color::WHITE))
                        .on_press(Message::Input(InputMessage::RemoveAttachment(i)))
                        .padding(2)
                        .style(|_theme, _status| button::Style {
                            background: Some(iced::Background::Color(iced::Color::from_rgb(
                                0.6, 0.2, 0.2,
                            ))),
                            text_color: iced::Color::WHITE,
                            border: iced::Border {
                                radius: 10.0.into(),
                                ..Default::default()
                            },
                            ..Default::default()
                        });

                    let attachment_card = container(
                        column![
                            row![preview, remove_btn]
                                .spacing(4)
                                .align_y(iced::Alignment::Start),
                            text(label)
                                .size(font_size * 0.7)
                                .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
                        ]
                        .spacing(2)
                        .align_x(iced::Alignment::Center),
                    )
                    .padding(4)
                    .style(|_| container::Style {
                        background: Some(iced::Background::Color(iced::Color::from_rgb(
                            0.15, 0.15, 0.18,
                        ))),
                        border: iced::Border {
                            radius: 4.0.into(),
                            width: 1.0,
                            color: iced::Color::from_rgb(0.3, 0.3, 0.35),
                        },
                        ..Default::default()
                    });

                    attachment_card.into()
                }
                _ => text("?").into(),
            })
            .collect();

        Some(row(attachment_items).spacing(8).into())
    };

    // Show history search popup if active
    if input.search_active {
        return view_history_search(input, font_size, input_row);
    }

    // Show permission denied prompt if applicable
    if let Some(cmd) = permission_denied_command {
        return view_permission_denied_prompt(font_size, cmd, input_row);
    }

    // Show completion popup if visible
    if input.completion_visible && !input.completions.is_empty() {
        return view_completion_popup(input, font_size, input_row, attachments_view);
    }

    if let Some(attachments) = attachments_view {
        column![attachments, input_row].spacing(4).into()
    } else {
        input_row.into()
    }
}

/// Render the history search popup.
fn view_history_search<'a>(
    input: &'a InputState,
    font_size: f32,
    input_row: iced::widget::Row<'a, Message>,
) -> Element<'a, Message> {
    let search_label = text("(reverse-i-search)")
        .size(font_size * 0.9)
        .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
        .font(iced::Font::MONOSPACE);

    let search_input = text_input("type to search...", &input.search_query)
        .on_input(|s| Message::Input(InputMessage::HistorySearchChanged(s)))
        .padding([4, 8])
        .size(font_size)
        .style(|_theme, _status| text_input::Style {
            background: iced::Background::Color(iced::Color::from_rgb(0.15, 0.15, 0.18)),
            border: iced::Border {
                radius: 4.0.into(),
                width: 1.0,
                color: iced::Color::from_rgb(0.4, 0.6, 0.8),
            },
            icon: iced::Color::from_rgb(0.5, 0.5, 0.5),
            placeholder: iced::Color::from_rgb(0.4, 0.4, 0.4),
            value: iced::Color::from_rgb(0.9, 0.9, 0.9),
            selection: iced::Color::from_rgb(0.3, 0.5, 0.8),
        })
        .font(iced::Font::MONOSPACE);

    let search_header = row![search_label, search_input]
        .spacing(8)
        .align_y(iced::Alignment::Center);

    // Build result items
    let result_items: Vec<Element<Message>> = input
        .search_results
        .iter()
        .enumerate()
        .take(10)
        .map(|(i, entry)| {
            let is_selected = i == input.search_index;
            let bg_color = if is_selected {
                iced::Color::from_rgb(0.2, 0.4, 0.6)
            } else {
                iced::Color::from_rgb(0.12, 0.12, 0.15)
            };
            let text_color = if is_selected {
                iced::Color::WHITE
            } else {
                iced::Color::from_rgb(0.8, 0.8, 0.8)
            };
            let time_color = iced::Color::from_rgb(0.5, 0.5, 0.5);

            // Format timestamp as relative time
            let time_str = format_relative_time(&entry.timestamp);
            let command = entry.command.clone();

            let item_content = row![
                text(command)
                    .size(font_size * 0.9)
                    .color(text_color)
                    .font(iced::Font::MONOSPACE)
                    .width(Length::Fill),
                text(time_str)
                    .size(font_size * 0.8)
                    .color(time_color)
                    .font(iced::Font::MONOSPACE),
            ]
            .spacing(8)
            .align_y(iced::Alignment::Center);

            button(item_content)
                .on_press(Message::Input(InputMessage::HistorySearchSelect(i)))
                .padding([6, 10])
                .width(Length::Fill)
                .style(move |_theme, _status| button::Style {
                    background: Some(iced::Background::Color(bg_color)),
                    text_color,
                    border: iced::Border::default(),
                    ..Default::default()
                })
                .into()
        })
        .collect();

    let results_list: Element<Message> = if result_items.is_empty() {
        text("No matches found")
            .size(font_size * 0.9)
            .color(iced::Color::from_rgb(0.5, 0.5, 0.5))
            .font(iced::Font::MONOSPACE)
            .into()
    } else {
        Column::with_children(result_items).spacing(0).into()
    };

    let popup = container(column![search_header, results_list].spacing(8))
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.1, 0.1, 0.12,
            ))),
            border: iced::Border {
                radius: 6.0.into(),
                width: 1.0,
                color: iced::Color::from_rgb(0.3, 0.5, 0.7),
            },
            ..Default::default()
        })
        .padding(10)
        .width(Length::Fill);

    column![popup, input_row].spacing(8).into()
}

/// Render the permission denied prompt.
fn view_permission_denied_prompt<'a>(
    font_size: f32,
    cmd: &str,
    input_row: iced::widget::Row<'a, Message>,
) -> Element<'a, Message> {

    let warning_icon = text("⚠️").size(font_size);
    let message = text("Permission denied")
        .size(font_size * 0.95)
        .color(iced::Color::from_rgb(1.0, 0.7, 0.3))
        .font(iced::Font::MONOSPACE);
    let cmd_text = text(format!("Command: {}", cmd))
        .size(font_size * 0.85)
        .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
        .font(iced::Font::MONOSPACE);

    let retry_btn = button(text("Retry with sudo").size(font_size * 0.9))
        .on_press(Message::Terminal(TerminalMessage::RetryWithSudo))
        .padding([6, 12])
        .style(|_theme, _status| button::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.3, 0.5, 0.7,
            ))),
            text_color: iced::Color::WHITE,
            border: iced::Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });

    let dismiss_btn = button(text("Dismiss").size(font_size * 0.9))
        .on_press(Message::Terminal(TerminalMessage::DismissPermissionPrompt))
        .padding([6, 12])
        .style(|_theme, _status| button::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.25, 0.25, 0.28,
            ))),
            text_color: iced::Color::from_rgb(0.8, 0.8, 0.8),
            border: iced::Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });

    let hotkey_hint = text("Ctrl+S to retry")
        .size(font_size * 0.75)
        .color(iced::Color::from_rgb(0.5, 0.5, 0.5))
        .font(iced::Font::MONOSPACE);

    let header = row![warning_icon, message]
        .spacing(8)
        .align_y(iced::Alignment::Center);
    let buttons = row![retry_btn, dismiss_btn, hotkey_hint]
        .spacing(10)
        .align_y(iced::Alignment::Center);

    let prompt = container(column![header, cmd_text, buttons].spacing(6))
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.15, 0.12, 0.1,
            ))),
            border: iced::Border {
                radius: 6.0.into(),
                width: 1.0,
                color: iced::Color::from_rgb(0.6, 0.4, 0.2),
            },
            ..Default::default()
        })
        .padding(10)
        .width(Length::Fill);

    column![prompt, input_row].spacing(8).into()
}

/// Render the completion popup.
fn view_completion_popup<'a>(
    input: &'a InputState,
    font_size: f32,
    input_row: iced::widget::Row<'a, Message>,
    attachments_view: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let completion_items: Vec<Element<Message>> = input
        .completions
        .iter()
        .enumerate()
        .take(10) // Show max 10 items
        .map(|(i, completion)| {
            let is_selected = i == input.completion_index;
            let bg_color = if is_selected {
                iced::Color::from_rgb(0.2, 0.4, 0.6)
            } else {
                iced::Color::from_rgb(0.15, 0.15, 0.18)
            };
            let text_color = if is_selected {
                iced::Color::WHITE
            } else {
                iced::Color::from_rgb(0.8, 0.8, 0.8)
            };

            let icon = completion.kind.icon();
            let kind_color = match completion.kind {
                CompletionKind::Directory => iced::Color::from_rgb(0.4, 0.7, 1.0),
                CompletionKind::Executable | CompletionKind::NativeCommand => {
                    iced::Color::from_rgb(0.4, 0.9, 0.4)
                }
                CompletionKind::Builtin => iced::Color::from_rgb(1.0, 0.8, 0.4),
                CompletionKind::Function => iced::Color::from_rgb(0.8, 0.6, 1.0),
                CompletionKind::Variable => iced::Color::from_rgb(1.0, 0.6, 0.6),
                _ => text_color,
            };

            let item_content = row![
                text(icon).size(font_size * 0.9).color(kind_color),
                text(" ").size(font_size * 0.9),
                text(&completion.text)
                    .size(font_size * 0.9)
                    .color(text_color)
                    .font(iced::Font::MONOSPACE),
            ]
            .spacing(2)
            .align_y(iced::Alignment::Center);

            button(item_content)
                .on_press(Message::Input(InputMessage::SelectCompletion(i)))
                .padding([4, 8])
                .width(Length::Fill)
                .style(move |_theme, _status| button::Style {
                    background: Some(iced::Background::Color(bg_color)),
                    text_color,
                    border: iced::Border::default(),
                    ..Default::default()
                })
                .into()
        })
        .collect();

    let popup = container(
        Column::with_children(completion_items)
            .spacing(0)
            .width(Length::Fixed(300.0)),
    )
    .style(|_| container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgb(
            0.12, 0.12, 0.15,
        ))),
        border: iced::Border {
            radius: 4.0.into(),
            width: 1.0,
            color: iced::Color::from_rgb(0.3, 0.3, 0.35),
        },
        ..Default::default()
    })
    .padding(4);

    if let Some(attachments) = attachments_view {
        column![attachments, popup, input_row].spacing(4).into()
    } else {
        column![popup, input_row].spacing(4).into()
    }
}
