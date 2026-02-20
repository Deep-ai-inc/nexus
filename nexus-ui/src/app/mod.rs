//! Nexus Strata Application — Slim Orchestrator
//!
//! Routes messages to child widgets and processes their typed outputs.
//! Each widget owns its state and children; the orchestrator owns the widgets
//! and shared context (kernel, cwd, focus).

pub(crate) mod message;
pub(crate) mod update_context;
mod routing;
mod actions;
mod update;
mod view;
#[cfg(test)]
mod tests;

use message::{NexusMessage, InputMsg};
use crate::ui::context_menu::render_context_menu;
use crate::features::input::InputWidget;
use crate::ui::scroll::ScrollModel;
use crate::features::selection::SelectionWidget;
use crate::features::shell::ShellWidget;
use crate::features::agent::AgentWidget;
use crate::ui::transient::TransientUi;

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use tokio::sync::{broadcast, Mutex};

use nexus_kernel::Kernel;

use crate::data::Focus;
use crate::data::context::NexusContext;
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
// Shared State (across all windows)
// =========================================================================

/// State shared across all Nexus windows.
///
/// Deliberately minimal — only truly global state lives here.
/// Each window gets its own Kernel (own CWD, variables, last_output).
/// History is naturally shared because all Kernels read/write the same
/// native shell history file.
#[derive(Clone)]
pub struct NexusShared {
    /// Global block ID counter — ensures unique IDs across all windows.
    pub next_block_id: Arc<AtomicU64>,
}

impl Default for NexusShared {
    fn default() -> Self {
        Self {
            next_block_id: Arc::new(AtomicU64::new(1)),
        }
    }
}

// =========================================================================
// Application State (per-window)
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
    pub next_block_id: Arc<AtomicU64>,
    pub focus: Focus,
    pub kernel: Arc<Mutex<Kernel>>,
    pub kernel_tx: broadcast::Sender<nexus_api::ShellEvent>,

    // --- Layout ---
    pub zoom_level: f32,

    // --- UI state ---
    pub last_edit_time: Instant,
    pub exit_requested: bool,
    pub drop_highlight: Option<message::DropZone>,
    pub(crate) drag: crate::features::selection::drag::DragState,

    // --- Render tracking ---
    last_frame: Cell<Instant>,
    fps_smooth: Cell<f32>,
    /// Cached cursor blink state — on_tick only re-renders when it transitions.
    last_cursor_blink: bool,
    pub context: NexusContext,

    /// Debug mode for layout visualization (toggle with Cmd+Shift+D).
    #[cfg(debug_assertions)]
    pub debug_layout: bool,
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

        // The adapter sets the viewport to the stable base size (unzoomed
        // logical dimensions). Layout always sees the same cols/rows regardless
        // of zoom — the adapter handles window resizing and GPU scaling.
        let vp = snapshot.viewport();
        let vw = vp.width;
        let vh = vp.height;

        let (cols, rows) = NexusState::compute_terminal_size(vw, vh);
        self.shell.pty.terminal_size.set((cols, rows));
        self.shell.pty.sync_pty_sizes();

        let cursor_visible = self.cursor_visible();

        self.shell.clear_click_registry();

        let mut scroll = ScrollColumn::from_state(&self.scroll.state)
            .spacing(4.0)
            .width(Length::Fill)
            .height(Length::Fill);
        if self.scroll.target == crate::ui::scroll::ScrollTarget::Bottom {
            scroll = scroll.scroll_offset(f32::MAX);
        }
        let scroll = self.layout_blocks(scroll);

        // Window safe-area insets: keep content clear of macOS rounded window
        // corners.  These are physical pixels divided by zoom so they stay
        // constant on screen regardless of zoom level.
        // Only the scroll content gets side/top insets; the input bar is
        // edge-to-edge with only bottom padding below it.
        let z = self.zoom_level;
        let safe = Padding::new(2.0 / z, 4.0 / z, 0.0, 4.0 / z);

        let mut main_col = Column::new()
            .width(Length::Fixed(vw))
            .height(Length::Fixed(vh))
            .padding_custom(Padding::new(0.0, 0.0, 4.0 / z, 0.0));

        main_col = main_col.push(
            Column::new()
                .width(Length::Fill)
                .height(Length::Fill)
                .padding_custom(safe)
                .push(scroll),
        );

        main_col = self.layout_overlays_and_input(main_col, cursor_visible);

        // Use constraint-based layout for debug visualization support
        {
            use strata::layout::{LayoutContext, LayoutConstraints};
            use strata::primitives::Point;

            #[cfg(debug_assertions)]
            let mut ctx = LayoutContext::new(snapshot).with_debug(self.debug_layout);
            #[cfg(not(debug_assertions))]
            let mut ctx = LayoutContext::new(snapshot);

            let constraints = LayoutConstraints::tight(vw, vh);
            main_col.layout_with_constraints(&mut ctx, constraints, Point::ORIGIN);
        }

        self.sync_scroll_states(snapshot);

        // Scroll-to-block: compute content-space position and store as pending offset.
        // The actual scroll mutation happens in update() via apply_pending().
        if let crate::ui::scroll::ScrollTarget::Block(target_id) = self.scroll.target {
            let source = crate::utils::ids::block_container(target_id);
            if let Some(bounds) = snapshot.widget_bounds(&source) {
                let scroll_bounds = self.scroll.state.bounds.get();
                let content_y = bounds.y - scroll_bounds.y + self.scroll.state.offset;
                let viewport_h = scroll_bounds.height;

                let target_offset = if bounds.height > viewport_h {
                    // Tall block: snap top to maximize visible content
                    content_y
                } else {
                    // Short block: position at 1/3 down for context
                    (content_y - viewport_h / 3.0).max(0.0)
                };

                let max = self.scroll.state.max.get();
                self.scroll.pending_offset.set(Some(target_offset.min(max)));
            }
        }

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
            crate::ui::theme::TEXT_MUTED,
            14.0,
        );

        // Debug layout overlay (Cmd+Shift+D to toggle)
        #[cfg(debug_assertions)]
        if self.debug_layout && snapshot.has_debug_rects() {
            // Collect debug rect data first (to avoid borrow conflict)
            let debug_data: Vec<_> = snapshot.debug_rects()
                .iter()
                .map(|r| (r.rect, r.label.clone(), r.color()))
                .collect();
            let count = debug_data.len();

            let p = snapshot.overlay_primitives_mut();
            for (rect, label, color) in debug_data {
                // Draw semi-transparent colored outline
                p.add_border(rect, 0.0, 1.0, color);

                // Draw label at top-left of rect (if large enough)
                if rect.width > 40.0 && rect.height > 16.0 {
                    p.add_text(
                        label,
                        strata::primitives::Point::new(rect.x + 2.0, rect.y + 2.0),
                        color,
                        10.0,
                    );
                }
            }

            // Draw legend in top-left corner
            p.add_rounded_rect(
                Rect::new(4.0, 24.0, 180.0, 40.0),
                4.0,
                strata::primitives::Color::rgba(0.0, 0.0, 0.0, 0.8),
            );
            p.add_text(
                format!("Debug Layout: {} elements", count),
                strata::primitives::Point::new(8.0, 28.0),
                strata::primitives::Color::rgba(1.0, 1.0, 1.0, 0.9),
                12.0,
            );
            p.add_text(
                "Cmd+Shift+D to toggle".to_string(),
                strata::primitives::Point::new(8.0, 44.0),
                strata::primitives::Color::rgba(0.7, 0.7, 0.7, 0.9),
                10.0,
            );
        }
    }

    fn on_key(&self, event: KeyEvent) -> Option<NexusMessage> {
        routing::on_key(self, event)
    }

    fn on_mouse(
        &self,
        event: MouseEvent,
        hit: Option<HitResult>,
        capture: &CaptureState,
    ) -> MouseResponse<NexusMessage> {
        routing::on_mouse(self, event, hit, capture)
    }

    fn on_file_drop(
        &self,
        event: FileDropEvent,
        hit: Option<HitResult>,
    ) -> Option<NexusMessage> {
        use message::FileDropMsg;
        match event {
            FileDropEvent::Hovered(path) => {
                let zone = crate::features::selection::drop::resolve_drop_zone(self, &hit);
                Some(NexusMessage::FileDrop(FileDropMsg::Hovered(path, zone)))
            }
            FileDropEvent::Dropped(path) => {
                let zone = crate::features::selection::drop::resolve_drop_zone(self, &hit);
                Some(NexusMessage::FileDrop(FileDropMsg::Dropped(path, zone)))
            }
            FileDropEvent::HoverLeft => {
                Some(NexusMessage::FileDrop(FileDropMsg::HoverLeft))
            }
        }
    }

    fn subscription(&self) -> Subscription<NexusMessage> {
        let subs = vec![
            self.shell.subscription(),
            self.agent.subscription(),
        ];

        Subscription::batch(subs)
    }

    fn on_tick(&mut self) -> bool {
        let output_dirty = self.shell.needs_redraw() || self.agent.needs_redraw();
        let auto_scrolling = self.drag.auto_scroll.get().is_some();
        self.on_output_arrived();
        let spring_animating = self.scroll.tick_overscroll();

        // Cursor blink: only re-render on the 500ms transition, not every tick.
        let cursor_now = self.cursor_visible();
        let cursor_changed = cursor_now != self.last_cursor_blink;
        self.last_cursor_blink = cursor_now;

        output_dirty || spring_animating || auto_scrolling || cursor_changed
    }

    fn selection(&self) -> Option<&strata::Selection> {
        self.selection.selection.as_ref()
    }

    fn zoom_level(&self) -> f32 {
        self.zoom_level
    }

    fn force_click_lookup(
        &self,
        addr: &strata::content_address::ContentAddress,
    ) -> Option<(String, strata::content_address::ContentAddress, f32)> {
        use crate::features::selection::snap;
        let content = self.build_snap_content(addr.source_id)?;
        let (start, end) = snap::snap_word(addr, &content);
        let word = snap::extract_snap_text(&start, &end, &content);
        if word.trim().is_empty() {
            return None;
        }
        let font_size = 14.0 * self.zoom_level;
        Some((word, start, font_size))
    }
}

// =========================================================================
// Window title
// =========================================================================

impl NexusState {
    /// Compute the dynamic window title based on current app state.
    ///
    /// Priority chain:
    ///   1. Focused PTY block with OSC title → "Nexus — <osc_title>"
    ///   2. Focused PTY block without OSC    → "Nexus — <command>"
    ///   3. Agent active                     → "Nexus — <cwd> (<branch>) · Agent"
    ///   4. Running kernel command            → "Nexus — <cwd> (<branch>) · <command>"
    ///   5. Idle                              → "Nexus — <cwd> (<branch>)"
    fn compute_title(&self) -> String {
        let prefix = "Nexus";

        // If a PTY block is focused, delegate to it.
        if let Focus::Block(id) = self.focus {
            if let Some(block) = self.shell.block_by_id(id) {
                let has_pty = self.shell.pty.has_handle(id);
                if has_pty || block.osc_title.is_some() {
                    let context = block.osc_title.as_deref()
                        .unwrap_or(&block.command);
                    return format!("{} — {}", prefix, truncate_title(context, 80));
                }
            }
        }

        // Native context: CWD + git branch + activity.
        let path = shorten_path(&self.cwd);

        let branch = self.context.git.as_ref().map(|g| g.branch.as_str());

        let location = match branch {
            Some(b) if !b.is_empty() => format!("{} ({})", path, b),
            _ => path,
        };

        if self.agent.is_active() {
            format!("{} — {} · Agent", prefix, location)
        } else if let Some(cmd) = self.last_running_command() {
            format!("{} — {} · {}", prefix, location, truncate_title(&cmd, 40))
        } else {
            format!("{} — {}", prefix, location)
        }
    }

    /// Get the command string of the most recent running block (kernel or PTY).
    fn last_running_command(&self) -> Option<String> {
        self.shell.blocks.blocks.iter().rev()
            .find(|b| b.is_running())
            .map(|b| b.command.clone())
    }
}

/// Shorten a filesystem path for display (replace home dir with ~).
fn shorten_path(path: &str) -> String {
    crate::utils::text::display_path(path)
}

/// Truncate a title string to a maximum character width, adding ellipsis if needed.
fn truncate_title(s: &str, max: usize) -> String {
    crate::utils::text::truncate_str(s, max)
}

impl RootComponent for NexusState {
    type SharedState = NexusShared;

    fn create(shared: &NexusShared, _images: &mut ImageStore) -> (Self, Command<NexusMessage>) {
        // Each window gets its own Kernel — full CWD/variable/output isolation.
        // History is naturally shared (all kernels read the same shell history file).
        let (mut kernel, kernel_rx) = Kernel::new().expect("Failed to create kernel");
        let kernel_tx = kernel.event_sender().clone();

        let command_history: Vec<String> = kernel
            .get_recent_history(1000)
            .into_iter()
            .map(|e| e.command)
            .collect();

        // Each window starts in $HOME. The process-level CWD is not meaningful
        // in multi-window mode — each window tracks its own CWD independently.
        let home = std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default());
        let cwd = home.display().to_string();

        let context = NexusContext::new(home.clone());

        // Sync the kernel's internal CWD to match this window's starting dir.
        kernel.state_mut().set_cwd(home).ok();

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
            next_block_id: shared.next_block_id.clone(),
            focus: Focus::Input,
            kernel,
            kernel_tx,

            zoom_level: 0.85,

            last_edit_time: Instant::now(),
            exit_requested: false,
            drop_highlight: None,
            drag: crate::features::selection::drag::DragState::new(),
            last_frame: Cell::new(Instant::now()),
            fps_smooth: Cell::new(0.0),
            last_cursor_blink: true,
            context,
            #[cfg(debug_assertions)]
            debug_layout: false,
        };

        (state, Command::none())
    }

    fn create_window(shared: &NexusShared, images: &mut ImageStore) -> Option<(Self, Command<NexusMessage>)> {
        Some(Self::create(shared, images))
    }

    fn is_new_window_request(msg: &NexusMessage) -> bool {
        matches!(msg, NexusMessage::NewWindow)
    }

    fn is_exit_request(msg: &NexusMessage) -> bool {
        matches!(msg, NexusMessage::QuitApp)
    }

    fn title(&self) -> String {
        self.compute_title()
    }

    fn background_color(&self) -> strata::primitives::Color {
        crate::ui::theme::BG_APP
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
        background_color: crate::ui::theme::BG_APP,
    })
}
