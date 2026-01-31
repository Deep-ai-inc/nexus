//! Nexus Strata Application — Slim Orchestrator
//!
//! Routes messages to child widgets and processes their typed outputs.
//! Each widget owns its state and children; the orchestrator owns the widgets
//! and shared context (kernel, cwd, focus).

pub(crate) mod colors;
pub(crate) mod source_ids;
pub(crate) mod context_menu;
mod message;
pub(crate) mod completion;
pub(crate) mod history_search;
pub(crate) mod input;
pub(crate) mod selection;
pub(crate) mod shell;
pub(crate) mod agent;
mod event_routing;
pub(crate) mod scroll_model;
pub(crate) mod transient_ui;
mod state_policy;
mod state_update;
mod state_view;

pub use message::NexusMessage;
use context_menu::render_context_menu;
use input::InputWidget;
use scroll_model::ScrollModel;
use selection::SelectionWidget;
use shell::ShellWidget;
use agent::AgentWidget;
use transient_ui::TransientUi;

use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, mpsc, Mutex};

use nexus_kernel::Kernel;

use crate::blocks::{Focus, PtyEvent};
use crate::context::NexusContext;
use strata::event_context::{CaptureState, KeyEvent, MouseEvent};
use strata::layout_snapshot::HitResult;
use strata::primitives::Rect;
use strata::{
    AppConfig, Column, Command, ImageStore, Length, MouseResponse, Padding, ScrollColumn,
    StrataApp, Subscription,
};
use crate::systems::{agent_subscription, kernel_subscription, pty_subscription};

// =========================================================================
// Attachment (clipboard image paste)
// =========================================================================

pub struct Attachment {
    pub data: Vec<u8>,
    pub image_handle: strata::ImageHandle,
    pub width: u32,
    pub height: u32,
}

// =========================================================================
// Application State
// =========================================================================

pub struct NexusState {
    // --- Widgets ---
    pub(crate) input: InputWidget,
    pub(crate) shell: ShellWidget,
    pub(crate) agent: AgentWidget,
    pub(crate) selection: SelectionWidget,

    // --- Subsystems ---
    pub(crate) scroll: ScrollModel,
    pub(crate) transient: TransientUi,

    // --- Shared context ---
    pub cwd: String,
    pub next_block_id: u64,
    pub focus: Focus,
    pub kernel: Arc<Mutex<Kernel>>,
    pub kernel_tx: broadcast::Sender<nexus_api::ShellEvent>,
    pub kernel_rx: Arc<Mutex<broadcast::Receiver<nexus_api::ShellEvent>>>,
    pub pty_rx: Arc<Mutex<mpsc::UnboundedReceiver<(nexus_api::BlockId, PtyEvent)>>>,
    pub agent_event_rx: Arc<Mutex<mpsc::UnboundedReceiver<crate::agent_adapter::AgentEvent>>>,

    // --- Layout ---
    pub window_size: (f32, f32),

    // --- UI state ---
    pub last_edit_time: Instant,
    pub exit_requested: bool,
    pub context: NexusContext,
}

// =========================================================================
// ApplyOutput — enforced convention for domain output processing
// =========================================================================

pub(crate) trait ApplyOutput<O> {
    fn apply_output(&mut self, output: O);
}

// =========================================================================
// StrataApp Implementation
// =========================================================================

pub struct NexusApp;

impl StrataApp for NexusApp {
    type State = NexusState;
    type Message = NexusMessage;

    fn init(_images: &mut ImageStore) -> (Self::State, Command<Self::Message>) {
        let (kernel, kernel_rx) = Kernel::new().expect("Failed to create kernel");
        let kernel_tx = kernel.event_sender().clone();

        let command_history: Vec<String> = kernel
            .store()
            .and_then(|store| store.get_recent_history(1000).ok())
            .map(|entries| entries.into_iter().rev().map(|e| e.command).collect())
            .unwrap_or_default();

        let (pty_tx, pty_rx) = mpsc::unbounded_channel();
        let (agent_event_tx, agent_event_rx) = mpsc::unbounded_channel();

        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~".to_string());

        let context = NexusContext::new(std::env::current_dir().unwrap_or_default());

        let mut input_widget = InputWidget::new(command_history);
        input_widget.text_input.focused = true;

        let state = NexusState {
            input: input_widget,
            shell: ShellWidget::new(pty_tx),
            agent: AgentWidget::new(agent_event_tx),
            selection: SelectionWidget::new(),

            scroll: ScrollModel::new(),
            transient: TransientUi::new(),

            cwd,
            next_block_id: 1,
            focus: Focus::Input,
            kernel: Arc::new(Mutex::new(kernel)),
            kernel_tx,
            kernel_rx: Arc::new(Mutex::new(kernel_rx)),
            pty_rx: Arc::new(Mutex::new(pty_rx)),
            agent_event_rx: Arc::new(Mutex::new(agent_event_rx)),

            window_size: (1200.0, 800.0),

            last_edit_time: Instant::now(),
            exit_requested: false,
            context,
        };

        (state, Command::none())
    }

    fn update(
        state: &mut Self::State,
        message: Self::Message,
        images: &mut ImageStore,
    ) -> Command<Self::Message> {
        if matches!(&message, NexusMessage::InputKey(_) | NexusMessage::InputMouse(_)) {
            state.last_edit_time = Instant::now();
        }

        let cmd = state.update(message, images);

        state.shell.sync_terminal_size();
        cmd
    }

    fn view(state: &Self::State, snapshot: &mut strata::LayoutSnapshot) {
        let vp = snapshot.viewport();
        let vw = vp.width;
        let vh = vp.height;

        let (cols, rows) = NexusState::compute_terminal_size(vw, vh);
        state.shell.terminal_size.set((cols, rows));

        let cursor_visible = state.cursor_visible();

        let scroll = ScrollColumn::from_state(&state.scroll.state)
            .spacing(4.0)
            .width(Length::Fill)
            .height(Length::Fill);
        let scroll = state.layout_blocks(scroll);

        let mut main_col = Column::new()
            .width(Length::Fixed(vw))
            .height(Length::Fixed(vh))
            .padding(0.0);

        main_col = main_col.push(
            Column::new()
                .padding_custom(Padding::new(2.0, 4.0, 0.0, 4.0))
                .width(Length::Fill)
                .height(Length::Fill)
                .push(scroll),
        );

        main_col = state.layout_overlays(main_col);
        main_col = state.layout_attachments(main_col);
        main_col = state.layout_input_bar(main_col, cursor_visible);

        main_col.layout(snapshot, Rect::new(0.0, 0.0, vw, vh));

        state.sync_scroll_states(snapshot);

        if let Some(menu) = state.transient.context_menu() {
            render_context_menu(snapshot, menu);
        }
    }

    fn selection(state: &Self::State) -> Option<&strata::Selection> {
        state.selection.selection.as_ref()
    }

    fn on_mouse(
        state: &Self::State,
        event: MouseEvent,
        hit: Option<HitResult>,
        capture: &CaptureState,
    ) -> MouseResponse<Self::Message> {
        event_routing::on_mouse(state, event, hit, capture)
    }

    fn on_key(state: &Self::State, event: KeyEvent) -> Option<Self::Message> {
        event_routing::on_key(state, event)
    }

    fn subscription(state: &Self::State) -> Subscription<Self::Message> {
        let mut subs = Vec::new();

        let pty_rx = state.pty_rx.clone();
        subs.push(Subscription::from_iced(
            pty_subscription(pty_rx).map(|(id, evt)| match evt {
                PtyEvent::Output(data) => NexusMessage::PtyOutput(id, data),
                PtyEvent::Exited(code) => NexusMessage::PtyExited(id, code),
            }),
        ));

        let kernel_rx = state.kernel_rx.clone();
        subs.push(Subscription::from_iced(
            kernel_subscription(kernel_rx).map(NexusMessage::KernelEvent),
        ));

        let agent_rx = state.agent_event_rx.clone();
        subs.push(Subscription::from_iced(
            agent_subscription(agent_rx).map(NexusMessage::AgentEvent),
        ));

        if state.shell.terminal_dirty || state.agent.dirty {
            subs.push(Subscription::from_iced(
                iced::time::every(std::time::Duration::from_millis(16))
                    .map(|_| NexusMessage::Tick),
            ));
        }

        Subscription::batch(subs)
    }

    fn title(_state: &Self::State) -> String {
        String::from("Nexus (Strata)")
    }

    fn background_color(_state: &Self::State) -> strata::primitives::Color {
        colors::BG_APP
    }

    fn should_exit(state: &Self::State) -> bool {
        state.exit_requested
    }
}

// =========================================================================
// Entry point
// =========================================================================

pub fn run() -> Result<(), strata::shell::Error> {
    strata::shell::run_with_config::<NexusApp>(AppConfig {
        title: String::from("Nexus (Strata)"),
        window_size: (1200.0, 800.0),
        antialiasing: true,
        background_color: colors::BG_APP,
    })
}
