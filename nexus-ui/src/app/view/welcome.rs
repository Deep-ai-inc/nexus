//! Welcome screen shown when there are no commands yet.

use iced::widget::{button, column, container, row, text, Space};
use iced::{Element, Length};

use crate::msg::{InputMessage, Message};

/// Render the welcome screen.
pub fn view<'a>(font_size: f32, cwd: &str) -> Element<'a, Message> {
    let title_color = iced::Color::from_rgb(0.6, 0.8, 0.6);
    let heading_color = iced::Color::from_rgb(0.8, 0.7, 0.5);
    let text_color = iced::Color::from_rgb(0.7, 0.7, 0.7);
    let dim_color = iced::Color::from_rgb(0.5, 0.5, 0.5);
    let accent_color = iced::Color::from_rgb(0.5, 0.7, 1.0);
    let ai_color = iced::Color::from_rgb(0.6, 0.5, 0.9);
    let card_bg = iced::Color::from_rgba(1.0, 1.0, 1.0, 0.03);

    // Shorten home directory
    let home = std::env::var("HOME").unwrap_or_default();
    let display_cwd = if cwd.starts_with(&home) {
        cwd.replacen(&home, "~", 1)
    } else {
        cwd.to_string()
    };

    // ASCII art logo
    let logo = r#"
 ███╗   ██╗███████╗██╗  ██╗██╗   ██╗███████╗
 ████╗  ██║██╔════╝╚██╗██╔╝██║   ██║██╔════╝
 ██╔██╗ ██║█████╗   ╚███╔╝ ██║   ██║███████╗
 ██║╚██╗██║██╔══╝   ██╔██╗ ██║   ██║╚════██║
 ██║ ╚████║███████╗██╔╝ ██╗╚██████╔╝███████║
 ╚═╝  ╚═══╝╚══════╝╚═╝  ╚═╝ ╚═════╝ ╚══════╝"#;

    let logo_text = text(logo)
        .size(font_size * 1.1)
        .font(iced::Font::MONOSPACE)
        .color(title_color);

    let version = text("v0.1.0")
        .size(font_size * 0.9)
        .font(iced::Font::MONOSPACE)
        .color(dim_color);

    let welcome = text("Welcome to Nexus Shell")
        .size(font_size * 1.2)
        .font(iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::MONOSPACE
        })
        .color(title_color);

    let cwd_label = text(format!("  {}", display_cwd))
        .size(font_size)
        .font(iced::Font::MONOSPACE)
        .color(accent_color);

    // Shell tips
    let shell_tip1 = text("• Type any command and press Enter")
        .size(font_size * 0.9)
        .font(iced::Font::MONOSPACE)
        .color(text_color);

    let shell_tip2 = text("• Use Tab for completions")
        .size(font_size * 0.9)
        .font(iced::Font::MONOSPACE)
        .color(text_color);

    // AI tips
    let ai_tip1 = text("• Click [SH] to switch to AI mode")
        .size(font_size * 0.9)
        .font(iced::Font::MONOSPACE)
        .color(ai_color);

    let ai_tip2 = text("• Prefix with \"? \" for one-shot AI queries")
        .size(font_size * 0.9)
        .font(iced::Font::MONOSPACE)
        .color(ai_color);

    // Clickable AI example
    let try_asking_btn = button(
        text("Try: ? what files are in this directory?")
            .size(font_size * 0.85)
            .font(iced::Font::MONOSPACE),
    )
    .style(move |_theme, status| {
        let bg = match status {
            button::Status::Hovered => iced::Color::from_rgba(0.6, 0.5, 0.9, 0.2),
            button::Status::Pressed => iced::Color::from_rgba(0.6, 0.5, 0.9, 0.3),
            _ => iced::Color::from_rgba(0.6, 0.5, 0.9, 0.1),
        };
        button::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: ai_color,
            border: iced::Border {
                color: ai_color.scale_alpha(0.3),
                width: 1.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        }
    })
    .padding([4, 8])
    .on_press(Message::Input(InputMessage::SetText(
        "? what files are in this directory?".to_string(),
    )));

    // Tips card
    let tips_header = text("Getting Started")
        .size(font_size)
        .font(iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::MONOSPACE
        })
        .color(heading_color);

    let tips_card = container(
        column![
            tips_header,
            Space::with_height(8),
            shell_tip1,
            shell_tip2,
            Space::with_height(8),
            ai_tip1,
            ai_tip2,
            Space::with_height(8),
            try_asking_btn,
        ]
        .spacing(2),
    )
    .padding(12)
    .style(move |_theme| container::Style {
        background: Some(iced::Background::Color(card_bg)),
        border: iced::Border {
            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    });

    // Shortcuts section
    let shortcuts_header = text("Shortcuts")
        .size(font_size)
        .font(iced::Font {
            weight: iced::font::Weight::Bold,
            ..iced::Font::MONOSPACE
        })
        .color(heading_color);

    let shortcut1 = text("Cmd+K     Clear screen")
        .size(font_size * 0.9)
        .font(iced::Font::MONOSPACE)
        .color(text_color);

    let shortcut2 = text("Cmd++/-   Zoom in/out")
        .size(font_size * 0.9)
        .font(iced::Font::MONOSPACE)
        .color(text_color);

    let shortcut3 = text("Ctrl+R    Search history")
        .size(font_size * 0.9)
        .font(iced::Font::MONOSPACE)
        .color(text_color);

    let shortcut4 = text("Up/Down   Navigate history")
        .size(font_size * 0.9)
        .font(iced::Font::MONOSPACE)
        .color(text_color);

    let shortcuts_card = container(
        column![
            shortcuts_header,
            Space::with_height(8),
            shortcut1,
            shortcut2,
            shortcut3,
            shortcut4,
        ]
        .spacing(2),
    )
    .padding(12)
    .style(move |_theme| container::Style {
        background: Some(iced::Background::Color(card_bg)),
        border: iced::Border {
            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.06),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    });

    // Left column: logo and welcome
    let left_col = column![
        logo_text,
        Space::with_height(8),
        row![welcome, text(" ").size(font_size), version].align_y(iced::Alignment::End),
        Space::with_height(4),
        cwd_label,
    ]
    .spacing(0)
    .width(Length::FillPortion(1));

    // Right column: tips and shortcuts cards
    let right_col = column![tips_card, Space::with_height(12), shortcuts_card,]
        .spacing(0)
        .width(Length::FillPortion(1));

    container(
        row![left_col, Space::with_width(40), right_col]
            .padding([20, 20])
            .align_y(iced::Alignment::Start),
    )
    .width(Length::Fill)
    .center_x(Length::Fill)
    .into()
}
