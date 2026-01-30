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
    AppConfig, Column, Command, ImageStore, LayoutSnapshot, Length,
    MouseResponse, Padding, ScrollAction, ScrollColumn, ScrollState, Selection, StrataApp,
    Subscription, TextInputAction, TextInputMouseAction, TextInputState,
};
use crate::systems::{agent_subscription, kernel_subscription, pty_subscription, spawn_agent_task};
use crate::widgets::job_indicator::{VisualJob, VisualJobState};

// =========================================================================
// Color palette (matches real Nexus app)
// =========================================================================
pub(crate) mod colors {
    use crate::strata::primitives::Color;

    // Backgrounds (matched from theme.rs BG_PRIMARY/SECONDARY/TERTIARY + view/mod.rs)
    pub const BG_APP: Color = Color { r: 0.07, g: 0.07, b: 0.09, a: 1.0 };
    pub const BG_BLOCK: Color = Color { r: 0.12, g: 0.12, b: 0.14, a: 1.0 };
    pub const BG_INPUT: Color = Color { r: 0.1, g: 0.1, b: 0.12, a: 1.0 };
    pub const BG_CARD: Color = Color { r: 0.16, g: 0.16, b: 0.18, a: 1.0 };

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
    pub const BORDER_SUBTLE: Color = Color { r: 0.2, g: 0.2, b: 0.22, a: 1.0 };
    pub const BORDER_INPUT: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.08 };

    // Welcome screen (matched from app/view/welcome.rs)
    pub const WELCOME_TITLE: Color = Color { r: 0.6, g: 0.8, b: 0.6, a: 1.0 };
    pub const WELCOME_HEADING: Color = Color { r: 0.8, g: 0.7, b: 0.5, a: 1.0 };
    pub const CARD_BG: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.03 };
    pub const CARD_BORDER: Color = Color { r: 1.0, g: 1.0, b: 1.0, a: 0.06 };

    // Cursor
    pub const CURSOR: Color = Color { r: 0.9, g: 0.9, b: 0.9, a: 0.8 };
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

    // Window
    ClearScreen,
    CloseWindow,
    BlurAll,
    Tick,
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

    // --- History search ---
    pub history_search_active: bool,
    pub history_search_query: String,
    pub history_search_results: Vec<String>,
    pub history_search_index: usize,

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

            history_search_active: false,
            history_search_query: String::new(),
            history_search_results: Vec::new(),
            history_search_index: 0,

            exit_requested: false,

            context,
        };

        (state, Command::none())
    }

    fn update(
        state: &mut Self::State,
        message: Self::Message,
        _images: &mut ImageStore,
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
                    return spawn_agent(state, query);
                } else {
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
                return handle_kernel_event(state, evt);
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
                // TODO: extract selected text and copy to clipboard
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
                }
            }
            NexusMessage::CompletionNav(delta) => {
                if !state.completions.is_empty() {
                    let len = state.completions.len() as isize;
                    let current = state.completion_index.unwrap_or(0) as isize;
                    let new_idx = ((current + delta) % len + len) % len;
                    state.completion_index = Some(new_idx as usize);
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

            // =============================================================
            // History search (Ctrl+R)
            // =============================================================
            NexusMessage::HistorySearchToggle => {
                if state.history_search_active {
                    // Cycle to next result
                    if !state.history_search_results.is_empty() {
                        state.history_search_index = (state.history_search_index + 1) % state.history_search_results.len();
                    }
                } else {
                    state.history_search_active = true;
                    state.history_search_query.clear();
                    state.history_search_results.clear();
                    state.history_search_index = 0;
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
                        let kill_id = SourceId::named(&format!("kill_{}", block.id.0));
                        scroll = scroll.push(ShellBlockWidget {
                            block,
                            kill_id,
                        });
                    }
                    UnifiedBlockRef::Agent(block) => {
                        let thinking_id = SourceId::named(&format!("thinking_{}", block.id.0));
                        let stop_id = SourceId::named(&format!("stop_{}", block.id.0));
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
            });
        }

        // History search bar (replaces input bar when active)
        if state.history_search_active {
            let current_match = state.history_search_results
                .get(state.history_search_index)
                .map(|s| s.as_str())
                .unwrap_or("");
            main_col = main_col.push(HistorySearchBar {
                query: &state.history_search_query,
                current_match,
                result_count: state.history_search_results.len(),
                result_index: state.history_search_index,
            });
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
                }),
        );

        main_col.layout(snapshot, Rect::new(0.0, 0.0, vw, vh));

        // Sync layout state
        state.history_scroll.sync_from_snapshot(snapshot);
        state.input.sync_from_snapshot(snapshot);
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
            state.history_scroll => NexusMessage::HistoryScroll,
            state.input          => NexusMessage::InputMouse,
        ]);

        // Button clicks
        if let MouseEvent::ButtonPressed { button: MouseButton::Left, .. } = &event {
            if let Some(HitResult::Widget(id)) = &hit {
                // Mode toggle
                if *id == SourceId::named("mode_toggle") {
                    return MouseResponse::message(NexusMessage::ToggleMode);
                }

                // Kill buttons
                for block in &state.blocks {
                    if block.is_running() {
                        let kill_id = SourceId::named(&format!("kill_{}", block.id.0));
                        if *id == kill_id {
                            return MouseResponse::message(NexusMessage::KillBlock(block.id));
                        }
                    }
                }

                // Agent thinking toggles
                for block in &state.agent_blocks {
                    let thinking_id = SourceId::named(&format!("thinking_{}", block.id.0));
                    if *id == thinking_id {
                        return MouseResponse::message(NexusMessage::ToggleThinking(block.id));
                    }

                    // Stop button
                    let stop_id = SourceId::named(&format!("stop_{}", block.id.0));
                    if *id == stop_id {
                        return MouseResponse::message(NexusMessage::AgentInterrupt);
                    }

                    // Tool toggles
                    for (i, _tool) in block.tools.iter().enumerate() {
                        let toggle_id = SourceId::named(&format!("tool_toggle_{}_{}", block.id.0, i));
                        if *id == toggle_id {
                            return MouseResponse::message(NexusMessage::ToggleTool(block.id, i));
                        }
                    }

                    // Permission buttons
                    if let Some(ref perm) = block.pending_permission {
                        let deny_id = SourceId::named(&format!("perm_deny_{}", block.id.0));
                        let allow_id = SourceId::named(&format!("perm_allow_{}", block.id.0));
                        let always_id = SourceId::named(&format!("perm_always_{}", block.id.0));

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
                            let sort_id = SourceId::named(&format!("sort_{}_{}", block.id.0, col_idx));
                            if *id == sort_id {
                                return MouseResponse::message(NexusMessage::SortTable(block.id, col_idx));
                            }
                        }
                    }
                }

                // Content selection start (clicked text)
            }

            // Text content selection
            if let Some(HitResult::Content(addr)) = hit {
                if state.input.focused {
                    return MouseResponse::message(NexusMessage::BlurAll);
                }
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
                        "k" => return Some(NexusMessage::ClearScreen),
                        "w" => return Some(NexusMessage::CloseWindow),
                        "c" => return Some(NexusMessage::Copy),
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

            // Escape: interrupt agent, leave PTY focus, or clear selection
            if matches!(key, Key::Named(NamedKey::Escape)) {
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
                // Tab → trigger completion
                if matches!(key, Key::Named(NamedKey::Tab)) {
                    return Some(NexusMessage::TabComplete);
                }

                // Arrow Up/Down → history navigation
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

fn handle_kernel_event(state: &mut NexusState, evt: ShellEvent) -> Command<NexusMessage> {
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

fn spawn_agent(state: &mut NexusState, query: String) -> Command<NexusMessage> {
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
            Vec::new(), // No attachments in V1
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
