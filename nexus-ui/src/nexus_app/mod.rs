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
pub(crate) mod drag_state;
pub(crate) mod file_drop;
mod state_policy;
mod state_update;
mod state_view;

pub use message::{NexusMessage, InputMsg, ShellMsg, AgentMsg, SelectionMsg, ContextMenuMsg, DragMsg};
use context_menu::render_context_menu;
use input::InputWidget;
use scroll_model::ScrollModel;
use selection::SelectionWidget;
use shell::ShellWidget;
use agent::AgentWidget;
use transient_ui::TransientUi;

use std::cell::Cell;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, Mutex};

use nexus_kernel::Kernel;

use crate::blocks::Focus;
use crate::context::NexusContext;
use strata::component::{Component, ComponentApp, Ctx, IdSpace, RootComponent};
use strata::event_context::{CaptureState, FileDropEvent, KeyEvent, MouseEvent};
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
    pub drop_highlight: Option<message::DropZone>,
    pub(crate) drag: drag_state::DragState,

    // --- FPS tracking (Cell for interior mutability in view()) ---
    last_frame: Cell<Instant>,
    fps_smooth: Cell<f32>,
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
        // FPS calculation (exponential moving average)
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame.get()).as_secs_f32();
        self.last_frame.set(now);
        let instant_fps = if dt > 0.0 { 1.0 / dt } else { 0.0 };
        let prev = self.fps_smooth.get();
        let fps = if prev == 0.0 { instant_fps } else { prev * 0.95 + instant_fps * 0.05 };
        self.fps_smooth.set(fps);

        let vp = snapshot.viewport();
        let vw = vp.width;
        let vh = vp.height;

        let (cols, rows) = NexusState::compute_terminal_size(vw, vh);
        self.shell.terminal_size.set((cols, rows));

        let cursor_visible = self.cursor_visible();

        self.shell.clear_anchor_registry();

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

        // Drop target highlight
        if let Some(ref zone) = self.drop_highlight {
            use strata::primitives::Color;
            let accent = Color::rgba(0.3, 0.6, 1.0, 0.15);
            let border_color = Color::rgba(0.3, 0.6, 1.0, 0.8);
            // Full-window glow overlay
            let p = snapshot.overlay_primitives_mut();
            p.add_rounded_rect(Rect::new(0.0, 0.0, vw, vh), 0.0, accent);
            p.add_border(Rect::new(2.0, 2.0, vw - 4.0, vh - 4.0), 4.0, 2.0, border_color);
            // Label indicating drop zone
            let label = match zone {
                message::DropZone::InputBar => "Drop to insert path",
                message::DropZone::AgentPanel => "Drop to attach file",
                message::DropZone::ShellBlock(_) => "Drop to insert path",
                message::DropZone::Empty => "Drop to insert path",
            };
            p.add_text(
                label.to_string(),
                strata::primitives::Point::new(vw / 2.0 - 60.0, vh / 2.0),
                Color::rgba(0.8, 0.9, 1.0, 0.9),
                16.0,
            );
        }

        // FPS counter (top-right corner)
        snapshot.primitives_mut().add_text(
            format!("{:.0} FPS", fps),
            strata::primitives::Point::new(vw - 70.0, 4.0),
            colors::TEXT_MUTED,
            14.0,
        );
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

    fn on_file_drop(
        &self,
        event: FileDropEvent,
        hit: Option<HitResult>,
    ) -> Option<NexusMessage> {
        use message::FileDropMsg;
        match event {
            FileDropEvent::Hovered(path) => {
                let zone = file_drop::resolve_drop_zone(self, &hit);
                Some(NexusMessage::FileDrop(FileDropMsg::Hovered(path, zone)))
            }
            FileDropEvent::Dropped(path) => {
                let zone = file_drop::resolve_drop_zone(self, &hit);
                Some(NexusMessage::FileDrop(FileDropMsg::Dropped(path, zone)))
            }
            FileDropEvent::HoverLeft => {
                Some(NexusMessage::FileDrop(FileDropMsg::HoverLeft))
            }
        }
    }

    fn subscription(&self) -> Subscription<NexusMessage> {
        let mut subs = vec![
            self.shell.subscription(),
            self.agent.subscription(),
        ];

        if self.shell.needs_redraw() || self.agent.needs_redraw() || self.drag.auto_scroll.get().is_some() {
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

        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~".to_string());

        let context = NexusContext::new(std::env::current_dir().unwrap_or_default());

        let kernel = Arc::new(Mutex::new(kernel));

        let mut input_widget = InputWidget::new(command_history, kernel.clone());
        // Must match the initial `focus: Focus::Input` below — can't call
        // set_focus() before the state is constructed.
        input_widget.text_input.focused = true;

        let state = NexusState {
            input: input_widget,
            shell: ShellWidget::new(Arc::new(Mutex::new(kernel_rx))),
            agent: AgentWidget::new(),
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
            drop_highlight: None,
            drag: drag_state::DragState::new(),
            last_frame: Cell::new(Instant::now()),
            fps_smooth: Cell::new(0.0),
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
