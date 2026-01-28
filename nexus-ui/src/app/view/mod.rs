//! View composition - the main layout of the Nexus UI.
//!
//! This module composes the overall layout and delegates to sub-modules
//! for specific components (history, overlays, welcome screen).

use iced::widget::{column, container, mouse_area, scrollable, stack};
use iced::{Element, Length};

use crate::constants::HISTORY_SCROLLABLE;
use crate::msg::{Message, TerminalMessage, WindowMessage};
use crate::state::Nexus;
use crate::ui::view_input;
use crate::widgets::job_indicator::job_status_bar;

mod history;
mod overlay;
mod welcome;

/// The main view function - composes all UI components.
pub fn view(state: &Nexus) -> Element<'_, Message> {
    let font_size = state.window.font_size;

    // 1. History area (blocks or welcome screen)
    let history_content = history::view(state);
    let history = scrollable(history_content)
        .id(scrollable::Id::new(HISTORY_SCROLLABLE))
        .height(Length::Fill);

    // 2. Job status bar
    let jobs_bar = job_status_bar(&state.terminal.jobs, font_size, |id| {
        Message::Terminal(TerminalMessage::JobClicked(id))
    });

    // 3. Input line
    let input_line = container(view_input(
        &state.input,
        state.window.font_size,
        &state.terminal.cwd,
        state.terminal.last_exit_code,
        state.terminal.permission_denied_command.as_deref(),
        state.context.current_suggestion(),
        state.terminal.focus.clone(),
    ))
    .padding([8, 12])
    .width(Length::Fill)
    .style(input_line_style);

    // 4. Main composition
    let content = column![history, jobs_bar, input_line].spacing(0);

    let main_content = mouse_area(
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(background_style),
    )
    .on_press(Message::Window(WindowMessage::BackgroundClicked));

    // 5. Layer overlays on top when visible (only one at a time)
    if state.input.palette_visible {
        stack![main_content, overlay::command_palette(state, font_size)].into()
    } else if state.input.buffer_search_visible {
        stack![main_content, overlay::buffer_search(state, font_size)].into()
    } else {
        main_content.into()
    }
}

/// Style for the input line container.
fn input_line_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgba(
            0.1, 0.1, 0.12, 1.0,
        ))),
        border: iced::Border {
            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    }
}

/// Style for the main background.
fn background_style(_theme: &iced::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgb(
            0.07, 0.07, 0.09,
        ))),
        ..Default::default()
    }
}
