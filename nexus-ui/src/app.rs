//! Main Nexus application using Iced's Elm architecture.
//!
//! This module is a thin coordinator that routes messages to domain handlers.
//! Each domain (Input, Terminal, Agent, Window) has its own handler module.
//! Cross-domain effects are handled via the Action enum.

use std::time::Instant;

use iced::widget::{column, container, scrollable, Column};
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
        .run()
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
                state.input.push_history(cmd.trim());
                tasks.push(handlers::terminal::execute(state, cmd));
            }
            Action::SpawnAgentQuery(query) => {
                transfer_attachments_to_kernel(state);
                tasks.push(handlers::agent::spawn_query(state, query));
            }
            Action::ClearAll => {
                state.agent.reset();
                state.terminal.reset();
            }
            Action::FocusInput => {
                state.terminal.focus = Focus::Input;
            }
        }
    }

    Task::batch(tasks)
}

/// Transfer pending attachments from input to kernel state.
fn transfer_attachments_to_kernel(state: &mut Nexus) {
    if state.input.attachments.is_empty() {
        return;
    }
    if let Ok(mut kernel) = state.terminal.kernel.try_lock() {
        let value = if state.input.attachments.len() == 1 {
            state.input.attachments[0].clone()
        } else {
            nexus_api::Value::List(state.input.attachments.clone())
        };
        kernel.state_mut().set_var_value("ATTACHMENT", value);
    }
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

    // Scrollable area for command history
    let history = scrollable(
        Column::with_children(content_elements)
            .spacing(4)
            .padding([10, 15]),
    )
    .id(scrollable::Id::new(HISTORY_SCROLLABLE))
    .height(Length::Fill);

    // Job status bar
    let jobs_bar = job_status_bar(&state.terminal.jobs, font_size, |id| {
        Message::Terminal(TerminalMessage::JobClicked(id))
    });

    // Input line
    let input_line = container(view_input(
        &state.input,
        state.window.font_size,
        &state.terminal.cwd,
        state.terminal.last_exit_code,
        state.terminal.permission_denied_command.as_deref(),
    ))
        .padding([8, 15])
        .width(Length::Fill);

    let content = column![history, jobs_bar, input_line].spacing(0);

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.07, 0.07, 0.09,
            ))),
            ..Default::default()
        })
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
