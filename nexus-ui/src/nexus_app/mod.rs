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

pub use message::{NexusMessage, InputMsg, ShellMsg, AgentMsg, SelectionMsg, ContextMenuMsg};
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

use crate::blocks::Focus;
use crate::context::NexusContext;
use strata::component::{Component, ComponentApp, Ctx, IdSpace, RootComponent};
use strata::event_context::{CaptureState, KeyEvent, MouseEvent};
use strata::layout_snapshot::HitResult;
use strata::primitives::Rect;
use strata::{
    AppConfig, Column, Command, ImageStore, Length, MouseResponse, Padding, ScrollColumn,
    Subscription,
};

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

    // --- Layout ---
    pub window_size: (f32, f32),

    // --- UI state ---
    pub last_edit_time: Instant,
    pub exit_requested: bool,
    pub context: NexusContext,
}

// =========================================================================
// Component Implementation
// =========================================================================

impl Component for NexusState {
    type Message = NexusMessage;
    type Output = ();

    fn update(&mut self, msg: NexusMessage, ctx: &mut Ctx) -> (Command<NexusMessage>, ()) {
        if matches!(&msg, NexusMessage::Input(InputMsg::Key(_) | InputMsg::Mouse(_))) {
            self.last_edit_time = Instant::now();
        }

        let cmd = self.dispatch_update(msg, ctx);

        self.shell.sync_terminal_size();
        (cmd, ())
    }

    fn view(&self, snapshot: &mut strata::LayoutSnapshot, _ids: IdSpace) {
        let vp = snapshot.viewport();
        let vw = vp.width;
        let vh = vp.height;

        let (cols, rows) = NexusState::compute_terminal_size(vw, vh);
        self.shell.terminal_size.set((cols, rows));

        let cursor_visible = self.cursor_visible();

        let scroll = ScrollColumn::from_state(&self.scroll.state)
            .spacing(4.0)
            .width(Length::Fill)
            .height(Length::Fill);
        let scroll = self.layout_blocks(scroll);

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

        main_col = self.layout_overlays_and_input(main_col, cursor_visible);

        main_col.layout(snapshot, Rect::new(0.0, 0.0, vw, vh));

        self.sync_scroll_states(snapshot);

        if let Some(menu) = self.transient.context_menu() {
            render_context_menu(snapshot, menu);
        }
    }

    fn on_key(&self, event: KeyEvent) -> Option<NexusMessage> {
        event_routing::on_key(self, event)
    }

    fn on_mouse(
        &self,
        event: MouseEvent,
        hit: Option<HitResult>,
        capture: &CaptureState,
    ) -> MouseResponse<NexusMessage> {
        event_routing::on_mouse(self, event, hit, capture)
    }

    fn subscription(&self) -> Subscription<NexusMessage> {
        let mut subs = vec![
            self.shell.subscription(),
            self.agent.subscription(),
        ];

        if self.shell.needs_redraw() || self.agent.needs_redraw() {
            subs.push(Subscription::from_iced(
                iced::time::every(std::time::Duration::from_millis(16))
                    .map(|_| NexusMessage::Tick),
            ));
        }

        Subscription::batch(subs)
    }

    fn selection(&self) -> Option<&strata::Selection> {
        self.selection.selection.as_ref()
    }
}

impl RootComponent for NexusState {
    fn create(_images: &mut ImageStore) -> (Self, Command<NexusMessage>) {
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

        let kernel = Arc::new(Mutex::new(kernel));

        let mut input_widget = InputWidget::new(command_history, kernel.clone());
        input_widget.text_input.focused = true;

        let state = NexusState {
            input: input_widget,
            shell: ShellWidget::new(
                pty_tx,
                Arc::new(Mutex::new(pty_rx)),
                Arc::new(Mutex::new(kernel_rx)),
            ),
            agent: AgentWidget::new(
                agent_event_tx,
                Arc::new(Mutex::new(agent_event_rx)),
            ),
            selection: SelectionWidget::new(),

            scroll: ScrollModel::new(),
            transient: TransientUi::new(),

            cwd,
            next_block_id: 1,
            focus: Focus::Input,
            kernel,
            kernel_tx,

            window_size: (1200.0, 800.0),

            last_edit_time: Instant::now(),
            exit_requested: false,
            context,
        };

        (state, Command::none())
    }

    fn title(&self) -> String {
        String::from("Nexus (Strata)")
    }

    fn background_color(&self) -> strata::primitives::Color {
        colors::BG_APP
    }

    fn should_exit(&self) -> bool {
        self.exit_requested
    }
}

// =========================================================================
// Entry point
// =========================================================================

pub fn run() -> Result<(), strata::shell::Error> {
    strata::shell::run_with_config::<ComponentApp<NexusState>>(AppConfig {
        title: String::from("Nexus (Strata)"),
        window_size: (1200.0, 800.0),
        antialiasing: true,
        background_color: colors::BG_APP,
    })
}
