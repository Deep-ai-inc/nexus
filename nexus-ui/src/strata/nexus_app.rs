//! Nexus Strata Application
//!
//! The Strata-based Nexus UI, built on the GPU-accelerated layout system.
//! This is the replacement for the legacy Iced widget-based UI.
//!
//! Run with: `cargo run -p nexus-ui -- --strata`

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::{broadcast, mpsc, Mutex};

use nexus_api::{BlockId, BlockState, ShellEvent, Value};
use nexus_kernel::{CommandClassification, CommandRegistry, Completion, Kernel};
use nexus_term::TerminalParser;

use crate::agent_adapter::{AgentEvent, PermissionResponse};
use crate::agent_block::{AgentBlock, AgentBlockState, PermissionRequest};
use crate::blocks::{Block, Focus, InputMode, PtyEvent, UnifiedBlockRef};
use crate::context::NexusContext;
use crate::pty::PtyHandle;
use crate::route_mouse;
use crate::shell_context::build_shell_context;
use crate::strata::content_address::{ContentAddress, SourceId};
use crate::strata::event_context::{
    CaptureState, Key, KeyEvent, MouseButton, MouseEvent, NamedKey,
};
use crate::strata::layout_snapshot::HitResult;
use crate::strata::nexus_widgets::{
    AgentBlockWidget, CompletionPopup, HistorySearchBar, JobBar, NexusInputBar,
    ShellBlockWidget, WelcomeScreen,
};
use crate::strata::primitives::Rect;
use crate::strata::{
    AppConfig, ButtonElement, Column, Command, CrossAxisAlignment, ImageElement, ImageHandle,
    ImageStore, LayoutSnapshot, Length, MouseResponse, Padding, Row, ScrollAction, ScrollColumn,
    ScrollState, Selection, StrataApp, Subscription, TextInputAction, TextInputMouseAction,
    TextInputState,
};
use crate::systems::{agent_subscription, kernel_subscription, pty_subscription, spawn_agent_task};
use crate::widgets::job_indicator::{VisualJob, VisualJobState};

// =========================================================================
// Source ID helpers — single source of truth for all source ID strings
// =========================================================================

/// Source IDs for shell and agent blocks.
pub(crate) mod source_ids {
    use super::*;

    pub fn shell_header(id: BlockId) -> SourceId { SourceId::named(&format!("shell_header_{}", id.0)) }
    pub fn shell_term(id: BlockId) -> SourceId { SourceId::named(&format!("shell_term_{}", id.0)) }
    pub fn native(id: BlockId) -> SourceId { SourceId::named(&format!("native_{}", id.0)) }
    pub fn table(id: BlockId) -> SourceId { SourceId::named(&format!("table_{}", id.0)) }
    pub fn table_sort(id: BlockId, col: usize) -> SourceId { SourceId::named(&format!("sort_{}_{}", id.0, col)) }
    pub fn kill(id: BlockId) -> SourceId { SourceId::named(&format!("kill_{}", id.0)) }

    pub fn agent_query(id: BlockId) -> SourceId { SourceId::named(&format!("agent_query_{}", id.0)) }
    pub fn agent_thinking(id: BlockId) -> SourceId { SourceId::named(&format!("agent_thinking_{}", id.0)) }
    pub fn agent_response(id: BlockId) -> SourceId { SourceId::named(&format!("agent_response_{}", id.0)) }
    pub fn agent_thinking_toggle(id: BlockId) -> SourceId { SourceId::named(&format!("thinking_{}", id.0)) }
    pub fn agent_stop(id: BlockId) -> SourceId { SourceId::named(&format!("stop_{}", id.0)) }
    pub fn agent_tool_toggle(id: BlockId, i: usize) -> SourceId { SourceId::named(&format!("tool_toggle_{}_{}", id.0, i)) }
    pub fn agent_perm_deny(id: BlockId) -> SourceId { SourceId::named(&format!("perm_deny_{}", id.0)) }
    pub fn agent_perm_allow(id: BlockId) -> SourceId { SourceId::named(&format!("perm_allow_{}", id.0)) }
    pub fn agent_perm_always(id: BlockId) -> SourceId { SourceId::named(&format!("perm_always_{}", id.0)) }
}

// =========================================================================
// Color palette (matches real Nexus app)
// =========================================================================
pub(crate) mod colors {
    use crate::strata::primitives::Color;

    // Backgrounds (matched from theme.rs BG_PRIMARY/SECONDARY/TERTIARY + view/mod.rs)
    pub const BG_APP: Color = Color { r: 0.07, g: 0.07, b: 0.09, a: 1.0 };
    pub const BG_BLOCK: Color = Color { r: 0.12, g: 0.12, b: 0.14, a: 1.0 };
    pub const BG_INPUT: Color = Color { r: 0.1, g: 0.1, b: 0.12, a: 1.0 };

    // Status (matched from theme.rs)
    pub const SUCCESS: Color = Color { r: 0.3, g: 0.8, b: 0.5, a: 1.0 };
    pub const ERROR: Color = Color { r: 0.9, g: 0.3, b: 0.3, a: 1.0 };
    pub const WARNING: Color = Color { r: 0.9, g: 0.7, b: 0.2, a: 1.0 };
    pub const RUNNING: Color = Color { r: 0.3, g: 0.7, b: 1.0, a: 1.0 };
    pub const THINKING: Color = Color { r: 0.6, g: 0.6, b: 0.7, a: 1.0 };

    // Text (matched from theme.rs FG_PRIMARY/SECONDARY/MUTED + input.rs)
    pub const TEXT_PRIMARY: Color = Color { r: 0.9, g: 0.9, b: 0.9, a: 1.0 };
    pub const TEXT_SECONDARY: Color = Color { r: 0.6, g: 0.6, b: 0.6, a: 1.0 };
    pub const TEXT_MUTED: Color = Color { r: 0.5, g: 0.5, b: 0.5, a: 1.0 };
    // rgb8(100, 149, 237) = cornflower blue
    pub const TEXT_PATH: Color = Color { r: 0.392, g: 0.584, b: 0.929, a: 1.0 };
    pub const TEXT_PURPLE: Color = Color { r: 0.6, g: 0.5, b: 0.9, a: 1.0 };
    pub const TEXT_QUERY: Color = Color { r: 0.5, g: 0.7, b: 1.0, a: 1.0 };

    // Tool colors (matched from agent_widgets.rs)
    pub const TOOL_PENDING: Color = Color { r: 0.6, g: 0.6, b: 0.3, a: 1.0 };
    pub const TOOL_OUTPUT: Color = Color { r: 0.8, g: 0.8, b: 0.8, a: 1.0 };

    // Code blocks (matched from agent_widgets.rs)
    pub const CODE_BG: Color = Color { r: 0.06, g: 0.06, b: 0.08, a: 1.0 };
    pub const CODE_TEXT: Color = Color { r: 0.9, g: 0.9, b: 0.9, a: 1.0 };

    // Buttons (matched from agent_widgets.rs + shell_view.rs)
    pub const BTN_DENY: Color = Color { r: 0.6, g: 0.15, b: 0.15, a: 1.0 };
    pub const BTN_ALLOW: Color = Color { r: 0.15, g: 0.5, b: 0.25, a: 1.0 };
    pub const BTN_ALWAYS: Color = Color { r: 0.1, g: 0.35, b: 0.18, a: 1.0 };
    pub const BTN_KILL: Color = Color { r: 0.7, g: 0.2, b: 0.2, a: 1.0 };

    // Borders (matched from theme.rs BORDER_DEFAULT + view/mod.rs)
    pub const BORDER_INPUT: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.08 };

    // Welcome screen (matched from app/view/welcome.rs)
    pub const WELCOME_TITLE: Color = Color { r: 0.6, g: 0.8, b: 0.6, a: 1.0 };
    pub const WELCOME_HEADING: Color = Color { r: 0.8, g: 0.7, b: 0.5, a: 1.0 };
    pub const CARD_BG: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.03 };
    pub const CARD_BORDER: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.06 };

}

// =========================================================================
// Message type
// =========================================================================

#[derive(Debug, Clone)]
pub enum NexusMessage {
    // Input
    InputKey(KeyEvent),
    InputMouse(TextInputMouseAction),
    Submit(String),
    ToggleMode,
    HistoryUp,
    HistoryDown,

    // Terminal / PTY
    PtyOutput(BlockId, Vec<u8>),
    PtyExited(BlockId, i32),
    PtyInput(KeyEvent),
    SendInterrupt,
    KernelEvent(ShellEvent),
    KillBlock(BlockId),

    // Agent
    AgentEvent(AgentEvent),
    ToggleThinking(BlockId),
    ToggleTool(BlockId, usize),
    PermissionGrant(BlockId, String),
    PermissionGrantSession(BlockId, String),
    PermissionDeny(BlockId, String),
    AgentInterrupt,

    // Scroll
    HistoryScroll(ScrollAction),
    CompletionScroll(ScrollAction),
    HistorySearchScroll(ScrollAction),

    // Selection
    SelectionStart(ContentAddress),
    SelectionExtend(ContentAddress),
    SelectionEnd,
    ClearSelection,
    Copy,

    // Table sorting
    SortTable(BlockId, usize),

    // Completions
    TabComplete,
    CompletionNav(isize),
    CompletionAccept,
    CompletionDismiss,

    // History search (Ctrl+R)
    HistorySearchToggle,
    HistorySearchKey(KeyEvent),
    HistorySearchAccept,
    HistorySearchDismiss,
    HistorySearchSelect(usize),
    HistorySearchAcceptIndex(usize),

    // Completion click
    CompletionSelect(usize),

    // Job indicator
    ScrollToJob(u32),

    // Clipboard
    Paste,
    RemoveAttachment(usize),

    // Context menu
    ShowContextMenu(f32, f32, Vec<ContextMenuItem>, ContextTarget),
    ContextMenuAction(ContextMenuItem),
    DismissContextMenu,

    // Multiline input
    InsertNewline,

    // Window
    ClearScreen,
    CloseWindow,
    BlurAll,
    Tick,
}

// =========================================================================
// Context menu
// =========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuItem {
    Copy,
    Paste,
    SelectAll,
    Clear,
}

impl ContextMenuItem {
    fn label(&self) -> &'static str {
        match self {
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::SelectAll => "Select All",
            Self::Clear => "Clear",
        }
    }

    fn shortcut(&self) -> &'static str {
        match self {
            Self::Copy => "\u{2318}C",
            Self::Paste => "\u{2318}V",
            Self::SelectAll => "\u{2318}A",
            Self::Clear => "",
        }
    }
}

#[derive(Debug, Clone)]
pub enum ContextTarget {
    Block(BlockId),
    AgentBlock(BlockId),
    Input,
}

pub struct ContextMenuState {
    pub x: f32,
    pub y: f32,
    pub items: Vec<ContextMenuItem>,
    pub target: ContextTarget,
    pub hovered_item: Cell<Option<usize>>,
}

// =========================================================================
// Attachment (clipboard image paste)
// =========================================================================

pub struct Attachment {
    pub data: Vec<u8>,       // PNG bytes
    pub image_handle: ImageHandle,
    pub width: u32,
    pub height: u32,
}

// =========================================================================
// Application State
// =========================================================================

pub struct NexusState {
    // --- Input ---
    pub input: TextInputState,
    pub mode: InputMode,
    pub shell_history: Vec<String>,
    pub shell_history_index: Option<usize>,
    pub agent_history: Vec<String>,
    pub agent_history_index: Option<usize>,
    pub saved_input: String,

    // --- Terminal ---
    pub blocks: Vec<Block>,
    pub block_index: HashMap<BlockId, usize>,
    pub next_block_id: u64,
    pub cwd: String,
    pub pty_handles: Vec<PtyHandle>,
    pub pty_tx: mpsc::UnboundedSender<(BlockId, PtyEvent)>,
    pub pty_rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
    pub focus: Focus,
    pub terminal_size: Cell<(u16, u16)>,
    pub last_exit_code: Option<i32>,
    pub commands: CommandRegistry,
    pub kernel: Arc<Mutex<Kernel>>,
    pub kernel_tx: broadcast::Sender<ShellEvent>,
    pub kernel_rx: Arc<Mutex<broadcast::Receiver<ShellEvent>>>,
    pub jobs: Vec<VisualJob>,
    pub terminal_dirty: bool,

    // --- Agent ---
    pub agent_blocks: Vec<AgentBlock>,
    pub agent_block_index: HashMap<BlockId, usize>,
    pub active_agent: Option<BlockId>,
    pub agent_event_tx: mpsc::UnboundedSender<AgentEvent>,
    pub agent_event_rx: Arc<Mutex<mpsc::UnboundedReceiver<AgentEvent>>>,
    pub agent_permission_tx: Option<mpsc::UnboundedSender<(String, PermissionResponse)>>,
    pub agent_cancel_flag: Arc<AtomicBool>,
    pub agent_dirty: bool,
    pub agent_session_id: Option<String>,

    // --- Layout ---
    pub history_scroll: ScrollState,
    pub window_size: (f32, f32),

    // --- Selection ---
    pub selection: Option<Selection>,
    pub is_selecting: bool,

    // --- Cursor blink ---
    pub last_edit_time: Instant,

    // --- Completions ---
    pub completions: Vec<Completion>,
    pub completion_index: Option<usize>,
    pub completion_anchor: usize,
    pub completion_scroll: ScrollState,
    pub hovered_completion: Cell<Option<usize>>,

    // --- History search ---
    pub history_search_active: bool,
    pub history_search_query: String,
    pub history_search_results: Vec<String>,
    pub history_search_index: usize,
    pub history_search_scroll: ScrollState,
    pub hovered_history_result: Cell<Option<usize>>,

    // --- Context menu ---
    pub context_menu: Option<ContextMenuState>,

    // --- Attachments ---
    pub attachments: Vec<Attachment>,

    // --- Terminal resize ---
    pub last_terminal_size: (u16, u16),

    // --- Images ---
    pub image_handles: HashMap<BlockId, (ImageHandle, u32, u32)>,

    // --- Window ---
    pub exit_requested: bool,

    // --- Context ---
    pub context: NexusContext,
}

impl NexusState {
    fn next_id(&mut self) -> BlockId {
        let id = BlockId(self.next_block_id);
        self.next_block_id += 1;
        id
    }

    fn current_history(&self) -> &[String] {
        match self.mode {
            InputMode::Shell => &self.shell_history,
            InputMode::Agent => &self.agent_history,
        }
    }

    fn current_history_index(&self) -> Option<usize> {
        match self.mode {
            InputMode::Shell => self.shell_history_index,
            InputMode::Agent => self.agent_history_index,
        }
    }

    fn set_history_index(&mut self, idx: Option<usize>) {
        match self.mode {
            InputMode::Shell => self.shell_history_index = idx,
            InputMode::Agent => self.agent_history_index = idx,
        }
    }

    fn push_history(&mut self, text: &str) {
        let history = match self.mode {
            InputMode::Shell => &mut self.shell_history,
            InputMode::Agent => &mut self.agent_history,
        };
        if history.last().map(|s| s.as_str()) != Some(text) {
            history.push(text.to_string());
            if history.len() > 1000 {
                history.remove(0);
            }
        }
    }

    /// Build a sorted list of unified block references for view rendering.
    fn unified_blocks(&self) -> Vec<UnifiedBlockRef<'_>> {
        let mut blocks: Vec<UnifiedBlockRef> = Vec::with_capacity(
            self.blocks.len() + self.agent_blocks.len()
        );
        for b in &self.blocks {
            blocks.push(UnifiedBlockRef::Shell(b));
        }
        for b in &self.agent_blocks {
            blocks.push(UnifiedBlockRef::Agent(b));
        }
        blocks.sort_by_key(|b| match b {
            UnifiedBlockRef::Shell(b) => b.id.0,
            UnifiedBlockRef::Agent(b) => b.id.0,
        });
        blocks
    }

    fn is_dirty(&self) -> bool {
        self.terminal_dirty || self.agent_dirty
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
        // Create kernel
        let (kernel, kernel_rx) = Kernel::new().expect("Failed to create kernel");
        let kernel_tx = kernel.event_sender().clone();

        // Load command history
        let command_history: Vec<String> = kernel
            .store()
            .and_then(|store| store.get_recent_history(1000).ok())
            .map(|entries| entries.into_iter().rev().map(|e| e.command).collect())
            .unwrap_or_default();

        // PTY channels
        let (pty_tx, pty_rx) = mpsc::unbounded_channel();

        // Agent channels
        let (agent_event_tx, agent_event_rx) = mpsc::unbounded_channel();

        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~".to_string());

        let context = NexusContext::new(std::env::current_dir().unwrap_or_default());

        let mut input = TextInputState::single_line("nexus_input");
        input.focused = true;

        let state = NexusState {
            input,
            mode: InputMode::Shell,
            shell_history: command_history,
            shell_history_index: None,
            agent_history: Vec::new(),
            agent_history_index: None,
            saved_input: String::new(),

            blocks: Vec::new(),
            block_index: HashMap::new(),
            next_block_id: 1,
            cwd,
            pty_handles: Vec::new(),
            pty_tx,
            pty_rx: Arc::new(Mutex::new(pty_rx)),
            focus: Focus::Input,
            terminal_size: Cell::new((120, 24)),
            last_exit_code: None,
            commands: CommandRegistry::new(),
            kernel: Arc::new(Mutex::new(kernel)),
            kernel_tx,
            kernel_rx: Arc::new(Mutex::new(kernel_rx)),
            jobs: Vec::new(),
            terminal_dirty: false,

            agent_blocks: Vec::new(),
            agent_block_index: HashMap::new(),
            active_agent: None,
            agent_event_tx,
            agent_event_rx: Arc::new(Mutex::new(agent_event_rx)),
            agent_permission_tx: None,
            agent_cancel_flag: Arc::new(AtomicBool::new(false)),
            agent_dirty: false,
            agent_session_id: None,

            history_scroll: ScrollState::new(),
            window_size: (1200.0, 800.0),

            selection: None,
            is_selecting: false,

            last_edit_time: Instant::now(),

            completions: Vec::new(),
            completion_index: None,
            completion_anchor: 0,
            completion_scroll: ScrollState::new(),
            hovered_completion: Cell::new(None),

            history_search_active: false,
            history_search_query: String::new(),
            history_search_results: Vec::new(),
            history_search_index: 0,
            history_search_scroll: ScrollState::new(),
            hovered_history_result: Cell::new(None),

            context_menu: None,
            attachments: Vec::new(),
            last_terminal_size: (120, 24),

            image_handles: HashMap::new(),
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
        // Reset cursor blink on input activity
        if matches!(&message, NexusMessage::InputKey(_) | NexusMessage::InputMouse(_)) {
            state.last_edit_time = Instant::now();
        }

        match message {
            // =============================================================
            // Input
            // =============================================================
            NexusMessage::InputKey(event) => {
                match state.input.handle_key(&event, false) {
                    TextInputAction::Submit(text) => {
                        return Command::message(NexusMessage::Submit(text));
                    }
                    _ => {}
                }
            }
            NexusMessage::InputMouse(action) => {
                state.input.focused = true;
                state.input.apply_mouse(action);
            }
            NexusMessage::Submit(submitted_text) => {
                let text = submitted_text.trim().to_string();
                if text.is_empty() {
                    return Command::none();
                }

                // Check for "? " prefix → agent mode one-shot
                let is_agent_query = state.mode == InputMode::Agent
                    || text.starts_with("? ");

                let query = if text.starts_with("? ") {
                    text[2..].to_string()
                } else {
                    text.clone()
                };

                // Record history
                state.push_history(&text);

                // Input already cleared by handle_key's Submit

                if is_agent_query {
                    // Collect attachments as Value::Media
                    let attachments: Vec<Value> = state.attachments.drain(..).map(|a| {
                        Value::Media {
                            data: a.data,
                            content_type: "image/png".to_string(),
                            metadata: Default::default(),
                        }
                    }).collect();
                    return spawn_agent(state, query, attachments);
                } else {
                    state.attachments.clear();
                    return execute_command(state, text);
                }
            }
            NexusMessage::ToggleMode => {
                state.mode = match state.mode {
                    InputMode::Shell => InputMode::Agent,
                    InputMode::Agent => InputMode::Shell,
                };
            }
            NexusMessage::HistoryUp => {
                let history = state.current_history();
                let history_len = history.len();
                if history_len == 0 {
                    return Command::none();
                }
                let idx = state.current_history_index();
                let new_index = match idx {
                    None => {
                        state.saved_input = state.input.text.clone();
                        Some(history_len - 1)
                    }
                    Some(0) => Some(0),
                    Some(i) => Some(i - 1),
                };
                state.set_history_index(new_index);
                if let Some(i) = new_index {
                    let text = state.current_history()[i].clone();
                    state.input.text = text;
                    state.input.cursor = state.input.text.len();
                }
            }
            NexusMessage::HistoryDown => {
                let history_len = state.current_history().len();
                let idx = state.current_history_index();
                match idx {
                    None => {}
                    Some(i) if i >= history_len - 1 => {
                        state.set_history_index(None);
                        state.input.text = state.saved_input.clone();
                        state.input.cursor = state.input.text.len();
                        state.saved_input.clear();
                    }
                    Some(i) => {
                        state.set_history_index(Some(i + 1));
                        let text = state.current_history()[i + 1].clone();
                        state.input.text = text;
                        state.input.cursor = state.input.text.len();
                    }
                }
            }

            // =============================================================
            // Terminal / PTY
            // =============================================================
            NexusMessage::PtyOutput(id, data) => {
                if let Some(&idx) = state.block_index.get(&id) {
                    if let Some(block) = state.blocks.get_mut(idx) {
                        block.parser.feed(&data);
                        block.version += 1;
                    }
                }
                if data.len() < 128 {
                    state.terminal_dirty = false;
                    // Auto-scroll
                    state.history_scroll.offset = state.history_scroll.max.get();
                } else {
                    state.terminal_dirty = true;
                }
            }
            NexusMessage::PtyExited(id, exit_code) => {
                if let Some(&idx) = state.block_index.get(&id) {
                    if let Some(block) = state.blocks.get_mut(idx) {
                        block.state = if exit_code == 0 {
                            BlockState::Success
                        } else {
                            BlockState::Failed(exit_code)
                        };
                        block.duration_ms = Some(block.started_at.elapsed().as_millis() as u64);
                        block.version += 1;
                    }
                }
                state.pty_handles.retain(|h| h.block_id != id);
                state.last_exit_code = Some(exit_code);
                if state.focus == Focus::Block(id) {
                    state.focus = Focus::Input;
                    state.input.focused = true;
                }
            }
            NexusMessage::KernelEvent(evt) => {
                return handle_kernel_event(state, evt, images);
            }
            NexusMessage::SendInterrupt => {
                // Send SIGINT (Ctrl+C) to focused PTY block or last running PTY
                let target = match state.focus {
                    Focus::Block(id) => Some(id),
                    Focus::Input => state.blocks.iter().rev()
                        .find(|b| b.is_running())
                        .map(|b| b.id),
                };
                if let Some(id) = target {
                    if let Some(handle) = state.pty_handles.iter().find(|h| h.block_id == id) {
                        let _ = handle.send_interrupt();
                    }
                }
            }
            NexusMessage::KillBlock(id) => {
                if let Some(handle) = state.pty_handles.iter().find(|h| h.block_id == id) {
                    let _ = handle.send_interrupt();
                    handle.kill();
                }
            }
            NexusMessage::PtyInput(event) => {
                if let Focus::Block(block_id) = state.focus {
                    if let Some(handle) = state.pty_handles.iter().find(|h| h.block_id == block_id) {
                        if let Some(bytes) = strata_key_to_bytes(&event) {
                            let _ = handle.write(&bytes);
                        }
                    } else {
                        // PTY gone (finished block) — return to input
                        state.focus = Focus::Input;
                        state.input.focused = true;
                    }
                }
            }

            // =============================================================
            // Agent
            // =============================================================
            NexusMessage::AgentEvent(evt) => {
                state.agent_dirty = true;
                return handle_agent_event(state, evt);
            }
            NexusMessage::ToggleThinking(id) => {
                if let Some(&idx) = state.agent_block_index.get(&id) {
                    if let Some(block) = state.agent_blocks.get_mut(idx) {
                        block.toggle_thinking();
                    }
                }
            }
            NexusMessage::ToggleTool(id, tool_index) => {
                if let Some(&idx) = state.agent_block_index.get(&id) {
                    if let Some(block) = state.agent_blocks.get_mut(idx) {
                        if let Some(tool) = block.tools.get_mut(tool_index) {
                            tool.collapsed = !tool.collapsed;
                            block.version += 1;
                        }
                    }
                }
            }
            NexusMessage::PermissionGrant(block_id, perm_id) => {
                if let Some(&idx) = state.agent_block_index.get(&block_id) {
                    if let Some(block) = state.agent_blocks.get_mut(idx) {
                        block.clear_permission();
                    }
                }
                if let Some(ref tx) = state.agent_permission_tx {
                    let _ = tx.send((perm_id, PermissionResponse::GrantedOnce));
                }
            }
            NexusMessage::PermissionGrantSession(block_id, perm_id) => {
                if let Some(&idx) = state.agent_block_index.get(&block_id) {
                    if let Some(block) = state.agent_blocks.get_mut(idx) {
                        block.clear_permission();
                    }
                }
                if let Some(ref tx) = state.agent_permission_tx {
                    let _ = tx.send((perm_id, PermissionResponse::GrantedSession));
                }
            }
            NexusMessage::PermissionDeny(block_id, perm_id) => {
                if let Some(&idx) = state.agent_block_index.get(&block_id) {
                    if let Some(block) = state.agent_blocks.get_mut(idx) {
                        block.clear_permission();
                        block.fail("Permission denied".to_string());
                    }
                }
                if let Some(ref tx) = state.agent_permission_tx {
                    let _ = tx.send((perm_id, PermissionResponse::Denied));
                }
                state.active_agent = None;
            }
            NexusMessage::AgentInterrupt => {
                if state.active_agent.is_some() {
                    state.agent_cancel_flag.store(true, Ordering::SeqCst);
                }
            }

            // =============================================================
            // Scroll
            // =============================================================
            NexusMessage::HistoryScroll(action) => {
                state.history_scroll.apply(action);
            }
            NexusMessage::CompletionScroll(action) => {
                state.completion_scroll.apply(action);
            }
            NexusMessage::HistorySearchScroll(action) => {
                state.history_search_scroll.apply(action);
            }

            // =============================================================
            // Selection
            // =============================================================
            NexusMessage::SelectionStart(addr) => {
                state.selection = Some(Selection::new(addr.clone(), addr));
                state.is_selecting = true;
            }
            NexusMessage::SelectionExtend(addr) => {
                if let Some(sel) = &mut state.selection {
                    sel.focus = addr;
                }
            }
            NexusMessage::SelectionEnd => {
                state.is_selecting = false;
            }
            NexusMessage::ClearSelection => {
                state.selection = None;
                state.is_selecting = false;
            }
            NexusMessage::Copy => {
                let mut copied = false;

                // First try: terminal/content selection
                if let Some(ref sel) = state.selection {
                    if !sel.is_collapsed() {
                        let text = extract_selected_text(state, sel);
                        if !text.is_empty() {
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                let _ = clipboard.set_text(&text);
                                copied = true;
                            }
                        }
                    }
                }

                // Fallback: input text selection
                if !copied {
                    if let Some((sel_start, sel_end)) = state.input.selection {
                        let start = sel_start.min(sel_end);
                        let end = sel_start.max(sel_end);
                        if start != end {
                            let selected: String = state.input.text.chars()
                                .skip(start).take(end - start).collect();
                            if !selected.is_empty() {
                                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                    let _ = clipboard.set_text(&selected);
                                }
                            }
                        }
                    }
                }
            }

            // =============================================================
            // Table sorting
            // =============================================================
            NexusMessage::SortTable(block_id, col_idx) => {
                if let Some(&idx) = state.block_index.get(&block_id) {
                    if let Some(block) = state.blocks.get_mut(idx) {
                        block.table_sort.toggle(col_idx);
                        // Sort the table rows in native_output
                        if let Some(Value::Table { ref mut rows, .. }) = block.native_output {
                            let ascending = block.table_sort.ascending;
                            rows.sort_by(|a, b| {
                                let va = a.get(col_idx).map(|v| v.to_text()).unwrap_or_default();
                                let vb = b.get(col_idx).map(|v| v.to_text()).unwrap_or_default();
                                // Try numeric comparison first
                                if let (Ok(na), Ok(nb)) = (va.parse::<f64>(), vb.parse::<f64>()) {
                                    let cmp = na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal);
                                    if ascending { cmp } else { cmp.reverse() }
                                } else {
                                    let cmp = va.cmp(&vb);
                                    if ascending { cmp } else { cmp.reverse() }
                                }
                            });
                        }
                    }
                }
            }

            // =============================================================
            // Completions
            // =============================================================
            NexusMessage::TabComplete => {
                let text = state.input.text.clone();
                let cursor = state.input.cursor;
                let (completions, anchor) = state.kernel.blocking_lock().complete(&text, cursor);
                if completions.len() == 1 {
                    // Single completion: apply immediately
                    let comp = &completions[0];
                    let mut t = text;
                    let end = cursor.min(t.len());
                    t.replace_range(anchor..end, &comp.text);
                    state.input.cursor = anchor + comp.text.len();
                    state.input.text = t;
                    state.completions.clear();
                    state.completion_index = None;
                } else if !completions.is_empty() {
                    state.completions = completions;
                    state.completion_index = Some(0);
                    state.completion_anchor = anchor;
                    state.completion_scroll.offset = 0.0;
                }
            }
            NexusMessage::CompletionNav(delta) => {
                if !state.completions.is_empty() {
                    let len = state.completions.len() as isize;
                    let current = state.completion_index.unwrap_or(0) as isize;
                    let new_idx = ((current + delta) % len + len) % len;
                    state.completion_index = Some(new_idx as usize);
                    scroll_to_index(&mut state.completion_scroll, new_idx as usize, 26.0, 300.0);
                }
            }
            NexusMessage::CompletionAccept => {
                if let Some(idx) = state.completion_index {
                    if let Some(comp) = state.completions.get(idx) {
                        let mut t = state.input.text.clone();
                        let cursor = state.input.cursor;
                        let end = cursor.min(t.len());
                        t.replace_range(state.completion_anchor..end, &comp.text);
                        state.input.cursor = state.completion_anchor + comp.text.len();
                        state.input.text = t;
                    }
                }
                state.completions.clear();
                state.completion_index = None;
            }
            NexusMessage::CompletionDismiss => {
                state.completions.clear();
                state.completion_index = None;
            }
            NexusMessage::CompletionSelect(index) => {
                if let Some(comp) = state.completions.get(index) {
                    let mut t = state.input.text.clone();
                    let cursor = state.input.cursor;
                    let end = cursor.min(t.len());
                    t.replace_range(state.completion_anchor..end, &comp.text);
                    state.input.cursor = state.completion_anchor + comp.text.len();
                    state.input.text = t;
                }
                state.completions.clear();
                state.completion_index = None;
            }

            // =============================================================
            // History search (Ctrl+R)
            // =============================================================
            NexusMessage::HistorySearchToggle => {
                if state.history_search_active {
                    // Cycle to next result
                    if !state.history_search_results.is_empty() {
                        state.history_search_index = (state.history_search_index + 1) % state.history_search_results.len();
                        scroll_to_index(&mut state.history_search_scroll, state.history_search_index, 30.0, 300.0);
                    }
                } else {
                    state.history_search_active = true;
                    state.history_search_query.clear();
                    state.history_search_results.clear();
                    state.history_search_index = 0;
                    state.history_search_scroll.offset = 0.0;
                }
            }
            NexusMessage::HistorySearchKey(key_event) => {
                if let KeyEvent::Pressed { key, .. } = key_event {
                    match key {
                        Key::Character(c) => {
                            state.history_search_query.push_str(&c);
                        }
                        Key::Named(NamedKey::Backspace) => {
                            state.history_search_query.pop();
                        }
                        _ => {}
                    }
                    // Re-search
                    if state.history_search_query.is_empty() {
                        state.history_search_results.clear();
                    } else {
                        let results = state.kernel.blocking_lock()
                            .search_history(&state.history_search_query, 50);
                        state.history_search_results = results.into_iter().map(|e| e.command).collect();
                    }
                    state.history_search_index = 0;
                }
            }
            NexusMessage::HistorySearchAccept => {
                if let Some(result) = state.history_search_results.get(state.history_search_index) {
                    state.input.text = result.clone();
                    state.input.cursor = result.len();
                }
                state.history_search_active = false;
                state.history_search_query.clear();
                state.history_search_results.clear();
            }
            NexusMessage::HistorySearchDismiss => {
                state.history_search_active = false;
                state.history_search_query.clear();
                state.history_search_results.clear();
            }
            NexusMessage::HistorySearchSelect(index) => {
                if index < state.history_search_results.len() {
                    state.history_search_index = index;
                    scroll_to_index(&mut state.history_search_scroll, index, 30.0, 300.0);
                }
            }
            NexusMessage::HistorySearchAcceptIndex(index) => {
                if let Some(result) = state.history_search_results.get(index) {
                    state.input.text = result.clone();
                    state.input.cursor = result.len();
                }
                state.history_search_active = false;
                state.history_search_query.clear();
                state.history_search_results.clear();
            }

            // =============================================================
            // Job indicator
            // =============================================================
            NexusMessage::ScrollToJob(_job_id) => {
                // Scroll to bottom where running blocks are
                state.history_scroll.offset = state.history_scroll.max.get();
            }

            // =============================================================
            // Context menu
            // =============================================================
            NexusMessage::ShowContextMenu(x, y, items, target) => {
                state.context_menu = Some(ContextMenuState { x, y, items, target, hovered_item: Cell::new(None) });
            }
            NexusMessage::ContextMenuAction(item) => {
                let target = state.context_menu.as_ref().map(|m| m.target.clone());
                state.context_menu = None;
                match item {
                    ContextMenuItem::Copy => {
                        if let Some(text) = target.and_then(|t| extract_block_text(state, &t)) {
                            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                                let _ = clipboard.set_text(&text);
                            }
                        }
                    }
                    ContextMenuItem::Paste => {
                        return Command::message(NexusMessage::Paste);
                    }
                    ContextMenuItem::SelectAll => {
                        match target.as_ref() {
                            Some(ContextTarget::Input) | None => {
                                state.input.select_all();
                            }
                            Some(ContextTarget::Block(_)) | Some(ContextTarget::AgentBlock(_)) => {
                                let ordering = build_source_ordering(state);
                                let sources = ordering.sources_in_order();
                                if let (Some(&first), Some(&last)) = (sources.first(), sources.last()) {
                                    state.selection = Some(Selection::new(
                                        ContentAddress::start_of(first),
                                        ContentAddress::new(last, usize::MAX, usize::MAX),
                                    ));
                                    state.is_selecting = false;
                                }
                            }
                        }
                    }
                    ContextMenuItem::Clear => {
                        state.input.text.clear();
                        state.input.cursor = 0;
                        state.input.selection = None;
                    }
                }
            }
            NexusMessage::DismissContextMenu => {
                state.context_menu = None;
            }

            // =============================================================
            // Clipboard
            // =============================================================
            NexusMessage::Paste => {
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    // Try image first
                    if let Ok(img) = clipboard.get_image() {
                        let width = img.width as u32;
                        let height = img.height as u32;
                        let rgba_data = img.bytes.into_owned();

                        // Convert to PNG for storage
                        let mut png_data = Vec::new();
                        if let Some(img_buf) = image::RgbaImage::from_raw(width, height, rgba_data.clone()) {
                            let _ = img_buf.write_to(
                                &mut std::io::Cursor::new(&mut png_data),
                                image::ImageFormat::Png,
                            );
                        }

                        if !png_data.is_empty() {
                            let handle = images.load_rgba(width, height, rgba_data);
                            state.attachments.push(Attachment {
                                data: png_data,
                                image_handle: handle,
                                width,
                                height,
                            });
                        }
                    } else if let Ok(text) = clipboard.get_text() {
                        if !text.is_empty() {
                            state.input.insert_str(&text);
                        }
                    }
                }
            }
            NexusMessage::RemoveAttachment(idx) => {
                if idx < state.attachments.len() {
                    state.attachments.remove(idx);
                }
            }

            // =============================================================
            // Multiline
            // =============================================================
            NexusMessage::InsertNewline => {
                state.input.insert_newline();
            }

            // =============================================================
            // Window
            // =============================================================
            NexusMessage::ClearScreen => {
                // Kill all PTYs
                for handle in &state.pty_handles {
                    let _ = handle.send_interrupt();
                    handle.kill();
                }
                state.pty_handles.clear();
                // Cancel active agent
                if state.active_agent.is_some() {
                    state.agent_cancel_flag.store(true, Ordering::SeqCst);
                    state.active_agent = None;
                }
                // Clear all blocks
                state.blocks.clear();
                state.block_index.clear();
                state.agent_blocks.clear();
                state.agent_block_index.clear();
                state.jobs.clear();
                state.history_scroll.offset = 0.0;
                state.focus = Focus::Input;
                state.input.focused = true;
            }
            NexusMessage::CloseWindow => {
                state.exit_requested = true;
            }
            NexusMessage::BlurAll => {
                state.focus = Focus::Input;
                state.input.focused = true;
            }
            NexusMessage::Tick => {
                if state.is_dirty() {
                    state.terminal_dirty = false;
                    state.agent_dirty = false;
                    // Auto-scroll to bottom on new content
                    state.history_scroll.offset = state.history_scroll.max.get();
                }
            }
        }

        // Propagate terminal size changes to running PTY handles
        let current_size = state.terminal_size.get();
        if current_size != state.last_terminal_size {
            state.last_terminal_size = current_size;
            let (cols, rows) = current_size;
            for handle in &state.pty_handles {
                let _ = handle.resize(cols, rows);
            }
        }

        Command::none()
    }

    fn view(state: &Self::State, snapshot: &mut LayoutSnapshot) {
        let vp = snapshot.viewport();
        let vw = vp.width;
        let vh = vp.height;

        // Recalculate terminal size from viewport
        let char_width = crate::constants::DEFAULT_FONT_SIZE * crate::constants::CHAR_WIDTH_RATIO;
        let line_height = crate::constants::DEFAULT_FONT_SIZE * crate::constants::LINE_HEIGHT_FACTOR;
        let h_padding = 4.0 + 6.0 * 2.0; // outer padding + block padding
        let v_padding = 44.0;
        let cols = ((vw - h_padding) / char_width) as u16;
        let rows = ((vh - v_padding) / line_height) as u16;
        state.terminal_size.set((cols.max(40).min(500), rows.max(5).min(200)));

        // Cursor blink
        let now = Instant::now();
        let blink_elapsed = now.duration_since(state.last_edit_time).as_millis();
        let cursor_visible = (blink_elapsed / 500) % 2 == 0;

        // Build unified block list
        let unified = state.unified_blocks();
        let has_blocks = !unified.is_empty();

        // Build history content
        let mut scroll = ScrollColumn::from_state(&state.history_scroll)
            .spacing(4.0)
            .width(Length::Fill)
            .height(Length::Fill);

        if !has_blocks {
            // Welcome screen
            scroll = scroll.push(WelcomeScreen { cwd: &state.cwd });
        } else {
            // Render unified blocks
            for block_ref in &unified {
                match block_ref {
                    UnifiedBlockRef::Shell(block) => {
                        let kill_id = source_ids::kill(block.id);
                        let image_info = state.image_handles.get(&block.id).copied();
                        let is_focused = matches!(state.focus, Focus::Block(id) if id == block.id);
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

        // Main layout: Column with ScrollColumn + optional JobBar + InputBar
        let mut main_col = Column::new()
            .width(Length::Fixed(vw))
            .height(Length::Fixed(vh))
            .padding(0.0);

        // History area (takes remaining space)
        main_col = main_col.push(
            Column::new()
                .padding_custom(Padding::new(2.0, 4.0, 0.0, 4.0))
                .width(Length::Fill)
                .height(Length::Fill)
                .push(scroll),
        );

        // Job bar (only if jobs exist)
        if !state.jobs.is_empty() {
            main_col = main_col.push(JobBar { jobs: &state.jobs });
        }

        // Completion popup (above input bar)
        if !state.completions.is_empty() {
            main_col = main_col.push(CompletionPopup {
                completions: &state.completions,
                selected_index: state.completion_index,
                hovered_index: state.hovered_completion.get(),
                scroll: &state.completion_scroll,
            });
        }

        // History search overlay (above input bar when active)
        if state.history_search_active {
            main_col = main_col.push(HistorySearchBar {
                query: &state.history_search_query,
                results: &state.history_search_results,
                result_index: state.history_search_index,
                hovered_index: state.hovered_history_result.get(),
                scroll: &state.history_search_scroll,
            });
        }

        // Attachment thumbnails (above input bar)
        if !state.attachments.is_empty() {
            let mut attach_row = Row::new().spacing(8.0).padding(4.0);
            for (i, attachment) in state.attachments.iter().enumerate() {
                let scale = (60.0_f32 / attachment.width as f32).min(60.0 / attachment.height as f32).min(1.0);
                let w = attachment.width as f32 * scale;
                let h = attachment.height as f32 * scale;
                let remove_id = SourceId::named(&format!("remove_attach_{}", i));
                attach_row = attach_row.push(
                    Column::new()
                        .spacing(2.0)
                        .cross_align(CrossAxisAlignment::Center)
                        .image(ImageElement::new(attachment.image_handle, w, h).corner_radius(4.0))
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

        // Input bar
        main_col = main_col.push(
            Column::new()
                .padding_custom(Padding::new(2.0, 4.0, 4.0, 4.0))
                .width(Length::Fill)
                .push(NexusInputBar {
                    input: &state.input,
                    mode: state.mode,
                    cwd: &state.cwd,
                    last_exit_code: state.last_exit_code,
                    cursor_visible,
                    mode_toggle_id: SourceId::named("mode_toggle"),
                    line_count: {
                        let count = state.input.text.lines().count()
                            + if state.input.text.ends_with('\n') { 1 } else { 0 };
                        count.max(1).min(6)
                    },
                }),
        );

        main_col.layout(snapshot, Rect::new(0.0, 0.0, vw, vh));

        // Sync layout state
        state.history_scroll.sync_from_snapshot(snapshot);
        state.completion_scroll.sync_from_snapshot(snapshot);
        state.history_search_scroll.sync_from_snapshot(snapshot);
        state.input.sync_from_snapshot(snapshot);

        // Context menu overlay (rendered after layout, absolute positioned)
        if let Some(ref menu) = state.context_menu {
            render_context_menu(snapshot, menu);
        }
    }

    fn selection(state: &Self::State) -> Option<&Selection> {
        state.selection.as_ref()
    }

    fn on_mouse(
        state: &Self::State,
        event: MouseEvent,
        hit: Option<HitResult>,
        capture: &CaptureState,
    ) -> MouseResponse<Self::Message> {
        // Composable scroll + input handlers
        route_mouse!(&event, &hit, capture, [
            state.completion_scroll       => NexusMessage::CompletionScroll,
            state.history_search_scroll   => NexusMessage::HistorySearchScroll,
            state.history_scroll          => NexusMessage::HistoryScroll,
            state.input                   => NexusMessage::InputMouse,
        ]);

        // Right-click → context menu
        if let MouseEvent::ButtonPressed { button: MouseButton::Right, position, .. } = &event {
            // Check if hit is on input
            let input_bounds = state.input.bounds();
            if position.x >= input_bounds.x && position.x <= input_bounds.x + input_bounds.width
                && position.y >= input_bounds.y && position.y <= input_bounds.y + input_bounds.height
            {
                return MouseResponse::message(NexusMessage::ShowContextMenu(
                    position.x, position.y,
                    vec![ContextMenuItem::Paste, ContextMenuItem::SelectAll, ContextMenuItem::Clear],
                    ContextTarget::Input,
                ));
            }

            // Check if hit is on a terminal block (by matching source IDs)
            if let Some(HitResult::Content(ref addr)) = hit {
                for block in &state.blocks {
                    let term_id = source_ids::shell_term(block.id);
                    if addr.source_id == term_id {
                        return MouseResponse::message(NexusMessage::ShowContextMenu(
                            position.x, position.y,
                            vec![ContextMenuItem::Copy, ContextMenuItem::SelectAll],
                            ContextTarget::Block(block.id),
                        ));
                    }
                }
            }

            // Check if hit is on any block area (widget hits like buttons, etc.)
            // Use the last block as fallback — right-click anywhere in content area
            if hit.is_some() {
                // Find the most recent block as a reasonable target
                if let Some(block) = state.blocks.last() {
                    return MouseResponse::message(NexusMessage::ShowContextMenu(
                        position.x, position.y,
                        vec![ContextMenuItem::Copy, ContextMenuItem::SelectAll],
                        ContextTarget::Block(block.id),
                    ));
                }
                // Agent blocks
                if let Some(block) = state.agent_blocks.last() {
                    return MouseResponse::message(NexusMessage::ShowContextMenu(
                        position.x, position.y,
                        vec![ContextMenuItem::Copy, ContextMenuItem::SelectAll],
                        ContextTarget::AgentBlock(block.id),
                    ));
                }
            }
        }

        // Hover tracking for popups
        if let MouseEvent::CursorMoved { .. } = &event {
            if let Some(ref menu) = state.context_menu {
                let idx = if let Some(HitResult::Widget(id)) = &hit {
                    (0..menu.items.len()).find(|i| *id == SourceId::named(&format!("ctx_menu_{}", i)))
                } else {
                    None
                };
                menu.hovered_item.set(idx);
            }

            // Completion popup hover
            if !state.completions.is_empty() {
                let idx = if let Some(HitResult::Widget(id)) = &hit {
                    (0..state.completions.len().min(10)).find(|i| *id == CompletionPopup::item_id(*i))
                } else {
                    None
                };
                state.hovered_completion.set(idx);
            }

            // History search hover
            if state.history_search_active {
                let idx = if let Some(HitResult::Widget(id)) = &hit {
                    (0..state.history_search_results.len().min(10)).find(|i| *id == HistorySearchBar::result_id(*i))
                } else {
                    None
                };
                state.hovered_history_result.set(idx);
            }
        }

        // Context menu item clicks
        if let MouseEvent::ButtonPressed { button: MouseButton::Left, .. } = &event {
            if let Some(ref menu) = state.context_menu {
                if let Some(HitResult::Widget(id)) = &hit {
                    for (i, item) in menu.items.iter().enumerate() {
                        if *id == SourceId::named(&format!("ctx_menu_{}", i)) {
                            return MouseResponse::message(NexusMessage::ContextMenuAction(*item));
                        }
                    }
                }
                // Click anywhere when menu is open → dismiss
                return MouseResponse::message(NexusMessage::DismissContextMenu);
            }
        }

        // Button clicks
        if let MouseEvent::ButtonPressed { button: MouseButton::Left, .. } = &event {
            if let Some(HitResult::Widget(id)) = &hit {
                // Mode toggle
                if *id == SourceId::named("mode_toggle") {
                    return MouseResponse::message(NexusMessage::ToggleMode);
                }

                // Completion item clicks
                for i in 0..state.completions.len().min(10) {
                    if *id == CompletionPopup::item_id(i) {
                        return MouseResponse::message(NexusMessage::CompletionSelect(i));
                    }
                }

                // History search result clicks — select and accept
                if state.history_search_active {
                    for i in 0..state.history_search_results.len().min(10) {
                        if *id == HistorySearchBar::result_id(i) {
                            // Click selects the index, then accept is handled via update
                            return MouseResponse::message(NexusMessage::HistorySearchAcceptIndex(i));
                        }
                    }
                }

                // Attachment remove buttons
                for i in 0..state.attachments.len() {
                    let remove_id = SourceId::named(&format!("remove_attach_{}", i));
                    if *id == remove_id {
                        return MouseResponse::message(NexusMessage::RemoveAttachment(i));
                    }
                }

                // Job pill clicks
                for job in &state.jobs {
                    if *id == JobBar::job_pill_id(job.id) {
                        return MouseResponse::message(NexusMessage::ScrollToJob(job.id));
                    }
                }

                // Kill buttons
                for block in &state.blocks {
                    if block.is_running() {
                        let kill_id = source_ids::kill(block.id);
                        if *id == kill_id {
                            return MouseResponse::message(NexusMessage::KillBlock(block.id));
                        }
                    }
                }

                // Agent thinking toggles
                for block in &state.agent_blocks {
                    let thinking_id = source_ids::agent_thinking_toggle(block.id);
                    if *id == thinking_id {
                        return MouseResponse::message(NexusMessage::ToggleThinking(block.id));
                    }

                    // Stop button
                    let stop_id = source_ids::agent_stop(block.id);
                    if *id == stop_id {
                        return MouseResponse::message(NexusMessage::AgentInterrupt);
                    }

                    // Tool toggles
                    for (i, _tool) in block.tools.iter().enumerate() {
                        let toggle_id = source_ids::agent_tool_toggle(block.id, i);
                        if *id == toggle_id {
                            return MouseResponse::message(NexusMessage::ToggleTool(block.id, i));
                        }
                    }

                    // Permission buttons
                    if let Some(ref perm) = block.pending_permission {
                        let deny_id = source_ids::agent_perm_deny(block.id);
                        let allow_id = source_ids::agent_perm_allow(block.id);
                        let always_id = source_ids::agent_perm_always(block.id);

                        if *id == deny_id {
                            return MouseResponse::message(NexusMessage::PermissionDeny(block.id, perm.id.clone()));
                        }
                        if *id == allow_id {
                            return MouseResponse::message(NexusMessage::PermissionGrant(block.id, perm.id.clone()));
                        }
                        if *id == always_id {
                            return MouseResponse::message(NexusMessage::PermissionGrantSession(block.id, perm.id.clone()));
                        }
                    }
                }

                // Table sort header clicks
                for block in &state.blocks {
                    if let Some(Value::Table { columns, .. }) = &block.native_output {
                        for col_idx in 0..columns.len() {
                            let sort_id = source_ids::table_sort(block.id, col_idx);
                            if *id == sort_id {
                                return MouseResponse::message(NexusMessage::SortTable(block.id, col_idx));
                            }
                        }
                    }
                }

                // Content selection start (clicked text)
            }

            // Text content selection (start immediately, even if input was focused)
            if let Some(HitResult::Content(addr)) = hit {
                let capture_source = addr.source_id;
                return MouseResponse::message_and_capture(
                    NexusMessage::SelectionStart(addr),
                    capture_source,
                );
            }

            // Clicked empty space: blur inputs
            if state.input.focused {
                return MouseResponse::message(NexusMessage::BlurAll);
            }
        }

        // Selection drag
        if let MouseEvent::CursorMoved { .. } = &event {
            if let CaptureState::Captured(_) = capture {
                if let Some(HitResult::Content(addr)) = hit {
                    return MouseResponse::message(NexusMessage::SelectionExtend(addr));
                }
            }
        }

        // Selection release
        if let MouseEvent::ButtonReleased { button: MouseButton::Left, .. } = &event {
            if let CaptureState::Captured(_) = capture {
                return MouseResponse::message_and_release(NexusMessage::SelectionEnd);
            }
        }

        MouseResponse::none()
    }

    fn on_key(
        state: &Self::State,
        event: KeyEvent,
    ) -> Option<Self::Message> {
        // Only handle presses
        if matches!(&event, KeyEvent::Released { .. }) {
            return None;
        }

        if let KeyEvent::Pressed { ref key, ref modifiers, .. } = event {
            // History search mode intercepts most keys
            if state.history_search_active {
                if modifiers.ctrl {
                    if let Key::Character(c) = key {
                        if c == "r" {
                            return Some(NexusMessage::HistorySearchToggle);
                        }
                    }
                }
                return match key {
                    Key::Named(NamedKey::Enter) => Some(NexusMessage::HistorySearchAccept),
                    Key::Named(NamedKey::Escape) => Some(NexusMessage::HistorySearchDismiss),
                    Key::Named(NamedKey::ArrowDown) => {
                        if !state.history_search_results.is_empty()
                            && state.history_search_index < state.history_search_results.len() - 1
                        {
                            Some(NexusMessage::HistorySearchSelect(state.history_search_index + 1))
                        } else {
                            None
                        }
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        if state.history_search_index > 0 {
                            Some(NexusMessage::HistorySearchSelect(state.history_search_index - 1))
                        } else {
                            None
                        }
                    }
                    _ => Some(NexusMessage::HistorySearchKey(event)),
                };
            }

            // Completion popup intercepts keys when visible
            if !state.completions.is_empty() {
                return match key {
                    Key::Named(NamedKey::Tab) if modifiers.shift => Some(NexusMessage::CompletionNav(-1)),
                    Key::Named(NamedKey::Tab) => Some(NexusMessage::CompletionNav(1)),
                    Key::Named(NamedKey::ArrowDown) => Some(NexusMessage::CompletionNav(1)),
                    Key::Named(NamedKey::ArrowUp) => Some(NexusMessage::CompletionNav(-1)),
                    Key::Named(NamedKey::Enter) => Some(NexusMessage::CompletionAccept),
                    Key::Named(NamedKey::Escape) => Some(NexusMessage::CompletionDismiss),
                    _ => {
                        // Dismiss completions, then handle the key normally
                        // We can't send two messages from on_key, so dismiss is a side-effect
                        // and we return the normal key handling.
                        // Actually, we need to dismiss first. Return dismiss and the key
                        // will be re-sent on next frame... or just dismiss and pass through.
                        // Simplest: dismiss. User types again and re-triggers if needed.
                        Some(NexusMessage::CompletionDismiss)
                    }
                };
            }

            // Cmd shortcuts (global)
            if modifiers.meta {
                if let Key::Character(c) = key {
                    match c.as_str() {
                        "q" => return Some(NexusMessage::CloseWindow),
                        "k" => return Some(NexusMessage::ClearScreen),
                        "w" => return Some(NexusMessage::CloseWindow),
                        "c" => return Some(NexusMessage::Copy),
                        "v" => return Some(NexusMessage::Paste),
                        "." => return Some(NexusMessage::ToggleMode),
                        _ => {}
                    }
                }
            }

            // Ctrl shortcuts (global)
            if modifiers.ctrl {
                if let Key::Character(c) = key {
                    match c.as_str() {
                        "r" => return Some(NexusMessage::HistorySearchToggle),
                        "c" => {
                            // Ctrl+C: interrupt agent or send SIGINT to focused/last running PTY
                            if state.active_agent.is_some() {
                                return Some(NexusMessage::AgentInterrupt);
                            }
                            return Some(NexusMessage::SendInterrupt);
                        }
                        _ => {}
                    }
                }
            }

            // Escape: dismiss overlays, interrupt agent, leave PTY focus, clear selection
            if matches!(key, Key::Named(NamedKey::Escape)) {
                if state.context_menu.is_some() {
                    return Some(NexusMessage::DismissContextMenu);
                }
                if state.active_agent.is_some() {
                    return Some(NexusMessage::AgentInterrupt);
                }
                if matches!(state.focus, Focus::Block(_)) {
                    return Some(NexusMessage::BlurAll);
                }
                if state.selection.is_some() {
                    return Some(NexusMessage::ClearSelection);
                }
            }

            // When a PTY block is focused, forward keys to it
            if let Focus::Block(_) = state.focus {
                return Some(NexusMessage::PtyInput(event));
            }

            // When input is focused, route keys
            if state.input.focused {
                // Shift+Enter → insert newline (multiline input)
                if matches!(key, Key::Named(NamedKey::Enter)) && modifiers.shift {
                    return Some(NexusMessage::InsertNewline);
                }

                // Tab → trigger completion
                if matches!(key, Key::Named(NamedKey::Tab)) {
                    return Some(NexusMessage::TabComplete);
                }

                // Arrow Up/Down → history navigation (only if single-line or cursor on first/last line)
                if matches!(key, Key::Named(NamedKey::ArrowUp)) {
                    return Some(NexusMessage::HistoryUp);
                }
                if matches!(key, Key::Named(NamedKey::ArrowDown)) {
                    return Some(NexusMessage::HistoryDown);
                }

                return Some(NexusMessage::InputKey(event));
            }

            // Global shortcuts when input not focused
            match key {
                Key::Named(NamedKey::PageUp) => {
                    return Some(NexusMessage::HistoryScroll(ScrollAction::ScrollBy(300.0)));
                }
                Key::Named(NamedKey::PageDown) => {
                    return Some(NexusMessage::HistoryScroll(ScrollAction::ScrollBy(-300.0)));
                }
                _ => {}
            }
        }

        None
    }

    fn subscription(state: &Self::State) -> Subscription<Self::Message> {
        let mut subs = Vec::new();

        // PTY subscription
        let pty_rx = state.pty_rx.clone();
        subs.push(Subscription::from_iced(
            pty_subscription(pty_rx).map(|(id, evt)| match evt {
                PtyEvent::Output(data) => NexusMessage::PtyOutput(id, data),
                PtyEvent::Exited(code) => NexusMessage::PtyExited(id, code),
            }),
        ));

        // Kernel subscription
        let kernel_rx = state.kernel_rx.clone();
        subs.push(Subscription::from_iced(
            kernel_subscription(kernel_rx).map(NexusMessage::KernelEvent),
        ));

        // Agent subscription
        let agent_rx = state.agent_event_rx.clone();
        subs.push(Subscription::from_iced(
            agent_subscription(agent_rx).map(NexusMessage::AgentEvent),
        ));

        // Tick subscription when dirty (drives auto-scroll)
        if state.is_dirty() {
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

    fn background_color(_state: &Self::State) -> crate::strata::primitives::Color {
        colors::BG_APP
    }

    fn should_exit(state: &Self::State) -> bool {
        state.exit_requested
    }
}

// =========================================================================
// Command execution
// =========================================================================

fn execute_command(state: &mut NexusState, command: String) -> Command<NexusMessage> {
    let trimmed = command.trim().to_string();

    state.shell_history_index = None;
    state.agent_history_index = None;
    state.saved_input.clear();

    let block_id = state.next_id();

    // Handle built-in: clear
    if trimmed == "clear" {
        state.agent_cancel_flag.store(true, Ordering::SeqCst);
        state.blocks.clear();
        state.block_index.clear();
        state.agent_blocks.clear();
        state.agent_block_index.clear();
        state.active_agent = None;
        return Command::none();
    }

    // Classify and execute
    let classification = state.kernel.blocking_lock().classify_command(&trimmed);

    if classification == CommandClassification::Kernel {
        // Kernel (pipeline/native) command
        let mut block = Block::new(block_id, trimmed.clone());
        let (ts_cols, ts_rows) = state.terminal_size.get();
        block.parser = TerminalParser::new(ts_cols, ts_rows);
        let block_idx = state.blocks.len();
        state.blocks.push(block);
        state.block_index.insert(block_id, block_idx);

        let kernel = state.kernel.clone();
        let kernel_tx = state.kernel_tx.clone();
        let cwd = state.cwd.clone();
        let cmd = trimmed;

        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                rt.block_on(async {
                    let mut kernel = kernel.lock().await;
                    let _ = kernel
                        .state_mut()
                        .set_cwd(std::path::PathBuf::from(&cwd));
                    let _ = kernel.execute_with_block_id(&cmd, Some(block_id));
                });
            }));

            if let Err(panic_info) = result {
                let error_msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                    format!("Command panicked: {}", s)
                } else if let Some(s) = panic_info.downcast_ref::<String>() {
                    format!("Command panicked: {}", s)
                } else {
                    "Command panicked (unknown error)".to_string()
                };
                let _ = kernel_tx.send(ShellEvent::StderrChunk {
                    block_id,
                    data: format!("{}\n", error_msg).into_bytes(),
                });
                let _ = kernel_tx.send(ShellEvent::CommandFinished {
                    block_id,
                    exit_code: 1,
                    duration_ms: 0,
                });
            }
        });

        // Auto-scroll
        state.history_scroll.offset = state.history_scroll.max.get();
        return Command::none();
    }

    // External command - use PTY
    let mut block = Block::new(block_id, trimmed.clone());
    let (ts_cols, ts_rows) = state.terminal_size.get();
    block.parser = TerminalParser::new(ts_cols, ts_rows);
    let block_idx = state.blocks.len();
    state.blocks.push(block);
    state.block_index.insert(block_id, block_idx);

    state.focus = Focus::Block(block_id);
    state.input.focused = false;

    let tx = state.pty_tx.clone();
    let cwd = state.cwd.clone();
    let (cols, rows) = state.terminal_size.get();

    match PtyHandle::spawn_with_size(&trimmed, &cwd, block_id, tx, cols, rows) {
        Ok(handle) => {
            state.pty_handles.push(handle);
            state.history_scroll.offset = state.history_scroll.max.get();
        }
        Err(e) => {
            tracing::error!("Failed to spawn PTY: {}", e);
            if let Some(&idx) = state.block_index.get(&block_id) {
                if let Some(block) = state.blocks.get_mut(idx) {
                    block.state = BlockState::Failed(1);
                    block.parser.feed(format!("Error: {}\n", e).as_bytes());
                    block.version += 1;
                }
            }
            state.focus = Focus::Input;
            state.input.focused = true;
        }
    }

    Command::none()
}

// =========================================================================
// Kernel event handler
// =========================================================================

fn handle_kernel_event(state: &mut NexusState, evt: ShellEvent, images: &mut ImageStore) -> Command<NexusMessage> {
    match evt {
        ShellEvent::CommandStarted { block_id, command, .. } => {
            if !state.block_index.contains_key(&block_id) {
                let mut block = Block::new(block_id, command);
                let (ts_cols, ts_rows) = state.terminal_size.get();
                block.parser = TerminalParser::new(ts_cols, ts_rows);
                let block_idx = state.blocks.len();
                state.blocks.push(block);
                state.block_index.insert(block_id, block_idx);
            }
        }
        ShellEvent::StdoutChunk { block_id, data } | ShellEvent::StderrChunk { block_id, data } => {
            if let Some(&idx) = state.block_index.get(&block_id) {
                if let Some(block) = state.blocks.get_mut(idx) {
                    block.parser.feed(&data);
                    block.version += 1;
                }
            }
            state.terminal_dirty = true;
        }
        ShellEvent::CommandOutput { block_id, value } => {
            // If it's an image, decode and load into GPU
            if let Value::Media { ref data, ref content_type, .. } = value {
                if content_type.starts_with("image/") {
                    if let Ok(img) = image::load_from_memory(data) {
                        let rgba = img.to_rgba8();
                        let (w, h) = rgba.dimensions();
                        let handle = images.load_rgba(w, h, rgba.into_raw());
                        state.image_handles.insert(block_id, (handle, w, h));
                    }
                }
            }
            if let Some(&idx) = state.block_index.get(&block_id) {
                if let Some(block) = state.blocks.get_mut(idx) {
                    block.native_output = Some(value);
                }
            }
        }
        ShellEvent::CommandFinished { block_id, exit_code, duration_ms } => {
            if let Some(&idx) = state.block_index.get(&block_id) {
                if let Some(block) = state.blocks.get_mut(idx) {
                    block.state = if exit_code == 0 {
                        BlockState::Success
                    } else {
                        BlockState::Failed(exit_code)
                    };
                    block.duration_ms = Some(duration_ms);
                    block.version += 1;

                    // Context update
                    let cmd = block.command.clone();
                    let output = block.parser.grid_with_scrollback().to_string();
                    let output_trimmed = if output.len() > 10_000 {
                        output[output.len() - 10_000..].to_string()
                    } else {
                        output
                    };
                    state.context.on_command_finished(cmd, output_trimmed, exit_code);
                }
            }
            state.last_exit_code = Some(exit_code);
            state.focus = Focus::Input;
            state.input.focused = true;
            state.history_scroll.offset = state.history_scroll.max.get();
        }
        ShellEvent::JobStateChanged { job_id, state: job_state } => {
            match job_state {
                nexus_api::JobState::Running => {
                    if let Some(job) = state.jobs.iter_mut().find(|j| j.id == job_id) {
                        job.state = VisualJobState::Running;
                    } else {
                        state.jobs.push(VisualJob::new(
                            job_id,
                            format!("Job {}", job_id),
                            VisualJobState::Running,
                        ));
                    }
                }
                nexus_api::JobState::Stopped => {
                    if let Some(job) = state.jobs.iter_mut().find(|j| j.id == job_id) {
                        job.state = VisualJobState::Stopped;
                    } else {
                        state.jobs.push(VisualJob::new(
                            job_id,
                            format!("Job {}", job_id),
                            VisualJobState::Stopped,
                        ));
                    }
                }
                nexus_api::JobState::Done(_) => {
                    state.jobs.retain(|j| j.id != job_id);
                }
            }
        }
        ShellEvent::CwdChanged { new, .. } => {
            state.cwd = new.display().to_string();
            let _ = std::env::set_current_dir(&new);
        }
        _ => {}
    }

    Command::none()
}

// =========================================================================
// Agent event handler
// =========================================================================

fn handle_agent_event(state: &mut NexusState, event: AgentEvent) -> Command<NexusMessage> {
    // Handle session ID
    if let AgentEvent::SessionStarted { ref session_id } = event {
        state.agent_session_id = Some(session_id.clone());
    }

    if let Some(block_id) = state.active_agent {
        if let Some(&idx) = state.agent_block_index.get(&block_id) {
            if let Some(block) = state.agent_blocks.get_mut(idx) {
                match event {
                    AgentEvent::SessionStarted { .. } => {}
                    AgentEvent::Started { .. } => {
                        block.state = AgentBlockState::Streaming;
                    }
                    AgentEvent::ResponseText(text) => {
                        block.append_response(&text);
                    }
                    AgentEvent::ThinkingText(text) => {
                        block.append_thinking(&text);
                    }
                    AgentEvent::ToolStarted { id, name } => {
                        block.start_tool(id, name);
                    }
                    AgentEvent::ToolParameter { tool_id, name, value } => {
                        block.add_tool_parameter(&tool_id, name, value);
                    }
                    AgentEvent::ToolOutput { tool_id, chunk } => {
                        block.append_tool_output(&tool_id, &chunk);
                    }
                    AgentEvent::ToolEnded { .. } => {}
                    AgentEvent::ToolStatus { id, status, message, output } => {
                        block.update_tool_status(&id, status, message, output);
                    }
                    AgentEvent::ImageAdded { media_type, data } => {
                        block.add_image(media_type, data);
                    }
                    AgentEvent::PermissionRequested { id, tool_name, tool_id, description, action, working_dir } => {
                        block.request_permission(PermissionRequest {
                            id,
                            tool_name,
                            tool_id,
                            description,
                            action,
                            working_dir,
                        });
                    }
                    AgentEvent::Finished { .. } => {
                        block.complete();
                        state.active_agent = None;
                    }
                    AgentEvent::Interrupted { .. } => {
                        block.state = AgentBlockState::Interrupted;
                        state.active_agent = None;
                    }
                    AgentEvent::Error(err) => {
                        block.fail(err);
                        state.active_agent = None;
                    }
                }
            }
        }
    }

    // Auto-scroll on agent activity
    state.history_scroll.offset = state.history_scroll.max.get();
    Command::none()
}

// =========================================================================
// Agent spawning
// =========================================================================

fn spawn_agent(state: &mut NexusState, query: String, attachments: Vec<Value>) -> Command<NexusMessage> {
    let is_continuation = state.agent_session_id.is_some();
    let current_cwd = &state.cwd;

    let contextualized_query = if is_continuation {
        format!("[CWD: {}]\n{}", current_cwd, query)
    } else {
        let shell_context = build_shell_context(
            current_cwd,
            &state.blocks,
            &state.shell_history,
        );
        format!("{}{}", shell_context, query)
    };

    let block_id = state.next_id();
    let agent_block = AgentBlock::new(block_id, query.clone());
    let idx = state.agent_blocks.len();
    state.agent_block_index.insert(block_id, idx);
    state.agent_blocks.push(agent_block);
    state.active_agent = Some(block_id);

    // Reset cancel flag
    state.agent_cancel_flag.store(false, Ordering::SeqCst);

    let agent_tx = state.agent_event_tx.clone();
    let cancel_flag = state.agent_cancel_flag.clone();
    let cwd = PathBuf::from(&state.cwd);
    let session_id = state.agent_session_id.clone();

    tokio::spawn(async move {
        match spawn_agent_task(
            agent_tx,
            cancel_flag,
            contextualized_query,
            cwd,
            attachments,
            session_id,
        )
        .await
        {
            Ok(new_session_id) => {
                if let Some(sid) = new_session_id {
                    tracing::info!("Agent session: {}", sid);
                }
            }
            Err(e) => {
                tracing::error!("Agent task failed: {}", e);
            }
        }
    });

    // Mark block as streaming
    if let Some(&idx) = state.agent_block_index.get(&block_id) {
        if let Some(block) = state.agent_blocks.get_mut(idx) {
            block.state = AgentBlockState::Streaming;
        }
    }

    state.history_scroll.offset = state.history_scroll.max.get();
    Command::none()
}

// =========================================================================
// Selection text extraction
// =========================================================================

/// Extract the full visible text from a block targeted by the context menu.
fn extract_block_text(state: &NexusState, target: &ContextTarget) -> Option<String> {
    match target {
        ContextTarget::Block(block_id) => {
            let idx = state.block_index.get(block_id)?;
            let block = state.blocks.get(*idx)?;

            // If the block has native output, convert it to text
            if let Some(ref value) = block.native_output {
                return Some(value.to_text());
            }

            // Otherwise extract from terminal grid
            let grid = if block.parser.is_alternate_screen() || block.is_running() {
                block.parser.grid()
            } else {
                block.parser.grid_with_scrollback()
            };

            let mut lines = Vec::new();
            for row in grid.rows_iter() {
                let text: String = row.iter().map(|cell| cell.c).collect();
                let trimmed = text.trim_end();
                if !trimmed.is_empty() || !lines.is_empty() {
                    lines.push(trimmed.to_string());
                }
            }
            // Trim trailing empty lines
            while lines.last().map_or(false, |l| l.is_empty()) {
                lines.pop();
            }
            if lines.is_empty() {
                None
            } else {
                Some(lines.join("\n"))
            }
        }
        ContextTarget::AgentBlock(block_id) => {
            let idx = state.agent_block_index.get(block_id)?;
            let block = state.agent_blocks.get(*idx)?;

            let mut text = String::new();
            if !block.response.is_empty() {
                text.push_str(&block.response);
            }
            if text.is_empty() && !block.thinking.is_empty() {
                text.push_str(&block.thinking);
            }
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        }
        ContextTarget::Input => {
            let text = &state.input.text;
            if text.is_empty() { None } else { Some(text.clone()) }
        }
    }
}

/// Extract the text content within a selection, supporting cross-source selections.
///
/// Uses source ordering to determine which sources fall within the selection,
/// then extracts the relevant text range from each (terminal grid, native output,
/// or agent response).
fn extract_selected_text(state: &NexusState, sel: &Selection) -> String {
    // Get source ordering from the most recent layout snapshot
    // We rebuild a mini ordering from known sources in document order
    let ordering = build_source_ordering(state);
    let sources = sel.sources(&ordering);

    if sources.is_empty() {
        return String::new();
    }

    let (start, end) = sel.normalized(&ordering);
    let mut parts: Vec<String> = Vec::new();

    for source_id in &sources {
        let is_start = *source_id == start.source_id;
        let is_end = *source_id == end.source_id;

        if let Some(text) = extract_source_text(state, *source_id, is_start, is_end, &start, &end) {
            if !text.is_empty() {
                parts.push(text);
            }
        }
    }

    parts.join("\n")
}

/// Build a source ordering reflecting current document order.
///
/// Sources appear in the order blocks are rendered: shell blocks (terminal or native),
/// then agent blocks. This matches the view() layout pass.
fn build_source_ordering(state: &NexusState) -> crate::strata::content_address::SourceOrdering {
    let mut ordering = crate::strata::content_address::SourceOrdering::new();
    let unified = state.unified_blocks();
    for block_ref in &unified {
        match block_ref {
            UnifiedBlockRef::Shell(block) => {
                // Header (command line) comes first
                ordering.register(source_ids::shell_header(block.id));
                // Then content: native output, table, or terminal
                if let Some(ref value) = block.native_output {
                    if matches!(value, nexus_api::Value::Table { .. }) {
                        ordering.register(source_ids::table(block.id));
                    } else {
                        ordering.register(source_ids::native(block.id));
                    }
                } else {
                    ordering.register(source_ids::shell_term(block.id));
                }
            }
            UnifiedBlockRef::Agent(block) => {
                ordering.register(source_ids::agent_query(block.id));
                if !block.thinking.is_empty() && !block.thinking_collapsed {
                    ordering.register(source_ids::agent_thinking(block.id));
                }
                if !block.response.is_empty() {
                    ordering.register(source_ids::agent_response(block.id));
                }
            }
        }
    }
    ordering
}

/// Extract text from a single source within a selection range.
fn extract_source_text(
    state: &NexusState,
    source_id: SourceId,
    is_start: bool,
    is_end: bool,
    start: &ContentAddress,
    end: &ContentAddress,
) -> Option<String> {
    // Shell blocks
    for block in &state.blocks {
        // Command header
        let header_id = source_ids::shell_header(block.id);
        if header_id == source_id {
            let text = format!("$ {}", block.command);
            let lines: Vec<&str> = text.lines().collect();
            return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
        }

        // Terminal output
        let term_id = source_ids::shell_term(block.id);
        if term_id == source_id && block.native_output.is_none() {
            let grid = if block.parser.is_alternate_screen() || block.is_running() {
                block.parser.grid()
            } else {
                block.parser.grid_with_scrollback()
            };
            let cols = grid.cols() as usize;
            if cols == 0 {
                return Some(String::new());
            }

            let start_offset = if is_start { start.content_offset } else { 0 };
            let total_cells = grid.content_rows() as usize * cols;
            let end_offset = if is_end { end.content_offset } else { total_cells };

            if start_offset >= end_offset {
                return Some(String::new());
            }

            let rows: Vec<Vec<nexus_term::Cell>> = grid.rows_iter().map(|r| r.to_vec()).collect();
            return Some(extract_grid_range(&rows, cols, start_offset, end_offset));
        }

        // Native output (multi-item: each line is a separate item)
        let native_id = source_ids::native(block.id);
        if native_id == source_id {
            if let Some(ref value) = block.native_output {
                let full_text = value.to_text();
                let lines: Vec<&str> = full_text.lines().collect();
                return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
            }
        }

        // Table output (multi-item: headers first, then cell texts row-major)
        let table_id = source_ids::table(block.id);
        if table_id == source_id {
            if let Some(nexus_api::Value::Table { columns, rows }) = &block.native_output {
                let mut lines: Vec<String> = Vec::new();
                // Headers come first (matching render order)
                for col in columns {
                    lines.push(col.name.clone());
                }
                // Then data cells row-major
                for row in rows {
                    for cell in row {
                        let text = cell.to_text();
                        for l in text.lines() {
                            lines.push(l.to_string());
                        }
                    }
                }
                let line_refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
                return Some(extract_multi_item_range(&line_refs, is_start, is_end, start, end));
            }
        }
    }

    // Agent blocks
    for block in &state.agent_blocks {
        // Query
        let query_id = source_ids::agent_query(block.id);
        if query_id == source_id {
            // Query renders as "? " + query text (two items in a Row)
            let lines: Vec<&str> = vec!["?", &block.query];
            return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
        }

        // Thinking
        let thinking_id = source_ids::agent_thinking(block.id);
        if thinking_id == source_id {
            let preview = if block.thinking.len() > 500 {
                format!("{}...", &block.thinking[..500])
            } else {
                block.thinking.clone()
            };
            let lines: Vec<&str> = preview.lines().collect();
            return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
        }

        // Response
        let response_id = source_ids::agent_response(block.id);
        if response_id == source_id {
            if block.response.is_empty() {
                return None;
            }
            // Response is rendered line-by-line, each as a multi-item source
            let lines: Vec<&str> = block.response.lines().collect();
            return Some(extract_multi_item_range(&lines, is_start, is_end, start, end));
        }
    }

    None
}

/// Extract a range of characters from a terminal grid.
fn extract_grid_range(rows: &[Vec<nexus_term::Cell>], cols: usize, start: usize, end: usize) -> String {
    let start_row = start / cols;
    let start_col = start % cols;
    let end_row = end / cols;
    let end_col = end % cols;

    let mut result = String::new();
    for row_idx in start_row..=end_row {
        if row_idx >= rows.len() {
            break;
        }
        let row = &rows[row_idx];
        let col_start = if row_idx == start_row { start_col } else { 0 };
        let col_end = if row_idx == end_row { end_col } else { row.len() };

        let line: String = row.iter()
            .skip(col_start)
            .take(col_end.saturating_sub(col_start))
            .map(|cell| cell.c)
            .collect();

        result.push_str(line.trim_end());
        if row_idx < end_row {
            result.push('\n');
        }
    }
    result
}

/// Extract text from a multi-item source (each line is a separate item).
///
/// `item_index` identifies the line, `content_offset` the character within that line.
fn extract_multi_item_range(
    lines: &[&str],
    is_start: bool,
    is_end: bool,
    start: &ContentAddress,
    end: &ContentAddress,
) -> String {
    if lines.is_empty() {
        return String::new();
    }

    let start_item = if is_start { start.item_index } else { 0 };
    let end_item = if is_end { end.item_index } else { lines.len().saturating_sub(1) };

    if start_item > end_item || start_item >= lines.len() {
        return String::new();
    }

    let mut parts: Vec<String> = Vec::new();
    for i in start_item..=end_item.min(lines.len() - 1) {
        let line = lines[i];
        let chars: Vec<char> = line.chars().collect();
        let from = if i == start_item && is_start { start.content_offset.min(chars.len()) } else { 0 };
        let to = if i == end_item && is_end { end.content_offset.min(chars.len()) } else { chars.len() };
        if from <= to {
            parts.push(chars[from..to].iter().collect());
        }
    }
    parts.join("\n")
}

// =========================================================================
// Context menu rendering
// =========================================================================

fn render_context_menu(snapshot: &mut LayoutSnapshot, menu: &ContextMenuState) {
    use crate::strata::primitives::{Color, Point, Rect};

    let w = 200.0_f32;
    let row_h = 30.0_f32;
    let padding = 6.0_f32;
    let h = menu.items.len() as f32 * row_h + padding * 2.0;

    // Clamp position to stay within viewport
    let vp = snapshot.viewport();
    let x = menu.x.min(vp.width - w - 4.0).max(0.0);
    let y = menu.y.min(vp.height - h - 4.0).max(0.0);

    let p = snapshot.overlay_primitives_mut();

    // Shadow
    p.add_shadow(
        Rect::new(x + 3.0, y + 3.0, w, h),
        8.0, 16.0,
        Color::rgba(0.0, 0.0, 0.0, 0.7),
    );
    // Background — dark opaque
    p.add_rounded_rect(Rect::new(x, y, w, h), 8.0, Color::rgb(0.08, 0.08, 0.10));
    // Border
    p.add_border(Rect::new(x, y, w, h), 8.0, 1.0, Color::rgba(1.0, 1.0, 1.0, 0.15));

    let ix = x + padding;
    let iw = w - padding * 2.0;

    let hovered = menu.hovered_item.get();

    for (i, item) in menu.items.iter().enumerate() {
        let iy = y + padding + i as f32 * row_h;
        let item_rect = Rect::new(ix, iy, iw, row_h - 2.0);

        // Register as clickable widget
        let item_id = SourceId::named(&format!("ctx_menu_{}", i));
        snapshot.register_widget(item_id, item_rect);

        let p = snapshot.overlay_primitives_mut();

        // Item background — highlight on hover
        let bg = if hovered == Some(i) {
            Color::rgb(0.25, 0.35, 0.55)
        } else {
            Color::rgb(0.15, 0.15, 0.18)
        };
        p.add_rounded_rect(item_rect, 4.0, bg);

        // Label
        p.add_text(item.label(), Point::new(ix + 10.0, iy + 6.0), Color::rgb(0.92, 0.92, 0.92));

        // Shortcut hint (right-aligned)
        let shortcut = item.shortcut();
        if !shortcut.is_empty() {
            p.add_text(shortcut, Point::new(ix + iw - 36.0, iy + 6.0), Color::rgb(0.45, 0.45, 0.5));
        }
    }
}

// =========================================================================
// Key-to-bytes conversion for PTY input
// =========================================================================

/// Convert a Strata KeyEvent to bytes suitable for writing to a PTY.
fn strata_key_to_bytes(event: &KeyEvent) -> Option<Vec<u8>> {
    let (key, modifiers, text) = match event {
        KeyEvent::Pressed { key, modifiers, text } => (key, modifiers, text.as_deref()),
        KeyEvent::Released { .. } => return None,
    };

    match key {
        Key::Character(c) => {
            if modifiers.ctrl && c.len() == 1 {
                // Ctrl+letter = ASCII 1-26
                let ch = c.chars().next()?;
                if ch.is_ascii_alphabetic() {
                    let ctrl_code = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
                    return Some(vec![ctrl_code]);
                }
            }
            // Use OS-provided text if available (handles shift correctly)
            if let Some(t) = text {
                if !t.is_empty() {
                    return Some(t.as_bytes().to_vec());
                }
            }
            Some(c.as_bytes().to_vec())
        }
        Key::Named(named) => {
            if modifiers.ctrl {
                match named {
                    NamedKey::ArrowLeft => return Some(vec![0x1b, b'[', b'1', b';', b'5', b'D']),
                    NamedKey::ArrowRight => return Some(vec![0x1b, b'[', b'1', b';', b'5', b'C']),
                    NamedKey::ArrowUp => return Some(vec![0x1b, b'[', b'1', b';', b'5', b'A']),
                    NamedKey::ArrowDown => return Some(vec![0x1b, b'[', b'1', b';', b'5', b'B']),
                    _ => {}
                }
            }
            if modifiers.shift {
                match named {
                    NamedKey::ArrowLeft => return Some(vec![0x1b, b'[', b'1', b';', b'2', b'D']),
                    NamedKey::ArrowRight => return Some(vec![0x1b, b'[', b'1', b';', b'2', b'C']),
                    NamedKey::ArrowUp => return Some(vec![0x1b, b'[', b'1', b';', b'2', b'A']),
                    NamedKey::ArrowDown => return Some(vec![0x1b, b'[', b'1', b';', b'2', b'B']),
                    _ => {}
                }
            }
            if modifiers.alt {
                match named {
                    NamedKey::ArrowLeft => return Some(vec![0x1b, b'[', b'1', b';', b'3', b'D']),
                    NamedKey::ArrowRight => return Some(vec![0x1b, b'[', b'1', b';', b'3', b'C']),
                    NamedKey::ArrowUp => return Some(vec![0x1b, b'[', b'1', b';', b'3', b'A']),
                    NamedKey::ArrowDown => return Some(vec![0x1b, b'[', b'1', b';', b'3', b'B']),
                    _ => {}
                }
            }
            match named {
                NamedKey::Enter => Some(vec![b'\r']),
                NamedKey::Backspace => Some(vec![0x7f]),
                NamedKey::Tab => Some(vec![b'\t']),
                NamedKey::Escape => Some(vec![0x1b]),
                NamedKey::Space => Some(vec![b' ']),
                NamedKey::ArrowUp => Some(vec![0x1b, b'[', b'A']),
                NamedKey::ArrowDown => Some(vec![0x1b, b'[', b'B']),
                NamedKey::ArrowRight => Some(vec![0x1b, b'[', b'C']),
                NamedKey::ArrowLeft => Some(vec![0x1b, b'[', b'D']),
                NamedKey::Home => Some(vec![0x1b, b'[', b'H']),
                NamedKey::End => Some(vec![0x1b, b'[', b'F']),
                NamedKey::PageUp => Some(vec![0x1b, b'[', b'5', b'~']),
                NamedKey::PageDown => Some(vec![0x1b, b'[', b'6', b'~']),
                NamedKey::Delete => Some(vec![0x1b, b'[', b'3', b'~']),
                _ => None,
            }
        }
    }
}

// =========================================================================
// Scroll helpers
// =========================================================================

/// Scroll a `ScrollState` so that the item at `index` is visible within a
/// viewport of `viewport_height`.  Each item is assumed to be `item_height`
/// pixels tall.
fn scroll_to_index(scroll: &mut ScrollState, index: usize, item_height: f32, viewport_height: f32) {
    let item_top = index as f32 * item_height;
    let item_bottom = item_top + item_height;
    if item_top < scroll.offset {
        scroll.offset = item_top;
    } else if item_bottom > scroll.offset + viewport_height {
        scroll.offset = item_bottom - viewport_height;
    }
}

// =========================================================================
// Entry point
// =========================================================================

pub fn run() -> Result<(), crate::strata::shell::Error> {
    crate::strata::shell::run_with_config::<NexusApp>(AppConfig {
        title: String::from("Nexus (Strata)"),
        window_size: (1200.0, 800.0),
        antialiasing: true,
        background_color: colors::BG_APP,
    })
}
