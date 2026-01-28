//! Overlay views: Command Palette and Buffer Search.

use iced::widget::{column, container, mouse_area, row, scrollable, text, text_input, Column, Space};
use iced::{Element, Length};

use crate::constants::{BUFFER_SEARCH_INPUT, PALETTE_INPUT, PALETTE_SCROLLABLE};
use crate::handlers;
use crate::msg::{InputMessage, Message};
use crate::state::Nexus;

/// Render the command palette overlay.
pub fn command_palette(state: &Nexus, font_size: f32) -> Element<'_, Message> {
    let registry = handlers::window::action_registry();
    let matches = registry.search(&state.input.palette_query, state);
    let query = state.input.palette_query.clone();

    // Build result list with clickable items
    let results: Vec<Element<Message>> = matches
        .iter()
        .enumerate()
        .map(|(i, action)| {
            let is_selected = i == state.input.palette_index;
            let bg_color = if is_selected {
                iced::Color::from_rgb(0.2, 0.25, 0.35)
            } else {
                iced::Color::TRANSPARENT
            };

            let keybinding_text = action
                .keybinding
                .as_ref()
                .map(|kb| kb.display())
                .unwrap_or_default();

            let query_clone = query.clone();
            let item_content = container(
                row![
                    column![
                        text(action.name).size(font_size),
                        text(action.description)
                            .size(font_size * 0.8)
                            .color(iced::Color::from_rgb(0.6, 0.6, 0.6)),
                    ]
                    .spacing(2),
                    Space::with_width(Length::Fill),
                    text(keybinding_text)
                        .size(font_size * 0.85)
                        .color(iced::Color::from_rgb(0.5, 0.5, 0.5)),
                ]
                .align_y(iced::Alignment::Center)
                .padding([8, 12]),
            )
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(iced::Background::Color(bg_color)),
                ..Default::default()
            });

            mouse_area(item_content)
                .on_press(Message::Input(InputMessage::PaletteSelect(query_clone, i)))
                .interaction(iced::mouse::Interaction::Pointer)
                .into()
        })
        .collect();

    let results_column = Column::with_children(results).spacing(0);

    let results_scrollable = scrollable(results_column)
        .id(scrollable::Id::new(PALETTE_SCROLLABLE))
        .height(Length::Shrink)
        .style(|_theme, _status| scrollable::Style {
            container: container::Style::default(),
            vertical_rail: scrollable::Rail {
                background: Some(iced::Background::Color(iced::Color::TRANSPARENT)),
                border: iced::Border::default(),
                scroller: scrollable::Scroller {
                    color: iced::Color::from_rgb(0.3, 0.3, 0.35),
                    border: iced::Border {
                        radius: 4.0.into(),
                        ..Default::default()
                    },
                },
            },
            horizontal_rail: scrollable::Rail {
                background: Some(iced::Background::Color(iced::Color::TRANSPARENT)),
                border: iced::Border::default(),
                scroller: scrollable::Scroller {
                    color: iced::Color::from_rgb(0.3, 0.3, 0.35),
                    border: iced::Border::default(),
                },
            },
            gap: None,
        });

    let search_input = text_input("Type to search actions...", &state.input.palette_query)
        .id(text_input::Id::new(PALETTE_INPUT))
        .on_input(|s| Message::Input(InputMessage::PaletteQueryChanged(s)))
        .padding([10, 12])
        .size(font_size * 1.1)
        .width(Length::Fill);

    let palette = container(
        column![
            search_input,
            container(results_scrollable).max_height(400.0),
        ]
        .spacing(0)
        .width(Length::Fixed(500.0)),
    )
    .style(|_theme| container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgb(
            0.12, 0.12, 0.15,
        ))),
        border: iced::Border {
            color: iced::Color::from_rgb(0.25, 0.25, 0.3),
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: iced::Shadow {
            color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 20.0,
        },
        ..Default::default()
    });

    // Center at top with backdrop
    mouse_area(
        container(
            column![Space::with_height(80.0), palette].align_x(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center)
        .style(backdrop_style),
    )
    .on_press(Message::Input(InputMessage::PaletteClose))
    .into()
}

/// Render the buffer search overlay.
pub fn buffer_search(state: &Nexus, font_size: f32) -> Element<'_, Message> {
    let results = &state.input.buffer_search_results;
    let match_count = results.len();

    let results_elements: Vec<Element<Message>> = results
        .iter()
        .enumerate()
        .take(15)
        .map(|(i, (_block_id, line_num, line_text))| {
            let is_selected = i == state.input.buffer_search_index;
            let bg_color = if is_selected {
                iced::Color::from_rgb(0.2, 0.25, 0.35)
            } else {
                iced::Color::TRANSPARENT
            };

            container(
                row![
                    text(format!("L{}", line_num))
                        .size(font_size * 0.8)
                        .color(iced::Color::from_rgb(0.5, 0.5, 0.5))
                        .width(Length::Fixed(50.0)),
                    text(line_text.trim())
                        .size(font_size * 0.9)
                        .color(iced::Color::from_rgb(0.85, 0.85, 0.85)),
                ]
                .spacing(8)
                .padding([6, 12]),
            )
            .width(Length::Fill)
            .style(move |_theme| container::Style {
                background: Some(iced::Background::Color(bg_color)),
                ..Default::default()
            })
            .into()
        })
        .collect();

    let results_column = Column::with_children(results_elements).spacing(0);

    let match_info = if state.input.buffer_search_query.is_empty() {
        String::new()
    } else if match_count == 0 {
        "No matches".to_string()
    } else {
        format!(
            "{} of {} matches",
            state.input.buffer_search_index + 1,
            match_count
        )
    };

    let search_row = row![
        text_input("Search in output...", &state.input.buffer_search_query)
            .id(text_input::Id::new(BUFFER_SEARCH_INPUT))
            .on_input(|s| Message::Input(InputMessage::BufferSearchChanged(s)))
            .padding([10, 12])
            .size(font_size)
            .width(Length::Fill),
        text(match_info)
            .size(font_size * 0.85)
            .color(iced::Color::from_rgb(0.5, 0.5, 0.5))
            .width(Length::Shrink),
        Space::with_width(12.0),
    ]
    .align_y(iced::Alignment::Center);

    let search_panel = container(
        column![search_row, results_column]
            .spacing(0)
            .width(Length::Fixed(600.0)),
    )
    .style(|_theme| container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgb(
            0.12, 0.12, 0.15,
        ))),
        border: iced::Border {
            color: iced::Color::from_rgb(0.25, 0.25, 0.3),
            width: 1.0,
            radius: 8.0.into(),
        },
        shadow: iced::Shadow {
            color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.5),
            offset: iced::Vector::new(0.0, 4.0),
            blur_radius: 20.0,
        },
        ..Default::default()
    });

    // Center at top with backdrop
    mouse_area(
        container(
            column![Space::with_height(80.0), search_panel].align_x(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center)
        .style(backdrop_style),
    )
    .on_press(Message::Input(InputMessage::BufferSearchClose))
    .into()
}

/// Shared backdrop style for overlays.
fn backdrop_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgba(
            0.0, 0.0, 0.0, 0.5,
        ))),
        ..Default::default()
    }
}
