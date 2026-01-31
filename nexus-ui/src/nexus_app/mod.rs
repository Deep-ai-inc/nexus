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

pub use message::NexusMessage;
use context_menu::{ContextMenuItem, ContextMenuState, ContextTarget, render_context_menu};
use input::{InputWidget, InputOutput};
use selection::SelectionWidget;
use shell::{ShellWidget, ShellOutput};
use agent::{AgentWidget, AgentOutput};

use std::cell::Cell;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, mpsc, Mutex};

use nexus_api::{BlockId, Value};
use nexus_kernel::Kernel;

use crate::agent_adapter::AgentEvent;
use crate::blocks::{Focus, PtyEvent, UnifiedBlockRef};
use crate::context::NexusContext;
use strata::content_address::SourceId;
use strata::event_context::{CaptureState, KeyEvent, MouseEvent};
use strata::layout_snapshot::HitResult;
use crate::nexus_widgets::{
    AgentBlockWidget, CompletionPopup, HistorySearchBar, JobBar, NexusInputBar,
    ShellBlockWidget, WelcomeScreen,
};
use strata::primitives::Rect;
use strata::{
    AppConfig, ButtonElement, Column, Command, CrossAxisAlignment, ImageElement, ImageHandle,
    ImageStore, LayoutSnapshot, Length, MouseResponse, Padding, Row, ScrollColumn,
    ScrollState, Selection, StrataApp, Subscription,
};
use crate::systems::{agent_subscription, kernel_subscription, pty_subscription};
use crate::shell_context::build_shell_context;

// =========================================================================
// Attachment (clipboard image paste)
// =========================================================================

pub struct Attachment {
    pub data: Vec<u8>,
    pub image_handle: ImageHandle,
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

    // --- Shared context ---
    pub cwd: String,
    pub next_block_id: u64,
    pub focus: Focus,
    pub kernel: Arc<Mutex<Kernel>>,
    pub kernel_tx: broadcast::Sender<nexus_api::ShellEvent>,
    pub kernel_rx: Arc<Mutex<broadcast::Receiver<nexus_api::ShellEvent>>>,
    pub pty_rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
    pub agent_event_rx: Arc<Mutex<mpsc::UnboundedReceiver<AgentEvent>>>,

    // --- Layout ---
    pub history_scroll: ScrollState,
    pub window_size: (f32, f32),

    // --- UI state ---
    pub last_edit_time: Instant,
    pub context_menu: Option<ContextMenuState>,
    pub exit_requested: bool,
    pub context: NexusContext,
}

impl NexusState {
    fn next_id(&mut self) -> BlockId {
        let id = BlockId(self.next_block_id);
        self.next_block_id += 1;
        id
    }

    fn unified_blocks(&self) -> Vec<UnifiedBlockRef<'_>> {
        let mut blocks: Vec<UnifiedBlockRef> =
            Vec::with_capacity(self.shell.blocks.len() + self.agent.blocks.len());
        for b in &self.shell.blocks {
            blocks.push(UnifiedBlockRef::Shell(b));
        }
        for b in &self.agent.blocks {
            blocks.push(UnifiedBlockRef::Agent(b));
        }
        blocks.sort_by_key(|b| match b {
            UnifiedBlockRef::Shell(b) => b.id.0,
            UnifiedBlockRef::Agent(b) => b.id.0,
        });
        blocks
    }
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

            cwd,
            next_block_id: 1,
            focus: Focus::Input,
            kernel: Arc::new(Mutex::new(kernel)),
            kernel_tx,
            kernel_rx: Arc::new(Mutex::new(kernel_rx)),
            pty_rx: Arc::new(Mutex::new(pty_rx)),
            agent_event_rx: Arc::new(Mutex::new(agent_event_rx)),

            history_scroll: ScrollState::new(),
            window_size: (1200.0, 800.0),

            last_edit_time: Instant::now(),
            context_menu: None,
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
        if matches!(
            &message,
            NexusMessage::InputKey(_) | NexusMessage::InputMouse(_)
        ) {
            state.last_edit_time = Instant::now();
        }

        match message {
            // === Input → InputWidget ===
            NexusMessage::InputKey(event) => {
                if let InputOutput::Submit {
                    text,
                    is_agent,
                    attachments,
                } = state.input.handle_key(&event)
                {
                    return handle_submit(state, text, is_agent, attachments);
                }
            }
            NexusMessage::InputMouse(action) => state.input.handle_mouse(action),
            NexusMessage::Submit(text) => {
                if let InputOutput::Submit {
                    text,
                    is_agent,
                    attachments,
                } = state.input.submit(text)
                {
                    return handle_submit(state, text, is_agent, attachments);
                }
            }
            NexusMessage::ToggleMode => state.input.toggle_mode(),
            NexusMessage::HistoryUp => state.input.history_up(),
            NexusMessage::HistoryDown => state.input.history_down(),
            NexusMessage::InsertNewline => state.input.insert_newline(),
            NexusMessage::RemoveAttachment(idx) => state.input.remove_attachment(idx),

            // === Completions → InputWidget (routes to CompletionWidget child) ===
            NexusMessage::TabComplete => {
                state.input.tab_complete(&state.kernel);
            }
            NexusMessage::CompletionNav(delta) => state.input.completion_nav(delta),
            NexusMessage::CompletionAccept => state.input.completion_accept(),
            NexusMessage::CompletionDismiss => state.input.completion_dismiss(),
            NexusMessage::CompletionSelect(index) => state.input.completion_select(index),
            NexusMessage::CompletionScroll(action) => state.input.completion.scroll(action),

            // === History search → InputWidget (routes to HistorySearchWidget child) ===
            NexusMessage::HistorySearchToggle => state.input.history_search_toggle(),
            NexusMessage::HistorySearchKey(key_event) => {
                state.input.history_search_key(key_event, &state.kernel);
            }
            NexusMessage::HistorySearchAccept => state.input.history_search_accept(),
            NexusMessage::HistorySearchDismiss => state.input.history_search_dismiss(),
            NexusMessage::HistorySearchSelect(index) => state.input.history_search_select(index),
            NexusMessage::HistorySearchAcceptIndex(index) => {
                state.input.history_search_accept_index(index);
            }
            NexusMessage::HistorySearchScroll(action) => state.input.history_search.scroll(action),

            // === Shell → ShellWidget ===
            NexusMessage::PtyOutput(id, data) => {
                let output = state.shell.handle_pty_output(id, data);
                process_shell_output(state, output);
            }
            NexusMessage::PtyExited(id, exit_code) => {
                let output = state.shell.handle_pty_exited(id, exit_code, &state.focus);
                process_shell_output(state, output);
            }
            NexusMessage::KernelEvent(evt) => {
                let output = state.shell.handle_kernel_event(evt, images);
                process_shell_output(state, output);
            }
            NexusMessage::SendInterrupt => state.shell.send_interrupt(&state.focus),
            NexusMessage::KillBlock(id) => state.shell.kill_block(id),
            NexusMessage::PtyInput(event) => {
                if let Focus::Block(block_id) = state.focus {
                    if !state.shell.forward_key(block_id, &event) {
                        state.focus = Focus::Input;
                        state.input.text_input.focused = true;
                    }
                }
            }
            NexusMessage::SortTable(block_id, col_idx) => {
                state.shell.sort_table(block_id, col_idx);
            }

            // === Agent → AgentWidget ===
            NexusMessage::AgentEvent(evt) => {
                state.agent.dirty = true;
                let output = state.agent.handle_event(evt);
                process_agent_output(state, output);
            }
            NexusMessage::ToggleThinking(id) => state.agent.toggle_thinking(id),
            NexusMessage::ToggleTool(id, idx) => state.agent.toggle_tool(id, idx),
            NexusMessage::PermissionGrant(block_id, perm_id) => {
                state.agent.permission_grant(block_id, perm_id);
            }
            NexusMessage::PermissionGrantSession(block_id, perm_id) => {
                state.agent.permission_grant_session(block_id, perm_id);
            }
            NexusMessage::PermissionDeny(block_id, perm_id) => {
                state.agent.permission_deny(block_id, perm_id);
            }
            NexusMessage::AgentInterrupt => state.agent.interrupt(),

            // === Selection → SelectionWidget ===
            NexusMessage::SelectionStart(addr) => state.selection.start(addr),
            NexusMessage::SelectionExtend(addr) => state.selection.extend(addr),
            NexusMessage::SelectionEnd => state.selection.end(),
            NexusMessage::ClearSelection => state.selection.clear(),
            NexusMessage::Copy => handle_copy(state),

            // === Scroll ===
            NexusMessage::HistoryScroll(action) => state.history_scroll.apply(action),
            NexusMessage::ScrollToJob(_) => {
                state.history_scroll.offset = state.history_scroll.max.get();
            }

            // === Context menu ===
            NexusMessage::ShowContextMenu(x, y, items, target) => {
                state.context_menu = Some(ContextMenuState {
                    x,
                    y,
                    items,
                    target,
                    hovered_item: Cell::new(None),
                });
            }
            NexusMessage::ContextMenuAction(item) => {
                return handle_context_menu_action(state, item);
            }
            NexusMessage::DismissContextMenu => state.context_menu = None,

            // === Clipboard ===
            NexusMessage::Paste => handle_paste(state, images),

            // === Window ===
            NexusMessage::ClearScreen => {
                state.shell.clear();
                state.agent.clear();
                state.history_scroll.offset = 0.0;
                state.focus = Focus::Input;
                state.input.text_input.focused = true;
            }
            NexusMessage::CloseWindow => state.exit_requested = true,
            NexusMessage::BlurAll => {
                state.focus = Focus::Input;
                state.input.text_input.focused = true;
            }
            NexusMessage::Tick => {
                if state.shell.terminal_dirty || state.agent.dirty {
                    state.shell.terminal_dirty = false;
                    state.agent.dirty = false;
                    state.history_scroll.offset = state.history_scroll.max.get();
                }
            }
        }

        state.shell.sync_terminal_size();
        Command::none()
    }

    fn view(state: &Self::State, snapshot: &mut LayoutSnapshot) {
        let vp = snapshot.viewport();
        let vw = vp.width;
        let vh = vp.height;

        // Recalculate terminal size from viewport
        let char_width = 14.0 * 0.607;
        let line_height = 14.0 * 1.4;
        let h_padding = 4.0 + 6.0 * 2.0;
        let v_padding = 44.0;
        let cols = ((vw - h_padding) / char_width) as u16;
        let rows = ((vh - v_padding) / line_height) as u16;
        state
            .shell
            .terminal_size
            .set((cols.max(40).min(500), rows.max(5).min(200)));

        let now = Instant::now();
        let blink_elapsed = now.duration_since(state.last_edit_time).as_millis();
        let cursor_visible = (blink_elapsed / 500) % 2 == 0;

        let unified = state.unified_blocks();
        let has_blocks = !unified.is_empty();

        let mut scroll = ScrollColumn::from_state(&state.history_scroll)
            .spacing(4.0)
            .width(Length::Fill)
            .height(Length::Fill);

        if !has_blocks {
            scroll = scroll.push(WelcomeScreen { cwd: &state.cwd });
        } else {
            for block_ref in &unified {
                match block_ref {
                    UnifiedBlockRef::Shell(block) => {
                        let kill_id = source_ids::kill(block.id);
                        let image_info = state.shell.image_handles.get(&block.id).copied();
                        let is_focused =
                            matches!(state.focus, Focus::Block(id) if id == block.id);
                        scroll = scroll.push(ShellBlockWidget {
                            block,
                            kill_id,
                            image_info,
                            is_focused,
                        });
                    }
                    UnifiedBlockRef::Agent(block) => {
                        let thinking_id = source_ids::agent_thinking_toggle(block.id);
                        let stop_id = source_ids::agent_stop(block.id);
                        scroll = scroll.push(AgentBlockWidget {
                            block,
                            thinking_toggle_id: thinking_id,
                            stop_id,
                        });
                    }
                }
            }
        }

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

        if !state.shell.jobs.is_empty() {
            main_col = main_col.push(JobBar {
                jobs: &state.shell.jobs,
            });
        }

        if state.input.completion.is_active() {
            main_col = main_col.push(CompletionPopup {
                completions: &state.input.completion.completions,
                selected_index: state.input.completion.index,
                hovered_index: state.input.completion.hovered.get(),
                scroll: &state.input.completion.scroll,
            });
        }

        if state.input.history_search.is_active() {
            main_col = main_col.push(HistorySearchBar {
                query: &state.input.history_search.query,
                results: &state.input.history_search.results,
                result_index: state.input.history_search.index,
                hovered_index: state.input.history_search.hovered.get(),
                scroll: &state.input.history_search.scroll,
            });
        }

        if !state.input.attachments.is_empty() {
            let mut attach_row = Row::new().spacing(8.0).padding(4.0);
            for (i, attachment) in state.input.attachments.iter().enumerate() {
                let scale = (60.0_f32 / attachment.width as f32)
                    .min(60.0 / attachment.height as f32)
                    .min(1.0);
                let w = attachment.width as f32 * scale;
                let h = attachment.height as f32 * scale;
                let remove_id = SourceId::named(&format!("remove_attach_{}", i));
                attach_row = attach_row.push(
                    Column::new()
                        .spacing(2.0)
                        .cross_align(CrossAxisAlignment::Center)
                        .image(
                            ImageElement::new(attachment.image_handle, w, h).corner_radius(4.0),
                        )
                        .push(
                            ButtonElement::new(remove_id, "\u{2715}")
                                .background(colors::BTN_DENY)
                                .corner_radius(4.0),
                        ),
                );
            }
            main_col = main_col.push(
                Column::new()
                    .padding_custom(Padding::new(2.0, 4.0, 0.0, 4.0))
                    .width(Length::Fill)
                    .push(attach_row),
            );
        }

        main_col = main_col.push(
            Column::new()
                .padding_custom(Padding::new(2.0, 4.0, 4.0, 4.0))
                .width(Length::Fill)
                .push(NexusInputBar {
                    input: &state.input.text_input,
                    mode: state.input.mode,
                    cwd: &state.cwd,
                    last_exit_code: state.shell.last_exit_code,
                    cursor_visible,
                    mode_toggle_id: SourceId::named("mode_toggle"),
                    line_count: {
                        let count = state.input.text_input.text.lines().count()
                            + if state.input.text_input.text.ends_with('\n') {
                                1
                            } else {
                                0
                            };
                        count.max(1).min(6)
                    },
                }),
        );

        main_col.layout(snapshot, Rect::new(0.0, 0.0, vw, vh));

        state.history_scroll.sync_from_snapshot(snapshot);
        state
            .input
            .completion
            .scroll
            .sync_from_snapshot(snapshot);
        state
            .input
            .history_search
            .scroll
            .sync_from_snapshot(snapshot);
        state.input.text_input.sync_from_snapshot(snapshot);

        if let Some(ref menu) = state.context_menu {
            render_context_menu(snapshot, menu);
        }
    }

    fn selection(state: &Self::State) -> Option<&Selection> {
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
// Output processors
// =========================================================================

fn process_shell_output(state: &mut NexusState, output: ShellOutput) {
    match output {
        ShellOutput::None => {}
        ShellOutput::FocusInput => {
            state.focus = Focus::Input;
            state.input.text_input.focused = true;
            state.history_scroll.offset = state.history_scroll.max.get();
        }
        ShellOutput::FocusBlock(id) => {
            state.focus = Focus::Block(id);
            state.input.text_input.focused = false;
            state.history_scroll.offset = state.history_scroll.max.get();
        }
        ShellOutput::ScrollToBottom => {
            state.history_scroll.offset = state.history_scroll.max.get();
        }
        ShellOutput::CwdChanged(path) => {
            state.cwd = path.display().to_string();
            let _ = std::env::set_current_dir(&path);
        }
        ShellOutput::CommandFinished {
            exit_code,
            command,
            output,
        } => {
            state.context.on_command_finished(command, output, exit_code);
            state.focus = Focus::Input;
            state.input.text_input.focused = true;
            state.history_scroll.offset = state.history_scroll.max.get();
        }
    }
}

fn process_agent_output(state: &mut NexusState, output: AgentOutput) {
    match output {
        AgentOutput::None => {}
        AgentOutput::ScrollToBottom => {
            state.history_scroll.offset = state.history_scroll.max.get();
        }
    }
}

// =========================================================================
// Command handlers
// =========================================================================

fn handle_submit(
    state: &mut NexusState,
    text: String,
    is_agent: bool,
    attachments: Vec<Value>,
) -> Command<NexusMessage> {
    state.input.reset_history_nav();

    if is_agent {
        let block_id = state.next_id();
        let contextualized_query = if state.agent.session_id.is_some() {
            format!("[CWD: {}]\n{}", state.cwd, text)
        } else {
            let shell_context = build_shell_context(
                &state.cwd,
                &state.shell.blocks,
                state.input.shell_history(),
            );
            format!("{}{}", shell_context, text)
        };
        state
            .agent
            .spawn(block_id, text, contextualized_query, attachments, &state.cwd);
        state.history_scroll.offset = state.history_scroll.max.get();
    } else {
        // Handle built-in: clear
        if text.trim() == "clear" {
            state.shell.clear();
            state.agent.clear();
            return Command::none();
        }

        let block_id = state.next_id();
        let output =
            state
                .shell
                .execute(text, block_id, &state.cwd, &state.kernel, &state.kernel_tx);
        process_shell_output(state, output);
    }

    Command::none()
}

fn handle_copy(state: &mut NexusState) {
    let mut copied = false;

    if let Some(text) =
        state
            .selection
            .extract_selected_text(&state.shell.blocks, &state.agent.blocks)
    {
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(&text);
            copied = true;
        }
    }

    if !copied {
        if let Some((sel_start, sel_end)) = state.input.text_input.selection {
            let start = sel_start.min(sel_end);
            let end = sel_start.max(sel_end);
            if start != end {
                let selected: String = state
                    .input
                    .text_input
                    .text
                    .chars()
                    .skip(start)
                    .take(end - start)
                    .collect();
                if !selected.is_empty() {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        let _ = clipboard.set_text(&selected);
                    }
                }
            }
        }
    }
}

fn handle_paste(state: &mut NexusState, images: &mut ImageStore) {
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        if let Ok(img) = clipboard.get_image() {
            let width = img.width as u32;
            let height = img.height as u32;
            let rgba_data = img.bytes.into_owned();

            let mut png_data = Vec::new();
            if let Some(img_buf) = image::RgbaImage::from_raw(width, height, rgba_data.clone()) {
                let _ = img_buf.write_to(
                    &mut std::io::Cursor::new(&mut png_data),
                    image::ImageFormat::Png,
                );
            }

            if !png_data.is_empty() {
                let handle = images.load_rgba(width, height, rgba_data);
                state.input.add_attachment(Attachment {
                    data: png_data,
                    image_handle: handle,
                    width,
                    height,
                });
            }
        } else if let Ok(text) = clipboard.get_text() {
            if !text.is_empty() {
                state.input.paste_text(&text);
            }
        }
    }
}

fn handle_context_menu_action(
    state: &mut NexusState,
    item: ContextMenuItem,
) -> Command<NexusMessage> {
    let target = state.context_menu.as_ref().map(|m| m.target.clone());
    state.context_menu = None;
    match item {
        ContextMenuItem::Copy => {
            if let Some(text) = target.and_then(|t| {
                selection::extract_block_text(
                    &state.shell.blocks,
                    &state.shell.block_index,
                    &state.agent.blocks,
                    &state.agent.block_index,
                    &state.input.text_input.text,
                    &t,
                )
            }) {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    let _ = clipboard.set_text(&text);
                }
            }
        }
        ContextMenuItem::Paste => {
            return Command::message(NexusMessage::Paste);
        }
        ContextMenuItem::SelectAll => match target.as_ref() {
            Some(ContextTarget::Input) | None => {
                state.input.text_input.select_all();
            }
            Some(ContextTarget::Block(_)) | Some(ContextTarget::AgentBlock(_)) => {
                state
                    .selection
                    .select_all(&state.shell.blocks, &state.agent.blocks);
            }
        },
        ContextMenuItem::Clear => {
            state.input.text_input.text.clear();
            state.input.text_input.cursor = 0;
            state.input.text_input.selection = None;
        }
    }
    Command::none()
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
