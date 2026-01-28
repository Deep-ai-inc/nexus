//! Main Nexus application using Iced's Elm architecture.
//!
//! This module is a thin coordinator that routes messages to domain handlers.
//! Each domain (Input, Terminal, Agent, Window) has its own handler module.
//! Cross-domain effects are handled via the Action enum.

use std::time::Instant;

use iced::widget::{
    button, column, container, mouse_area, row, scrollable, stack, text, text_input, Column, Space,
};
use iced::{event, Element, Length, Subscription, Task, Theme};

use nexus_api::BlockId;

use crate::agent_widgets::view_agent_block;
use crate::blocks::{Focus, UnifiedBlockRef};
use crate::constants::HISTORY_SCROLLABLE;
use crate::handlers;
use crate::msg::{Action, AgentMessage, Message, TerminalMessage, WindowMessage};
use crate::systems::{agent_subscription, kernel_subscription, pty_subscription};
use crate::ui::{view_block, view_input};
use crate::widgets::job_indicator::job_status_bar;

// Re-exports for backwards compatibility and public API
pub use crate::blocks::{Block, PtyEvent, UnifiedBlock};
pub use crate::constants::{CHAR_WIDTH_RATIO, LINE_HEIGHT_FACTOR};
pub use crate::msg::{GlobalShortcut, ZoomDirection};
pub use crate::state::Nexus;

/// Run the Nexus application.
pub fn run() -> iced::Result {
    iced::application("Nexus", update, view)
        .subscription(subscription)
        .theme(|_| Theme::Dark)
        .window_size(iced::Size::new(1200.0, 800.0))
        .antialiasing(true)
        .run_with(|| {
            // Focus the input field on startup
            let focus_task = iced::widget::focus_next();
            (Nexus::default(), focus_task)
        })
}

// =============================================================================
// Update - Domain Router & Action Processor
// =============================================================================

/// The update function - routes messages to domain handlers.
fn update(state: &mut Nexus, message: Message) -> Task<Message> {
    match message {
        Message::Input(msg) => {
            let result =
                handlers::input::update(&mut state.input, &state.terminal.kernel, msg);
            let action_tasks = process_actions(state, result.actions);
            Task::batch([result.task, action_tasks])
        }
        Message::Terminal(msg) => handlers::terminal::update(state, msg),
        Message::Agent(msg) => handlers::agent::update(state, msg),
        Message::Window(msg) => handlers::window::update(state, msg),
        Message::Tick(_) => handle_tick(state),
    }
}

/// Process cross-domain actions returned by handlers.
/// Returns batched tasks from all actions.
fn process_actions(state: &mut Nexus, actions: Vec<Action>) -> Task<Message> {
    let mut tasks = Vec::new();

    for action in actions {
        match action {
            Action::ExecuteCommand(cmd) => {
                transfer_attachments_to_kernel(state);
                tasks.push(handlers::terminal::execute(state, cmd));
            }
            Action::SpawnAgentQuery(query) => {
                // Take attachments directly for agent (no kernel variable needed)
                let attachments = state.input.take_attachments();
                tracing::info!(
                    "SpawnAgentQuery: taking {} attachments from input",
                    attachments.len()
                );
                tasks.push(handlers::agent::spawn_query(state, query, attachments));
            }
            Action::ClearAll => {
                state.agent.reset();
                state.terminal.reset();
            }
            Action::FocusInput => {
                state.terminal.focus = Focus::Input;
                tasks.push(iced::widget::focus_next());
            }
            Action::ExecutePaletteAction { query, index } => {
                // Look up the action in the registry and execute it
                let registry = handlers::window::action_registry();
                let matches = registry.search(&query, state);
                if let Some(action) = matches.get(index) {
                    if action.available(state) {
                        tasks.push(action.run(state));
                    }
                }
            }
            Action::BufferSearch => {
                // Perform buffer search across terminal blocks
                perform_buffer_search(state);
            }
            Action::UpdatePaletteMatches => {
                // Update palette match count for navigation bounds
                let registry = handlers::window::action_registry();
                let matches = registry.search(&state.input.palette_query, state);
                state.input.palette_match_count = matches.len();
            }
        }
    }

    Task::batch(tasks)
}

/// Transfer pending attachments from input to kernel state.
/// Uses blocking lock since dropping user data is worse than a brief block.
fn transfer_attachments_to_kernel(state: &mut Nexus) {
    if state.input.attachments.is_empty() {
        return;
    }
    // Block briefly rather than risk dropping user attachments
    let mut kernel = state.terminal.kernel.blocking_lock();
    let value = if state.input.attachments.len() == 1 {
        state.input.attachments[0].clone()
    } else {
        nexus_api::Value::List(state.input.attachments.clone())
    };
    kernel.state_mut().set_var_value("ATTACHMENT", value);
    state.input.clear_attachments();
}

/// Handle the render tick (VSync-aligned frame).
fn handle_tick(state: &mut Nexus) -> Task<Message> {
    // Check if any domain has dirty state needing render
    if state.is_dirty() {
        state.terminal.is_dirty = false;
        state.agent.is_dirty = false;
        scrollable::snap_to(
            scrollable::Id::new(HISTORY_SCROLLABLE),
            scrollable::RelativeOffset::END,
        )
    } else {
        Task::none()
    }
}

// =============================================================================
// View
// =============================================================================

fn view(state: &Nexus) -> Element<'_, Message> {
    let font_size = state.window.font_size;

    // Collect unified blocks with their IDs for sorting
    let mut unified: Vec<(BlockId, UnifiedBlockRef)> =
        Vec::with_capacity(state.terminal.blocks.len() + state.agent.blocks.len());

    for block in &state.terminal.blocks {
        unified.push((block.id, UnifiedBlockRef::Shell(block)));
    }
    for block in &state.agent.blocks {
        unified.push((block.id, UnifiedBlockRef::Agent(block)));
    }

    // Sort by BlockId (ascending) for chronological order
    unified.sort_by_key(|(id, _)| id.0);

    // Render in order
    let content_elements: Vec<Element<Message>> = unified
        .into_iter()
        .map(|(_, block_ref)| match block_ref {
            UnifiedBlockRef::Shell(block) => view_block(block, font_size),
            UnifiedBlockRef::Agent(block) => view_agent_block(block, font_size)
                .map(|msg| Message::Agent(AgentMessage::Widget(msg))),
        })
        .collect();

    // Show welcome screen when empty, otherwise show command history
    let history_content: Element<Message> = if content_elements.is_empty() {
        view_welcome(font_size, &state.terminal.cwd)
    } else {
        Column::with_children(content_elements)
            .spacing(4)
            .padding([4, 8])
            .into()
    };

    // Scrollable area for command history
    let history = scrollable(history_content)
        .id(scrollable::Id::new(HISTORY_SCROLLABLE))
        .height(Length::Fill);

    // Job status bar
    let jobs_bar = job_status_bar(&state.terminal.jobs, font_size, |id| {
        Message::Terminal(TerminalMessage::JobClicked(id))
    });

    // Input line with distinct background
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
    .style(|_theme| container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgba(
            0.1, 0.1, 0.12, 1.0,
        ))),
        border: iced::Border {
            color: iced::Color::from_rgba(1.0, 1.0, 1.0, 0.08),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    });

    let content = column![history, jobs_bar, input_line].spacing(0);

    // Main content with background
    let main_content = mouse_area(
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_theme| container::Style {
                background: Some(iced::Background::Color(iced::Color::from_rgb(
                    0.07, 0.07, 0.09,
                ))),
                ..Default::default()
            }),
    )
    .on_press(Message::Window(WindowMessage::BackgroundClicked));

    // Layer overlays on top when visible (only one at a time)
    if state.input.palette_visible {
        stack![main_content, view_command_palette(state, font_size),].into()
    } else if state.input.buffer_search_visible {
        stack![main_content, view_buffer_search(state, font_size),].into()
    } else {
        main_content.into()
    }
}

/// Render the command palette overlay.
fn view_command_palette(state: &Nexus, font_size: f32) -> Element<'_, Message> {
    use crate::msg::InputMessage;

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

            // Make item clickable - clicking executes the action directly
            mouse_area(item_content)
                .on_press(Message::Input(InputMessage::PaletteSelect(
                    query_clone,
                    i,
                )))
                .interaction(iced::mouse::Interaction::Pointer)
                .into()
        })
        .collect();

    let results_column = Column::with_children(results).spacing(0);

    // Wrap results in a scrollable with max height
    let results_scrollable = scrollable(results_column)
        .id(scrollable::Id::new(crate::constants::PALETTE_SCROLLABLE))
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

    // Search input
    let search_input = text_input("Type to search actions...", &state.input.palette_query)
        .id(text_input::Id::new(crate::constants::PALETTE_INPUT))
        .on_input(|s| Message::Input(InputMessage::PaletteQueryChanged(s)))
        .padding([10, 12])
        .size(font_size * 1.1)
        .width(Length::Fill);

    // Palette container with max height for results
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

    // Center the palette at top of screen with some margin
    // Wrap in mouse_area to close when clicking outside
    mouse_area(
        container(
            column![Space::with_height(80.0), palette,].align_x(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center)
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.0, 0.0, 0.0, 0.5,
            ))),
            ..Default::default()
        }),
    )
    .on_press(Message::Input(InputMessage::PaletteClose))
    .into()
}

/// Render the buffer search overlay.
fn view_buffer_search(state: &Nexus, font_size: f32) -> Element<'_, Message> {
    use crate::msg::InputMessage;

    let results = &state.input.buffer_search_results;
    let match_count = results.len();

    // Build result list
    let results_elements: Vec<Element<Message>> = results
        .iter()
        .enumerate()
        .take(15) // Limit to 15 results
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

    // Search input with match count
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
            .id(text_input::Id::new(crate::constants::BUFFER_SEARCH_INPUT))
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

    // Search container
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

    // Center at top of screen
    // Wrap in mouse_area to close when clicking outside
    mouse_area(
        container(
            column![Space::with_height(80.0), search_panel,].align_x(iced::Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Center)
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgba(
                0.0, 0.0, 0.0, 0.5,
            ))),
            ..Default::default()
        }),
    )
    .on_press(Message::Input(InputMessage::BufferSearchClose))
    .into()
}

/// Perform buffer search across terminal blocks.
fn perform_buffer_search(state: &mut Nexus) {
    let query = state.input.buffer_search_query.to_lowercase();
    state.input.buffer_search_results.clear();

    if query.is_empty() {
        return;
    }

    // Search through all terminal blocks
    for block in &state.terminal.blocks {
        let output = block.parser.grid_with_scrollback().to_string();

        for (line_num, line) in output.lines().enumerate() {
            if line.to_lowercase().contains(&query) {
                state.input.buffer_search_results.push((
                    block.id,
                    line_num + 1, // 1-indexed
                    line.to_string(),
                ));

                // Limit total results
                if state.input.buffer_search_results.len() >= 100 {
                    return;
                }
            }
        }
    }
}

/// Render the welcome screen shown when there are no commands yet.
fn view_welcome<'a>(font_size: f32, cwd: &str) -> Element<'a, Message> {
    use crate::msg::InputMessage;

    let title_color = iced::Color::from_rgb(0.6, 0.8, 0.6);
    let heading_color = iced::Color::from_rgb(0.8, 0.7, 0.5);
    let text_color = iced::Color::from_rgb(0.7, 0.7, 0.7);
    let dim_color = iced::Color::from_rgb(0.5, 0.5, 0.5);
    let accent_color = iced::Color::from_rgb(0.5, 0.7, 1.0);
    let ai_color = iced::Color::from_rgb(0.6, 0.5, 0.9); // Purple for AI tips
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

    // AI tips (purple colored)
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

    // Shortcuts card
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

// =============================================================================
// Subscriptions
// =============================================================================

fn subscription(state: &Nexus) -> Subscription<Message> {
    let mut subscriptions = vec![
        // PTY events -> Terminal messages
        pty_subscription(state.terminal.pty_rx.clone()).map(|(id, evt)| match evt {
            PtyEvent::Output(data) => Message::Terminal(TerminalMessage::PtyOutput(id, data)),
            PtyEvent::Exited(code) => Message::Terminal(TerminalMessage::PtyExited(id, code)),
        }),
        // Kernel events -> Terminal messages
        kernel_subscription(state.terminal.kernel_rx.clone())
            .map(|evt| Message::Terminal(TerminalMessage::KernelEvent(evt))),
        // Agent events -> Agent messages
        agent_subscription(state.agent.event_rx.clone())
            .map(|evt| Message::Agent(AgentMessage::Event(evt))),
    ];

    // Listen for all events with window ID -> Window messages
    subscriptions.push(event::listen_with(|event, _status, window_id| {
        Some(Message::Window(WindowMessage::Event(event, window_id)))
    }));

    // VSYNC SUBSCRIPTION for throttled rendering
    if state.is_dirty() {
        subscriptions.push(iced::window::frames().map(|_| Message::Tick(Instant::now())));
    }

    Subscription::batch(subscriptions)
}
