//! Orchestrator - handles message routing and cross-domain logic.
//!
//! This module is the "brain" that coordinates:
//! - Message routing to domain handlers
//! - Cross-domain action processing
//! - Event subscriptions

use std::time::Instant;

use iced::widget::scrollable;
use iced::{event, Subscription, Task};

use crate::blocks::{Focus, PtyEvent};
use crate::constants::HISTORY_SCROLLABLE;
use crate::handlers;
use crate::msg::{Action, AgentMessage, InputMessage, Message, TerminalMessage, WindowMessage};
use crate::state::Nexus;
use crate::systems::{agent_subscription, kernel_subscription, pty_subscription};

// =============================================================================
// Update
// =============================================================================

/// The update function - routes messages to domain handlers.
pub fn update(state: &mut Nexus, message: Message) -> Task<Message> {
    match message {
        Message::Input(msg) => {
            // Auto-dismiss error suggestion when user starts typing
            if matches!(msg, InputMessage::EditorAction(_)) {
                state.context.last_interaction = None;
            }
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

// =============================================================================
// Subscriptions
// =============================================================================

/// Build subscriptions for external events.
pub fn subscription(state: &Nexus) -> Subscription<Message> {
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

    // VSYNC subscription for throttled rendering
    if state.is_dirty() {
        subscriptions.push(iced::window::frames().map(|_| Message::Tick(Instant::now())));
    }

    Subscription::batch(subscriptions)
}

// =============================================================================
// Action Processing
// =============================================================================

/// Process cross-domain actions returned by handlers.
fn process_actions(state: &mut Nexus, actions: Vec<Action>) -> Task<Message> {
    let mut tasks = Vec::new();

    for action in actions {
        match action {
            Action::ExecuteCommand(cmd) => {
                transfer_attachments_to_kernel(state);
                tasks.push(handlers::terminal::execute(state, cmd));
            }
            Action::SpawnAgentQuery(query) => {
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
                let registry = handlers::window::action_registry();
                let matches = registry.search(&query, state);
                if let Some(action) = matches.get(index) {
                    if action.available(state) {
                        tasks.push(action.run(state));
                    }
                }
            }
            Action::BufferSearch => {
                perform_buffer_search(state);
            }
            Action::UpdatePaletteMatches => {
                let registry = handlers::window::action_registry();
                let matches = registry.search(&state.input.palette_query, state);
                state.input.palette_match_count = matches.len();
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

/// Perform buffer search across terminal blocks.
pub fn perform_buffer_search(state: &mut Nexus) {
    let query = state.input.buffer_search_query.to_lowercase();
    state.input.buffer_search_results.clear();

    if query.is_empty() {
        return;
    }

    for block in &state.terminal.blocks {
        let output = block.parser.grid_with_scrollback().to_string();

        for (line_num, line) in output.lines().enumerate() {
            if line.to_lowercase().contains(&query) {
                state.input.buffer_search_results.push((
                    block.id,
                    line_num + 1,
                    line.to_string(),
                ));

                if state.input.buffer_search_results.len() >= 100 {
                    return;
                }
            }
        }
    }
}
