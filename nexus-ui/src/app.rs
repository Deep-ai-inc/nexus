//! Main Nexus application using Iced's Elm architecture.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use iced::futures::stream;
use iced::keyboard::{self, Key, Modifiers};
use iced::widget::{column, container, row, scrollable, text, text_input, Column};
use iced::{event, Element, Event, Length, Subscription, Task, Theme};
use tokio::sync::{broadcast, mpsc, Mutex};

use nexus_api::{BlockId, BlockState, OutputFormat, ShellEvent, Value};
use nexus_kernel::{CommandRegistry, Completion, CompletionKind, HistoryEntry, Kernel};
use nexus_term::TerminalParser;

use crate::agent_adapter::{AgentEvent, IcedAgentUI, PermissionResponse};
use crate::agent_block::{AgentBlock, AgentBlockState, PermissionRequest, ToolStatus};
use crate::agent_widgets::{view_agent_block, AgentWidgetMessage};
use crate::glyph_cache::get_cell_metrics;
use crate::pty::PtyHandle;
use crate::shell_context::build_shell_context;
use crate::widgets::job_indicator::{job_status_bar, VisualJob, VisualJobState};
use crate::widgets::table::{interactive_table, TableSort};
use crate::widgets::terminal_shader::TerminalShader;

// ============================================================================
// Terminal rendering constants - single source of truth
// ============================================================================

/// Default font size for terminal text.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;
/// Line height multiplier.
pub const LINE_HEIGHT_FACTOR: f32 = 1.4;
/// Character width ratio relative to font size.
pub const CHAR_WIDTH_RATIO: f32 = 0.607; // ~8.5/14.0, conservative for anti-aliasing

/// Scrollable ID for auto-scrolling.
const HISTORY_SCROLLABLE: &str = "history";

/// Unified block type - either a shell command or agent conversation.
#[derive(Debug)]
pub enum UnifiedBlock {
    Shell(Block),
    Agent(AgentBlock),
}

impl UnifiedBlock {
    /// Get the block ID for ordering.
    pub fn id(&self) -> BlockId {
        match self {
            UnifiedBlock::Shell(b) => b.id,
            UnifiedBlock::Agent(b) => b.id,
        }
    }

    /// Check if the block is still running/active.
    pub fn is_running(&self) -> bool {
        match self {
            UnifiedBlock::Shell(b) => b.is_running(),
            UnifiedBlock::Agent(b) => b.is_running(),
        }
    }
}

/// Reference to a unified block for view rendering (avoids cloning).
enum UnifiedBlockRef<'a> {
    Shell(&'a Block),
    Agent(&'a AgentBlock),
}

/// A command block containing input and output.
#[derive(Debug)]
pub struct Block {
    pub id: BlockId,
    pub command: String,
    pub parser: TerminalParser,
    pub state: BlockState,
    #[allow(dead_code)]
    pub format: OutputFormat,
    pub collapsed: bool,
    pub started_at: Instant,
    pub duration_ms: Option<u64>,
    /// Version counter for lazy invalidation.
    pub version: u64,
    /// Native command output (structured data, not terminal output).
    pub native_output: Option<Value>,
    /// Sort state for table output.
    pub table_sort: TableSort,
    /// Whether output contained "permission denied".
    pub has_permission_denied: bool,
    /// Whether output contained "command not found".
    pub has_command_not_found: bool,
}

impl Block {
    fn new(id: BlockId, command: String) -> Self {
        Self {
            id,
            command,
            parser: TerminalParser::new(120, 24),
            state: BlockState::Running,
            format: OutputFormat::PlainText,
            collapsed: false,
            started_at: Instant::now(),
            duration_ms: None,
            version: 0,
            native_output: None,
            table_sort: TableSort::new(),
            has_permission_denied: false,
            has_command_not_found: false,
        }
    }

    fn is_running(&self) -> bool {
        matches!(self.state, BlockState::Running)
    }
}

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        // Different blocks are never equal
        if self.id != other.id {
            return false;
        }

        // Running blocks always need redrawing (cursor, new output)
        if self.is_running() {
            return false;
        }

        // Finished blocks: check if anything visual changed
        self.version == other.version
            && self.collapsed == other.collapsed
            && self.parser.size() == other.parser.size()
    }
}

/// Focus state - makes illegal states unrepresentable.
#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    /// The command input field is focused.
    Input,
    /// A specific block is focused for interaction.
    Block(BlockId),
}

/// Input mode - determines how commands are processed.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum InputMode {
    /// Normal shell mode - commands are executed by the kernel.
    #[default]
    Shell,
    /// Agent mode - input is sent to the AI agent.
    Agent,
}

/// Messages for the Nexus application.
#[derive(Debug, Clone)]
pub enum Message {
    /// Input text changed.
    InputChanged(String),
    /// User submitted a command.
    Submit,
    /// PTY output received.
    PtyOutput(BlockId, Vec<u8>),
    /// PTY exited.
    PtyExited(BlockId, i32),
    /// Keyboard event when a block is focused.
    KeyPressed(Key, Modifiers),
    /// Generic event (for subscription) with window ID.
    Event(Event, iced::window::Id),
    /// Window resized.
    WindowResized(u32, u32),
    /// Global keyboard shortcut (Cmd+K, Cmd+Q, etc.)
    GlobalShortcut(GlobalShortcut),
    /// Input-specific key event (Up/Down for history)
    InputKey(Key, Modifiers),
    /// Zoom font size
    Zoom(ZoomDirection),
    /// VSync-aligned frame for batched rendering.
    /// Fires when the monitor is ready for the next frame.
    NextFrame(Instant),
    /// Kernel event (from pipeline execution).
    KernelEvent(ShellEvent),
    /// Sort table by column in a specific block.
    TableSort(BlockId, usize),
    /// User clicked a cell in a table (for semantic actions).
    TableCellClick(BlockId, usize, usize, Value),
    /// User clicked a job in the status bar.
    JobClicked(u32),
    /// Tab key pressed - trigger completion.
    TabCompletion,
    /// Select a completion item.
    SelectCompletion(usize),
    /// Cancel completion popup.
    CancelCompletion,
    /// Ctrl+R pressed - open history search.
    HistorySearchStart,
    /// History search query changed.
    HistorySearchChanged(String),
    /// Select a history search result.
    HistorySearchSelect(usize),
    /// Cancel history search.
    HistorySearchCancel,
    /// Retry last command with sudo (permission denied recovery).
    RetryWithSudo,
    /// Dismiss the permission denied prompt.
    DismissPermissionPrompt,
    /// Run a suggested command (command not found recovery).
    RunSuggestedCommand(String),
    /// Dismiss the command not found prompt.
    DismissCommandNotFound,
    /// Toggle between Shell and Agent input modes.
    ToggleInputMode,
    /// An image was pasted from clipboard (Mathematica-style rich input).
    ImagePasted(Vec<u8>, u32, u32), // (data, width, height)
    /// Remove an attachment from the input.
    RemoveAttachment(usize),
    /// Agent event received from the agent adapter.
    AgentEvent(AgentEvent),
    /// Agent widget interaction (toggle, permission response, etc.)
    AgentWidget(AgentWidgetMessage),
    /// Cancel the current agent operation.
    CancelAgent,
}

/// Global keyboard shortcuts.
#[derive(Debug, Clone)]
pub enum GlobalShortcut {
    /// Cmd+K - Clear screen
    ClearScreen,
    /// Cmd+W - Close window
    CloseWindow,
    /// Cmd+Q - Quit application
    Quit,
    /// Cmd+C - Copy
    Copy,
    /// Cmd+V - Paste
    Paste,
}

/// Zoom direction for font size changes.
#[derive(Debug, Clone)]
pub enum ZoomDirection {
    /// Cmd++ - Increase font size
    In,
    /// Cmd+- - Decrease font size
    Out,
    /// Cmd+0 - Reset to default
    Reset,
}

/// The main Nexus application state.
pub struct Nexus {
    /// Current input text.
    input: String,
    /// Command blocks (ordered).
    blocks: Vec<Block>,
    /// Block index by ID for O(1) lookup.
    block_index: HashMap<BlockId, usize>,
    /// Next block ID.
    next_block_id: u64,
    /// Current working directory.
    cwd: String,
    /// Active PTY handles.
    pty_handles: Vec<PtyHandle>,
    /// Channel for PTY output.
    pty_tx: mpsc::UnboundedSender<(BlockId, PtyEvent)>,
    /// Receiver for PTY events (shared with subscription).
    pty_rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
    /// Current focus state.
    focus: Focus,
    /// Terminal dimensions (cols, rows).
    terminal_size: (u16, u16),
    /// Window dimensions in pixels (for recalculating on zoom).
    window_dims: (f32, f32),
    /// Current font size (mutable for zoom).
    font_size: f32,
    /// Command history for Up/Down navigation.
    command_history: Vec<String>,
    /// Current position in history (None = new command being typed).
    history_index: Option<usize>,
    /// Saved input when browsing history (to restore on Down).
    saved_input: String,
    /// Exit code of last command (for prompt color).
    last_exit_code: Option<i32>,
    /// Window ID for resize operations.
    window_id: Option<iced::window::Id>,
    /// Suppress next input change (after Cmd shortcut to prevent typing).
    suppress_next_input: bool,
    /// Input value before current event (to detect ghost characters).
    input_before_event: String,
    /// Is there processed PTY data that hasn't been drawn yet?
    /// Used for 60 FPS throttling during high-throughput output.
    is_dirty: bool,
    /// Registry of native (in-process) commands.
    commands: CommandRegistry,
    /// Kernel for pipeline execution.
    kernel: Arc<Mutex<Kernel>>,
    /// Receiver for kernel events (shared with subscription).
    kernel_rx: Arc<Mutex<broadcast::Receiver<ShellEvent>>>,
    /// Visual jobs for the status bar.
    visual_jobs: Vec<VisualJob>,
    /// Tab completion state.
    completions: Vec<Completion>,
    /// Selected completion index.
    completion_index: usize,
    /// Start position of the word being completed.
    completion_start: usize,
    /// Whether completion popup is visible.
    completion_visible: bool,
    /// History search active (Ctrl+R mode).
    history_search_active: bool,
    /// History search query.
    history_search_query: String,
    /// History search results.
    history_search_results: Vec<HistoryEntry>,
    /// Selected history search index.
    history_search_index: usize,
    /// Command that failed with permission denied (for sudo retry).
    permission_denied_command: Option<String>,
    /// Command not found info (original command, suggestions).
    command_not_found: Option<(String, Vec<String>)>,
    /// Current input mode (Shell or Agent).
    input_mode: InputMode,
    /// Pending attachments (images/files) for the current command (Mathematica-style).
    attachments: Vec<nexus_api::Value>,
    /// Agent conversation blocks.
    agent_blocks: Vec<AgentBlock>,
    /// Agent block index by ID for O(1) lookup.
    agent_block_index: HashMap<BlockId, usize>,
    /// Currently active agent block (receiving events).
    active_agent_block: Option<BlockId>,
    /// Channel for agent events (receiver shared with subscription).
    agent_rx: Arc<Mutex<mpsc::UnboundedReceiver<AgentEvent>>>,
    /// Channel for agent events (sender given to agent adapter).
    agent_tx: mpsc::UnboundedSender<AgentEvent>,
    /// Channel for permission responses.
    permission_tx: Option<mpsc::UnboundedSender<(String, PermissionResponse)>>,
    /// Cancel flag for agent tasks.
    agent_cancel: Arc<AtomicBool>,
}

#[derive(Debug, Clone)]
pub enum PtyEvent {
    Output(Vec<u8>),
    Exited(i32),
}

impl Nexus {
    /// Get current character width based on font size.
    fn char_width(&self) -> f32 {
        self.font_size * CHAR_WIDTH_RATIO
    }

    /// Get current line height based on font size.
    fn line_height(&self) -> f32 {
        self.font_size * LINE_HEIGHT_FACTOR
    }

    /// Recalculate terminal dimensions based on window size and font metrics.
    /// Returns (cols, rows) and updates terminal_size.
    fn recalculate_terminal_size(&mut self) -> (u16, u16) {
        let h_padding = 30.0; // Left + right padding
        let v_padding = 80.0; // Top/bottom padding + input area

        let (width, height) = self.window_dims;
        let cols = ((width - h_padding) / self.char_width()) as u16;
        let rows = ((height - v_padding) / self.line_height()) as u16;

        // Clamp to reasonable ranges
        let cols = cols.max(40).min(500);
        let rows = rows.max(5).min(200);

        self.terminal_size = (cols, rows);
        (cols, rows)
    }

    /// Apply terminal resize to all blocks and PTYs.
    fn apply_resize(&mut self, cols: u16, rows: u16) {
        for block in &mut self.blocks {
            if block.is_running() {
                block.parser.resize(cols, rows);
            } else {
                // Finished blocks: two-step resize for reflow
                let (_, current_rows) = block.parser.size();
                block.parser.resize(cols, current_rows);
                let needed_rows = block.parser.content_height() as u16;
                block.parser.resize(cols, needed_rows.max(1));
            }
        }

        // Resize all active PTYs
        for handle in &self.pty_handles {
            let _ = handle.resize(cols, rows);
        }
    }
}

impl Default for Nexus {
    fn default() -> Self {
        let (pty_tx, pty_rx) = mpsc::unbounded_channel();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~".to_string());

        // Create kernel for pipeline execution
        let (kernel, kernel_rx) = Kernel::new().expect("Failed to create kernel");

        // Create agent event channel
        let (agent_tx, agent_rx) = mpsc::unbounded_channel();

        // Load command history from SQLite (kernel's store)
        let command_history = kernel
            .store()
            .and_then(|store| store.get_recent_history(1000).ok())
            .map(|entries| {
                entries
                    .into_iter()
                    .rev() // Oldest first for up-arrow navigation
                    .map(|e| e.command)
                    .collect()
            })
            .unwrap_or_default();

        Self {
            input: String::new(),
            blocks: Vec::new(),
            block_index: HashMap::new(),
            next_block_id: 1,
            cwd,
            pty_handles: Vec::new(),
            pty_tx,
            pty_rx: Arc::new(Mutex::new(pty_rx)),
            focus: Focus::Input,
            terminal_size: (120, 24),
            window_dims: (1200.0, 800.0), // Match initial window size
            font_size: DEFAULT_FONT_SIZE,
            command_history,
            history_index: None,
            saved_input: String::new(),
            last_exit_code: None,
            window_id: None,
            suppress_next_input: false,
            input_before_event: String::new(),
            is_dirty: false,
            commands: CommandRegistry::new(),
            kernel: Arc::new(Mutex::new(kernel)),
            kernel_rx: Arc::new(Mutex::new(kernel_rx)),
            visual_jobs: Vec::new(),
            completions: Vec::new(),
            completion_index: 0,
            completion_start: 0,
            completion_visible: false,
            history_search_active: false,
            history_search_query: String::new(),
            history_search_results: Vec::new(),
            history_search_index: 0,
            permission_denied_command: None,
            command_not_found: None,
            input_mode: InputMode::default(),
            attachments: Vec::new(),
            agent_blocks: Vec::new(),
            agent_block_index: HashMap::new(),
            active_agent_block: None,
            agent_rx: Arc::new(Mutex::new(agent_rx)),
            agent_tx,
            permission_tx: None,
            agent_cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Run the Nexus application.
pub fn run() -> iced::Result {
    iced::application("Nexus", update, view)
        .subscription(subscription)
        .theme(|_| Theme::Dark)
        .window_size(iced::Size::new(1200.0, 800.0))
        .antialiasing(true)
        .run()
}

fn update(state: &mut Nexus, message: Message) -> Task<Message> {
    match message {
        Message::TabCompletion => {
            // Get completions from kernel
            let kernel = state.kernel.blocking_lock();
            let cursor = state.input.len(); // Assume cursor at end
            let (completions, start) = kernel.complete(&state.input, cursor);
            drop(kernel);

            if completions.len() == 1 {
                // Single completion: apply immediately
                let completion = &completions[0];
                state.input = format!("{}{}", &state.input[..start], completion.text);
                state.completion_visible = false;
            } else if !completions.is_empty() {
                // Multiple completions: show popup
                state.completions = completions;
                state.completion_index = 0;
                state.completion_start = start;
                state.completion_visible = true;
            }
            return Task::none();
        }
        Message::SelectCompletion(index) => {
            if let Some(completion) = state.completions.get(index) {
                state.input = format!("{}{}", &state.input[..state.completion_start], completion.text);
            }
            state.completions.clear();
            state.completion_visible = false;
            return Task::none();
        }
        Message::CancelCompletion => {
            state.completions.clear();
            state.completion_visible = false;
            return Task::none();
        }
        Message::HistorySearchStart => {
            // Start history search mode with recent history
            state.history_search_active = true;
            state.history_search_query.clear();
            state.history_search_index = 0;
            // Load recent history initially
            let kernel = state.kernel.blocking_lock();
            state.history_search_results = kernel.get_recent_history(50);
            drop(kernel);
            return Task::none();
        }
        Message::HistorySearchChanged(query) => {
            state.history_search_query = query.clone();
            state.history_search_index = 0;
            // Search history
            let kernel = state.kernel.blocking_lock();
            if query.is_empty() {
                state.history_search_results = kernel.get_recent_history(50);
            } else {
                // FTS5 requires proper quoting for search terms
                let search_query = format!("\"{}\"*", query.replace('"', "\"\""));
                state.history_search_results = kernel.search_history(&search_query, 50);
            }
            drop(kernel);
            return Task::none();
        }
        Message::HistorySearchSelect(index) => {
            if let Some(entry) = state.history_search_results.get(index) {
                state.input = entry.command.clone();
            }
            state.history_search_active = false;
            state.history_search_query.clear();
            state.history_search_results.clear();
            return Task::none();
        }
        Message::HistorySearchCancel => {
            state.history_search_active = false;
            state.history_search_query.clear();
            state.history_search_results.clear();
            return Task::none();
        }
        Message::RetryWithSudo => {
            if let Some(cmd) = state.permission_denied_command.take() {
                let sudo_cmd = format!("sudo {}", cmd);
                return execute_command(state, sudo_cmd);
            }
            return Task::none();
        }
        Message::DismissPermissionPrompt => {
            state.permission_denied_command = None;
            return Task::none();
        }
        Message::RunSuggestedCommand(cmd) => {
            state.command_not_found = None;
            return execute_command(state, cmd);
        }
        Message::DismissCommandNotFound => {
            state.command_not_found = None;
            return Task::none();
        }
        Message::ToggleInputMode => {
            state.input_mode = match state.input_mode {
                InputMode::Shell => InputMode::Agent,
                InputMode::Agent => InputMode::Shell,
            };
            return Task::none();
        }
        Message::ImagePasted(data, width, height) => {
            // Add the pasted image as an attachment (Mathematica-style rich input)
            let metadata = nexus_api::MediaMetadata {
                width: Some(width),
                height: Some(height),
                duration_secs: None,
                filename: None,
                size: Some(data.len() as u64),
            };
            state.attachments.push(nexus_api::Value::Media {
                data,
                content_type: "image/png".to_string(),
                metadata,
            });
            return Task::none();
        }
        Message::RemoveAttachment(index) => {
            if index < state.attachments.len() {
                state.attachments.remove(index);
            }
            return Task::none();
        }
        Message::AgentEvent(event) => {
            // Mark dirty to ensure UI updates
            state.is_dirty = true;

            // Handle events from the agent adapter
            if let Some(block_id) = state.active_agent_block {
                if let Some(idx) = state.agent_block_index.get(&block_id) {
                    if let Some(block) = state.agent_blocks.get_mut(*idx) {
                        match event {
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
                            AgentEvent::ToolEnded { .. } => {
                                // Tool ended, wait for status update
                            }
                            AgentEvent::ToolStatus { id, status, message, output } => {
                                block.update_tool_status(&id, status, message, output);
                            }
                            AgentEvent::ImageAdded { media_type, data } => {
                                block.add_image(media_type, data);
                            }
                            AgentEvent::PermissionRequested {
                                id,
                                tool_name,
                                tool_id,
                                description,
                                action,
                                working_dir,
                            } => {
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
                                // Keep active_agent_block set - ToolStatus events may arrive after Finished
                                // It will be cleared when a new agent query starts
                            }
                            AgentEvent::Cancelled { .. } => {
                                block.fail("Cancelled".to_string());
                                state.active_agent_block = None;
                            }
                            AgentEvent::Error(err) => {
                                block.fail(err);
                                state.active_agent_block = None;
                            }
                        }
                    }
                }
            }
            return scrollable::snap_to(
                scrollable::Id::new(HISTORY_SCROLLABLE),
                scrollable::RelativeOffset::END,
            );
        }
        Message::AgentWidget(widget_msg) => {
            match widget_msg {
                AgentWidgetMessage::ToggleThinking(block_id) => {
                    if let Some(idx) = state.agent_block_index.get(&block_id) {
                        if let Some(block) = state.agent_blocks.get_mut(*idx) {
                            block.toggle_thinking();
                        }
                    }
                }
                AgentWidgetMessage::ToggleTool(block_id, tool_id) => {
                    if let Some(idx) = state.agent_block_index.get(&block_id) {
                        if let Some(block) = state.agent_blocks.get_mut(*idx) {
                            block.toggle_tool(&tool_id);
                        }
                    }
                }
                AgentWidgetMessage::PermissionGranted(block_id, perm_id) => {
                    if let Some(idx) = state.agent_block_index.get(&block_id) {
                        if let Some(block) = state.agent_blocks.get_mut(*idx) {
                            block.clear_permission();
                        }
                    }
                    // Send response to permission mediator
                    if let Some(ref tx) = state.permission_tx {
                        let _ = tx.send((perm_id, PermissionResponse::GrantedOnce));
                    }
                }
                AgentWidgetMessage::PermissionGrantedSession(block_id, perm_id) => {
                    if let Some(idx) = state.agent_block_index.get(&block_id) {
                        if let Some(block) = state.agent_blocks.get_mut(*idx) {
                            block.clear_permission();
                        }
                    }
                    if let Some(ref tx) = state.permission_tx {
                        let _ = tx.send((perm_id, PermissionResponse::GrantedSession));
                    }
                }
                AgentWidgetMessage::PermissionDenied(block_id, perm_id) => {
                    if let Some(idx) = state.agent_block_index.get(&block_id) {
                        if let Some(block) = state.agent_blocks.get_mut(*idx) {
                            block.clear_permission();
                            block.fail("Permission denied".to_string());
                        }
                    }
                    if let Some(ref tx) = state.permission_tx {
                        let _ = tx.send((perm_id, PermissionResponse::Denied));
                    }
                    state.active_agent_block = None;
                }
                AgentWidgetMessage::CopyText(text) => {
                    // Copy text to clipboard
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        let _ = clipboard.set_text(&text);
                    }
                }
            }
            return Task::none();
        }
        Message::CancelAgent => {
            if let Some(block_id) = state.active_agent_block {
                if let Some(idx) = state.agent_block_index.get(&block_id) {
                    if let Some(block) = state.agent_blocks.get_mut(*idx) {
                        block.fail("Cancelled by user".to_string());
                    }
                }
                state.active_agent_block = None;
            }
            return Task::none();
        }
        Message::InputChanged(value) => {
            // Suppress input if a Cmd shortcut just fired (prevents typing shortcut char)
            if state.suppress_next_input {
                state.suppress_next_input = false;
                return Task::none();
            }

            // Hide completion popup when input changes
            if state.completion_visible {
                state.completions.clear();
                state.completion_visible = false;
            }
            // Track previous value for ghost character detection
            state.input_before_event = state.input.clone();
            state.input = value;
            state.focus = Focus::Input;
        }
        Message::Submit => {
            let input = state.input.trim();
            if !input.is_empty() {
                // Check for one-shot agent prefix: "? " or "ai "
                let (is_agent_query, query) = if input.starts_with("? ") {
                    (true, input.strip_prefix("? ").unwrap().to_string())
                } else if input.starts_with("ai ") {
                    (true, input.strip_prefix("ai ").unwrap().to_string())
                } else {
                    (state.input_mode == InputMode::Agent, input.to_string())
                };

                let command = state.input.clone();
                state.input.clear();

                // Store attachments in kernel state before execution (Mathematica-style)
                // They'll be accessible via $ATTACHMENT variable
                if !state.attachments.is_empty() {
                    if let Ok(mut kernel) = state.kernel.try_lock() {
                        // Store as a list if multiple, or single value if one
                        let value = if state.attachments.len() == 1 {
                            state.attachments[0].clone()
                        } else {
                            nexus_api::Value::List(state.attachments.clone())
                        };
                        kernel.state_mut().set_var_value("ATTACHMENT", value);
                    }
                }
                // Clear attachments from UI
                state.attachments.clear();

                if is_agent_query {
                    // Build shell context for agent (append-only semantics)
                    // Context goes in user message, not system prompt, to preserve prefix caching
                    let shell_context = build_shell_context(
                        &state.cwd,
                        &state.blocks,
                        &state.command_history,
                    );

                    // Combine context with user query for the agent
                    let contextualized_query = format!("{}{}", shell_context, query);

                    // Log the contextualized query for development
                    tracing::info!("Agent query: {}", query);

                    // Create an agent block to show the query and receive responses
                    let block_id = BlockId(state.next_block_id);
                    state.next_block_id += 1;

                    let agent_block = AgentBlock::new(block_id, query.clone());

                    let idx = state.agent_blocks.len();
                    state.agent_block_index.insert(block_id, idx);
                    state.agent_blocks.push(agent_block);
                    state.active_agent_block = Some(block_id);

                    // Reset cancel flag for new agent task
                    state.agent_cancel.store(false, Ordering::SeqCst);

                    // Spawn actual agent task
                    let agent_tx = state.agent_tx.clone();
                    let cancel_flag = state.agent_cancel.clone();
                    let cwd = PathBuf::from(&state.cwd);

                    tokio::spawn(async move {
                        if let Err(e) = spawn_agent_task(
                            agent_tx,
                            cancel_flag,
                            contextualized_query,
                            cwd,
                        ).await {
                            tracing::error!("Agent task failed: {}", e);
                        }
                    });

                    // Mark block as streaming
                    if let Some(idx) = state.agent_block_index.get(&block_id) {
                        if let Some(block) = state.agent_blocks.get_mut(*idx) {
                            block.state = AgentBlockState::Streaming;
                        }
                    }

                    return scrollable::snap_to(
                        scrollable::Id::new(HISTORY_SCROLLABLE),
                        scrollable::RelativeOffset::END,
                    );
                } else {
                    return execute_command(state, command);
                }
            }
        }
        Message::PtyOutput(block_id, data) => {
            // O(1) lookup via HashMap
            if let Some(&idx) = state.block_index.get(&block_id) {
                if let Some(block) = state.blocks.get_mut(idx) {
                    block.parser.feed(&data);
                    block.version += 1; // Invalidate lazy cache
                    // Check for permission denied
                    if !block.has_permission_denied {
                        if let Ok(text) = std::str::from_utf8(&data) {
                            if text.to_lowercase().contains("permission denied") {
                                block.has_permission_denied = true;
                            }
                        }
                    }
                }
            }

            // VSYNC-BATCHED THROTTLING:
            // For small updates (typing echo), draw immediately for 0ms latency.
            // For large updates (streaming output), batch via dirty flag and
            // let NextFrame handle it (VSync-aligned for smooth rendering).
            if data.len() < 128 {
                // Small data = likely typing echo, draw immediately
                state.is_dirty = false; // We're handling it now
                return scrollable::snap_to(
                    scrollable::Id::new(HISTORY_SCROLLABLE),
                    scrollable::RelativeOffset::END,
                );
            } else {
                // Large data = streaming, mark dirty for VSync batching
                state.is_dirty = true;
                return Task::none();
            }
        }
        Message::PtyExited(block_id, exit_code) => {
            // O(1) lookup via HashMap
            let mut show_permission_prompt = false;
            let mut failed_command = None;
            if let Some(&idx) = state.block_index.get(&block_id) {
                if let Some(block) = state.blocks.get_mut(idx) {
                    block.state = if exit_code == 0 {
                        BlockState::Success
                    } else {
                        BlockState::Failed(exit_code)
                    };
                    block.duration_ms = Some(block.started_at.elapsed().as_millis() as u64);
                    block.version += 1;
                    // Check for permission denied on failed command
                    if exit_code != 0 && block.has_permission_denied {
                        show_permission_prompt = true;
                        failed_command = Some(block.command.clone());
                    }
                }
            }
            state.pty_handles.retain(|h| h.block_id != block_id);

            // Track last exit code for prompt color
            state.last_exit_code = Some(exit_code);

            // Show permission denied prompt if applicable
            if show_permission_prompt {
                state.permission_denied_command = failed_command;
            }

            // If the focused block exited, return focus to input
            if state.focus == Focus::Block(block_id) {
                state.focus = Focus::Input;
            }
        }
        Message::KernelEvent(shell_event) => {
            match shell_event {
                ShellEvent::CommandStarted { block_id, command, cwd: _ } => {
                    // Create block if it doesn't exist
                    if !state.block_index.contains_key(&block_id) {
                        let mut block = Block::new(block_id, command);
                        block.parser = TerminalParser::new(state.terminal_size.0, state.terminal_size.1);
                        let block_idx = state.blocks.len();
                        state.blocks.push(block);
                        state.block_index.insert(block_id, block_idx);
                    }
                }
                ShellEvent::StdoutChunk { block_id, data } => {
                    if let Some(&idx) = state.block_index.get(&block_id) {
                        if let Some(block) = state.blocks.get_mut(idx) {
                            block.parser.feed(&data);
                            block.version += 1;
                        }
                    }
                    state.is_dirty = true;
                }
                ShellEvent::StderrChunk { block_id, data } => {
                    if let Some(&idx) = state.block_index.get(&block_id) {
                        if let Some(block) = state.blocks.get_mut(idx) {
                            block.parser.feed(&data);
                            block.version += 1;
                            // Check for permission denied
                            if !block.has_permission_denied {
                                if let Ok(text) = std::str::from_utf8(&data) {
                                    if text.to_lowercase().contains("permission denied") {
                                        block.has_permission_denied = true;
                                    }
                                }
                            }
                        }
                    }
                    state.is_dirty = true;
                }
                ShellEvent::CommandOutput { block_id, value } => {
                    if let Some(&idx) = state.block_index.get(&block_id) {
                        if let Some(block) = state.blocks.get_mut(idx) {
                            block.native_output = Some(value);
                        }
                    }
                }
                ShellEvent::CommandFinished { block_id, exit_code, duration_ms } => {
                    let mut show_permission_prompt = false;
                    let mut failed_command = None;
                    if let Some(&idx) = state.block_index.get(&block_id) {
                        if let Some(block) = state.blocks.get_mut(idx) {
                            block.state = if exit_code == 0 {
                                BlockState::Success
                            } else {
                                BlockState::Failed(exit_code)
                            };
                            block.duration_ms = Some(duration_ms);
                            block.version += 1;
                            // Check for permission denied on failed command
                            if exit_code != 0 && block.has_permission_denied {
                                show_permission_prompt = true;
                                failed_command = Some(block.command.clone());
                            }
                        }
                    }
                    state.last_exit_code = Some(exit_code);
                    state.focus = Focus::Input;

                    // Show permission denied prompt if applicable
                    if show_permission_prompt {
                        state.permission_denied_command = failed_command;
                    }

                    return scrollable::snap_to(
                        scrollable::Id::new(HISTORY_SCROLLABLE),
                        scrollable::RelativeOffset::END,
                    );
                }
                ShellEvent::OpenClaudePanel { initial_prompt: _, cwd: _ } => {
                    // TODO: Integrate with nexus-agent when agent mode is fully implemented
                }
                ShellEvent::JobStateChanged { job_id, state: job_state } => {
                    // Update visual jobs
                    match job_state {
                        nexus_api::JobState::Running => {
                            // Add or update job
                            if let Some(job) = state.visual_jobs.iter_mut().find(|j| j.id == job_id) {
                                job.state = VisualJobState::Running;
                            } else {
                                // New job - we don't have the command here, use placeholder
                                state.visual_jobs.push(VisualJob::new(
                                    job_id,
                                    format!("Job {}", job_id),
                                    VisualJobState::Running,
                                ));
                            }
                        }
                        nexus_api::JobState::Stopped => {
                            // Add or update job as stopped
                            if let Some(job) = state.visual_jobs.iter_mut().find(|j| j.id == job_id) {
                                job.state = VisualJobState::Stopped;
                            } else {
                                state.visual_jobs.push(VisualJob::new(
                                    job_id,
                                    format!("Job {}", job_id),
                                    VisualJobState::Stopped,
                                ));
                            }
                        }
                        nexus_api::JobState::Done(_) => {
                            // Remove completed job
                            state.visual_jobs.retain(|j| j.id != job_id);
                        }
                    }
                }
                _ => {}
            }
        }
        Message::TableSort(block_id, column_index) => {
            // Toggle sort on the specified column
            if let Some(&idx) = state.block_index.get(&block_id) {
                if let Some(block) = state.blocks.get_mut(idx) {
                    block.table_sort.toggle(column_index);
                    block.version += 1; // Trigger redraw
                }
            }
        }
        Message::TableCellClick(block_id, row, col, value) => {
            // Handle semantic click on a table cell
            // For now, just log it - we'll add context menus later
            tracing::debug!("Cell clicked: block={:?}, row={}, col={}, value={:?}", block_id, row, col, value);
            // TODO: Open context menu or perform default action based on value type
        }
        Message::KeyPressed(key, modifiers) => {
            if let Focus::Block(block_id) = state.focus {
                if let Some(handle) = state.pty_handles.iter().find(|h| h.block_id == block_id) {
                    // Handle Ctrl+C/D/Z
                    if modifiers.control() {
                        if let Key::Character(c) = &key {
                            match c.to_lowercase().as_str() {
                                "c" => {
                                    let _ = handle.send_interrupt();
                                    return Task::none();
                                }
                                "d" => {
                                    let _ = handle.send_eof();
                                    return Task::none();
                                }
                                "z" => {
                                    let _ = handle.send_suspend();
                                    return Task::none();
                                }
                                _ => {}
                            }
                        }
                    }

                    // Escape returns focus to input
                    if matches!(key, Key::Named(keyboard::key::Named::Escape)) {
                        state.focus = Focus::Input;
                        return Task::none();
                    }

                    // Convert key to bytes and send to PTY
                    if let Some(bytes) = key_to_bytes(&key, &modifiers) {
                        let _ = handle.write(&bytes);
                    }
                }
            }
        }
        Message::Event(event, window_id) => {
            // Capture window ID for resize operations
            if state.window_id.is_none() {
                state.window_id = Some(window_id);
            }

            match event {
                Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    // Global shortcuts (Cmd+K, Cmd+Q, etc.) work regardless of focus
                    if modifiers.command() {
                        if let Key::Character(c) = &key {
                            let ch = c.to_lowercase();
                            let task = match ch.as_str() {
                                "k" => Some(update(state, Message::GlobalShortcut(GlobalShortcut::ClearScreen))),
                                "w" => Some(update(state, Message::GlobalShortcut(GlobalShortcut::CloseWindow))),
                                "q" => Some(update(state, Message::GlobalShortcut(GlobalShortcut::Quit))),
                                "c" => Some(update(state, Message::GlobalShortcut(GlobalShortcut::Copy))),
                                "v" => Some(update(state, Message::GlobalShortcut(GlobalShortcut::Paste))),
                                "=" | "+" => Some(update(state, Message::Zoom(ZoomDirection::In))),
                                "-" => Some(update(state, Message::Zoom(ZoomDirection::Out))),
                                "0" => Some(update(state, Message::Zoom(ZoomDirection::Reset))),
                                "." => Some(update(state, Message::ToggleInputMode)),
                                _ => None,
                            };
                            if let Some(task) = task {
                                // Suppress next input to prevent shortcut char from being typed
                                state.suppress_next_input = true;
                                return task;
                            }
                        }
                    }

                    // Ctrl+C in input clears the line (like traditional shell)
                    if modifiers.control() && matches!(state.focus, Focus::Input) {
                        if let Key::Character(c) = &key {
                            match c.to_lowercase().as_str() {
                                "c" => {
                                    state.input.clear();
                                    state.history_index = None;
                                    state.saved_input.clear();
                                    state.history_search_active = false;
                                    state.permission_denied_command = None;
                                    return Task::none();
                                }
                                "r" => {
                                    // Ctrl+R - start history search
                                    return update(state, Message::HistorySearchStart);
                                }
                                "s" => {
                                    // Ctrl+S - retry with sudo (if permission denied prompt is shown)
                                    if state.permission_denied_command.is_some() {
                                        return update(state, Message::RetryWithSudo);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }

                    // Focus-dependent key handling
                    match state.focus {
                        Focus::Input => {
                            // History search mode takes priority
                            if state.history_search_active {
                                match &key {
                                    Key::Named(keyboard::key::Named::Escape) => {
                                        return update(state, Message::HistorySearchCancel);
                                    }
                                    Key::Named(keyboard::key::Named::Enter) => {
                                        return update(state, Message::HistorySearchSelect(state.history_search_index));
                                    }
                                    Key::Named(keyboard::key::Named::ArrowUp) => {
                                        if state.history_search_index > 0 {
                                            state.history_search_index -= 1;
                                        }
                                        return Task::none();
                                    }
                                    Key::Named(keyboard::key::Named::ArrowDown) => {
                                        if state.history_search_index < state.history_search_results.len().saturating_sub(1) {
                                            state.history_search_index += 1;
                                        }
                                        return Task::none();
                                    }
                                    _ => {}
                                }
                                // Don't process other keys - let them go to the search input
                                return Task::none();
                            }

                            // Tab for completion
                            if matches!(key, Key::Named(keyboard::key::Named::Tab)) {
                                if state.completion_visible {
                                    // Apply selected completion
                                    return update(state, Message::SelectCompletion(state.completion_index));
                                } else {
                                    // Trigger completion
                                    return update(state, Message::TabCompletion);
                                }
                            }

                            // Escape to cancel completion
                            if matches!(key, Key::Named(keyboard::key::Named::Escape)) && state.completion_visible {
                                return update(state, Message::CancelCompletion);
                            }

                            // Arrow keys for completion navigation when popup is visible
                            if state.completion_visible {
                                match &key {
                                    Key::Named(keyboard::key::Named::ArrowUp) => {
                                        if state.completion_index > 0 {
                                            state.completion_index -= 1;
                                        }
                                        return Task::none();
                                    }
                                    Key::Named(keyboard::key::Named::ArrowDown) => {
                                        if state.completion_index < state.completions.len().saturating_sub(1) {
                                            state.completion_index += 1;
                                        }
                                        return Task::none();
                                    }
                                    Key::Named(keyboard::key::Named::Enter) => {
                                        return update(state, Message::SelectCompletion(state.completion_index));
                                    }
                                    _ => {}
                                }
                            }

                            // Up/Down for history navigation (only when completion not visible)
                            match &key {
                                Key::Named(keyboard::key::Named::ArrowUp) if !modifiers.shift() => {
                                    return update(state, Message::InputKey(key, modifiers));
                                }
                                Key::Named(keyboard::key::Named::ArrowDown) if !modifiers.shift() => {
                                    return update(state, Message::InputKey(key, modifiers));
                                }
                                _ => {}
                            }
                        }
                        Focus::Block(_) => {
                            // Forward key events to PTY
                            return update(state, Message::KeyPressed(key, modifiers));
                        }
                    }
                }
                Event::Window(iced::window::Event::Resized(size)) => {
                    return update(state, Message::WindowResized(size.width as u32, size.height as u32));
                }
                _ => {}
            }
        }
        Message::WindowResized(width, height) => {
            // Store window dimensions for zoom recalculation
            state.window_dims = (width as f32, height as f32);

            let old_cols = state.terminal_size.0;
            let (cols, rows) = state.recalculate_terminal_size();

            // Only resize if column count changed
            if cols != old_cols {
                state.apply_resize(cols, rows);
            }
        }
        Message::GlobalShortcut(shortcut) => {
            // Strip the shortcut character ONLY if text_input just typed it
            // (compare current input to input_before_event to avoid false positives)
            let strip_char = match &shortcut {
                GlobalShortcut::ClearScreen => Some('k'),
                GlobalShortcut::CloseWindow => Some('w'),
                GlobalShortcut::Quit => Some('q'),
                GlobalShortcut::Copy => Some('c'),
                GlobalShortcut::Paste => Some('v'),
            };
            if let Some(ch) = strip_char {
                // Only pop if input grew by exactly this character
                let expected_lower = format!("{}{}", state.input_before_event, ch);
                let expected_upper = format!("{}{}", state.input_before_event, ch.to_ascii_uppercase());
                if state.input == expected_lower || state.input == expected_upper {
                    state.input.pop();
                }
            }

            match shortcut {
                GlobalShortcut::ClearScreen => {
                    // Cancel any running agent task
                    state.agent_cancel.store(true, Ordering::SeqCst);
                    // Clear shell blocks
                    state.blocks.clear();
                    state.block_index.clear();
                    // Clear agent blocks
                    state.agent_blocks.clear();
                    state.agent_block_index.clear();
                    state.active_agent_block = None;
                }
                GlobalShortcut::CloseWindow | GlobalShortcut::Quit => {
                    return iced::exit();
                }
                GlobalShortcut::Copy => {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        // Copy the input (after stripping the 'c')
                        let _ = clipboard.set_text(&state.input);
                    }
                }
                GlobalShortcut::Paste => {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        // Try to get image first (Mathematica-style rich input)
                        if let Ok(img) = clipboard.get_image() {
                            // Convert ImageData to PNG bytes
                            let width = img.width as u32;
                            let height = img.height as u32;

                            // Create PNG from raw RGBA data
                            let mut png_data = Vec::new();
                            {
                                use image::{ImageBuffer, RgbaImage};
                                let img_buf: RgbaImage = ImageBuffer::from_raw(
                                    width, height, img.bytes.into_owned()
                                ).unwrap_or_else(|| ImageBuffer::new(1, 1));

                                img_buf.write_to(
                                    &mut std::io::Cursor::new(&mut png_data),
                                    image::ImageFormat::Png
                                ).ok();
                            }

                            if !png_data.is_empty() {
                                return update(state, Message::ImagePasted(png_data, width, height));
                            }
                        }

                        // Fall back to text paste
                        if let Ok(text) = clipboard.get_text() {
                            state.input.push_str(&text);
                        }
                    }
                }
            }
        }
        Message::Zoom(direction) => {
            // Strip the shortcut character ONLY if text_input just typed it
            let strip_chars: &[char] = match &direction {
                ZoomDirection::In => &['=', '+'],
                ZoomDirection::Out => &['-'],
                ZoomDirection::Reset => &['0'],
            };
            for &ch in strip_chars {
                let expected = format!("{}{}", state.input_before_event, ch);
                if state.input == expected {
                    state.input.pop();
                    break;
                }
            }

            let old_size = state.font_size;
            state.font_size = match direction {
                ZoomDirection::In => (state.font_size + 1.0).min(32.0),
                ZoomDirection::Out => (state.font_size - 1.0).max(8.0),
                ZoomDirection::Reset => DEFAULT_FONT_SIZE,
            };

            // Only resize if font size actually changed
            if (state.font_size - old_size).abs() > 0.001 {
                // Keep same terminal dimensions, resize window proportionally
                let (cols, rows) = state.terminal_size;
                let new_char_width = state.font_size * CHAR_WIDTH_RATIO;
                let new_line_height = state.font_size * LINE_HEIGHT_FACTOR;

                let h_padding = 30.0;
                let v_padding = 80.0;

                let new_width = (cols as f32 * new_char_width) + h_padding;
                let new_height = (rows as f32 * new_line_height) + v_padding;

                // Update cached window dims
                state.window_dims = (new_width, new_height);

                // Resize window if we have the ID
                if let Some(window_id) = state.window_id {
                    return iced::window::resize(
                        window_id,
                        iced::Size::new(new_width, new_height),
                    );
                }
            }
        }
        Message::NextFrame(_timestamp) => {
            // VSync-aligned frame - fires when monitor is ready for next frame
            // Only subscribed when is_dirty is true
            if state.is_dirty {
                state.is_dirty = false;
                // Request redraw by scrolling to bottom (this triggers view refresh)
                return scrollable::snap_to(
                    scrollable::Id::new(HISTORY_SCROLLABLE),
                    scrollable::RelativeOffset::END,
                );
            }
        }
        Message::JobClicked(job_id) => {
            // Bring job to foreground by executing 'fg %N'
            let command = format!("fg %{}", job_id);
            state.input.clear();
            return execute_command(state, command);
        }
        Message::InputKey(key, _modifiers) => {
            match &key {
                Key::Named(keyboard::key::Named::ArrowUp) => {
                    // Navigate to previous history entry
                    if state.command_history.is_empty() {
                        return Task::none();
                    }

                    match state.history_index {
                        None => {
                            // First press: save current input before browsing
                            state.saved_input = state.input.clone();
                            state.history_index = Some(state.command_history.len() - 1);
                        }
                        Some(0) => {
                            // Already at oldest, do nothing
                        }
                        Some(i) => {
                            state.history_index = Some(i - 1);
                        }
                    }

                    if let Some(i) = state.history_index {
                        state.input = state.command_history[i].clone();
                    }
                }
                Key::Named(keyboard::key::Named::ArrowDown) => {
                    match state.history_index {
                        None => {
                            // Not in history mode, do nothing
                        }
                        Some(i) if i >= state.command_history.len() - 1 => {
                            // At newest entry, restore saved input
                            state.history_index = None;
                            state.input = state.saved_input.clone();
                            state.saved_input.clear();
                        }
                        Some(i) => {
                            state.history_index = Some(i + 1);
                            state.input = state.command_history[i + 1].clone();
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Task::none()
}

/// Convert a keyboard key to bytes to send to the PTY.
fn key_to_bytes(key: &Key, modifiers: &Modifiers) -> Option<Vec<u8>> {
    match key {
        Key::Character(c) => {
            let s = c.as_str();
            if modifiers.control() && s.len() == 1 {
                // Ctrl+letter = ASCII 1-26
                let ch = s.chars().next()?;
                if ch.is_ascii_alphabetic() {
                    let ctrl_code = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
                    return Some(vec![ctrl_code]);
                }
            }
            Some(s.as_bytes().to_vec())
        }
        Key::Named(named) => {
            use keyboard::key::Named;

            // Handle modifier combinations for arrow keys
            if modifiers.control() {
                match named {
                    // Ctrl+Arrow for word navigation
                    Named::ArrowLeft => return Some(vec![0x1b, b'[', b'1', b';', b'5', b'D']),
                    Named::ArrowRight => return Some(vec![0x1b, b'[', b'1', b';', b'5', b'C']),
                    Named::ArrowUp => return Some(vec![0x1b, b'[', b'1', b';', b'5', b'A']),
                    Named::ArrowDown => return Some(vec![0x1b, b'[', b'1', b';', b'5', b'B']),
                    _ => {}
                }
            }

            if modifiers.shift() {
                match named {
                    Named::ArrowLeft => return Some(vec![0x1b, b'[', b'1', b';', b'2', b'D']),
                    Named::ArrowRight => return Some(vec![0x1b, b'[', b'1', b';', b'2', b'C']),
                    Named::ArrowUp => return Some(vec![0x1b, b'[', b'1', b';', b'2', b'A']),
                    Named::ArrowDown => return Some(vec![0x1b, b'[', b'1', b';', b'2', b'B']),
                    _ => {}
                }
            }

            if modifiers.alt() {
                match named {
                    Named::ArrowLeft => return Some(vec![0x1b, b'[', b'1', b';', b'3', b'D']),
                    Named::ArrowRight => return Some(vec![0x1b, b'[', b'1', b';', b'3', b'C']),
                    Named::ArrowUp => return Some(vec![0x1b, b'[', b'1', b';', b'3', b'A']),
                    Named::ArrowDown => return Some(vec![0x1b, b'[', b'1', b';', b'3', b'B']),
                    _ => {}
                }
            }

            match named {
                Named::Enter => Some(vec![b'\r']),
                Named::Backspace => Some(vec![0x7f]),
                Named::Tab => Some(vec![b'\t']),
                Named::Escape => Some(vec![0x1b]),
                Named::Space => Some(vec![b' ']),
                // Arrow keys
                Named::ArrowUp => Some(vec![0x1b, b'[', b'A']),
                Named::ArrowDown => Some(vec![0x1b, b'[', b'B']),
                Named::ArrowRight => Some(vec![0x1b, b'[', b'C']),
                Named::ArrowLeft => Some(vec![0x1b, b'[', b'D']),
                // Navigation
                Named::Home => Some(vec![0x1b, b'[', b'H']),
                Named::End => Some(vec![0x1b, b'[', b'F']),
                Named::PageUp => Some(vec![0x1b, b'[', b'5', b'~']),
                Named::PageDown => Some(vec![0x1b, b'[', b'6', b'~']),
                Named::Insert => Some(vec![0x1b, b'[', b'2', b'~']),
                Named::Delete => Some(vec![0x1b, b'[', b'3', b'~']),
                // Function keys
                Named::F1 => Some(vec![0x1b, b'O', b'P']),
                Named::F2 => Some(vec![0x1b, b'O', b'Q']),
                Named::F3 => Some(vec![0x1b, b'O', b'R']),
                Named::F4 => Some(vec![0x1b, b'O', b'S']),
                Named::F5 => Some(vec![0x1b, b'[', b'1', b'5', b'~']),
                Named::F6 => Some(vec![0x1b, b'[', b'1', b'7', b'~']),
                Named::F7 => Some(vec![0x1b, b'[', b'1', b'8', b'~']),
                Named::F8 => Some(vec![0x1b, b'[', b'1', b'9', b'~']),
                Named::F9 => Some(vec![0x1b, b'[', b'2', b'0', b'~']),
                Named::F10 => Some(vec![0x1b, b'[', b'2', b'1', b'~']),
                Named::F11 => Some(vec![0x1b, b'[', b'2', b'3', b'~']),
                Named::F12 => Some(vec![0x1b, b'[', b'2', b'4', b'~']),
                _ => None,
            }
        }
        _ => None,
    }
}

// ============================================================================
// Agent Task Spawning
// ============================================================================

/// No-op persistence for agent state (we don't persist agent sessions yet).
struct NoopPersistence;

impl nexus_agent::agent::persistence::AgentStatePersistence for NoopPersistence {
    fn save_agent_state(&mut self, _state: nexus_agent::SessionState) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Spawn an agent task to process a query.
async fn spawn_agent_task(
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    cancel_flag: Arc<AtomicBool>,
    query: String,
    working_dir: PathBuf,
) -> anyhow::Result<()> {
    use nexus_agent::{Agent, AgentComponents, SessionConfig};
    use nexus_executor::DefaultCommandExecutor;
    use nexus_llm::factory::create_llm_client_from_model;

    // Try to detect which model to use based on environment
    let model_name = if std::env::var("ANTHROPIC_API_KEY").is_ok() {
        "claude-sonnet"
    } else if std::env::var("OPENAI_API_KEY").is_ok() {
        "gpt-4o"
    } else {
        // Send error event if no API key is configured
        let _ = event_tx.send(AgentEvent::Error(
            "No API key found. Set ANTHROPIC_API_KEY or OPENAI_API_KEY environment variable.".to_string()
        ));
        return Ok(());
    };

    tracing::info!("Creating LLM client for model: {}", model_name);

    // Create LLM provider
    let llm_provider = match create_llm_client_from_model(model_name, None, false, None).await {
        Ok(provider) => provider,
        Err(e) => {
            let _ = event_tx.send(AgentEvent::Error(format!("Failed to create LLM client: {}", e)));
            return Ok(());
        }
    };

    // Create components with cancel flag connected to UI
    let ui = Arc::new(IcedAgentUI::with_cancel_flag(event_tx.clone(), cancel_flag));

    let components = AgentComponents {
        llm_provider,
        project_manager: Box::new(nexus_agent::config::DefaultProjectManager::new()),
        command_executor: Box::new(DefaultCommandExecutor),
        ui,
        state_persistence: Box::new(NoopPersistence),
        permission_handler: None, // TODO: Add permission handling
        sub_agent_runner: None,
    };

    // Create session config
    let session_config = SessionConfig {
        init_path: Some(working_dir),
        ..Default::default()
    };

    // Create and run agent
    let mut agent = Agent::new(components, session_config);

    // Initialize project context
    if let Err(e) = agent.init_project_context() {
        let _ = event_tx.send(AgentEvent::Error(format!("Failed to init project context: {}", e)));
        return Ok(());
    }

    // Add the user message
    if let Err(e) = agent.append_message(nexus_llm::Message::new_user(query)) {
        let _ = event_tx.send(AgentEvent::Error(format!("Failed to add message: {}", e)));
        return Ok(());
    }

    // Run the agent iteration
    if let Err(e) = agent.run_single_iteration().await {
        let _ = event_tx.send(AgentEvent::Error(format!("Agent error: {}", e)));
    }

    Ok(())
}

fn view(state: &Nexus) -> Element<'_, Message> {
    // Build all blocks - interleaved by BlockId for proper chronological ordering
    let font_size = state.font_size;

    // Collect unified blocks with their IDs for sorting
    let mut unified: Vec<(BlockId, UnifiedBlockRef)> = Vec::with_capacity(
        state.blocks.len() + state.agent_blocks.len()
    );

    for block in &state.blocks {
        unified.push((block.id, UnifiedBlockRef::Shell(block)));
    }
    for block in &state.agent_blocks {
        unified.push((block.id, UnifiedBlockRef::Agent(block)));
    }

    // Sort by BlockId (ascending) - gives chronological order
    unified.sort_by_key(|(id, _)| id.0);

    // Render in order
    let content_elements: Vec<Element<Message>> = unified
        .into_iter()
        .map(|(_, block_ref)| match block_ref {
            UnifiedBlockRef::Shell(block) => view_block(block, font_size),
            UnifiedBlockRef::Agent(block) => {
                view_agent_block(block, font_size).map(Message::AgentWidget)
            }
        })
        .collect();

    // Scrollable area for command history with ID for auto-scroll
    let history = scrollable(
        Column::with_children(content_elements)
            .spacing(4)
            .padding([10, 15]),
    )
    .id(scrollable::Id::new(HISTORY_SCROLLABLE))
    .height(Length::Fill);

    // Job status bar (only visible when jobs exist)
    let jobs_bar = job_status_bar(&state.visual_jobs, font_size, Message::JobClicked);

    // Input line always visible at bottom
    let input_line = container(view_input(state))
        .padding([8, 15])
        .width(Length::Fill);

    let content = column![history, jobs_bar, input_line].spacing(0);

    let result = container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.07, 0.07, 0.09,
            ))),
            ..Default::default()
        })
        .into();

    result
}

fn subscription(state: &Nexus) -> Subscription<Message> {
    let mut subscriptions = vec![
        pty_subscription(state.pty_rx.clone()),
        kernel_subscription(state.kernel_rx.clone()),
        agent_subscription(state.agent_rx.clone()),
    ];

    // Listen for all events with window ID - focus-dependent routing happens in update()
    subscriptions.push(
        event::listen_with(|event, _status, window_id| {
            Some(Message::Event(event, window_id))
        })
    );

    // VSYNC SUBSCRIPTION for throttled rendering:
    // Only subscribe to frame events if we have pending changes (is_dirty).
    // This fires when the monitor is ready for the next frame (VSync-aligned),
    // adapting to the user's monitor refresh rate (60Hz, 120Hz, 144Hz, etc.).
    // When idle (is_dirty = false), we don't subscribe, ensuring 0% CPU usage.
    if state.is_dirty {
        subscriptions.push(
            iced::window::frames().map(|_| Message::NextFrame(Instant::now()))
        );
    }

    Subscription::batch(subscriptions)
}

/// Async subscription that awaits kernel events.
fn kernel_subscription(
    rx: Arc<Mutex<broadcast::Receiver<ShellEvent>>>,
) -> Subscription<Message> {
    struct KernelSubscription;

    Subscription::run_with_id(
        std::any::TypeId::of::<KernelSubscription>(),
        stream::unfold(rx, |rx| async move {
            loop {
                let result = {
                    let mut guard = rx.lock().await;
                    guard.recv().await
                };

                match result {
                    Ok(shell_event) => return Some((Message::KernelEvent(shell_event), rx)),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Messages were dropped due to slow receiver, continue
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Channel closed, stop subscription
                        return None;
                    }
                }
            }
        }),
    )
}

/// Async subscription that awaits PTY events instead of polling.
fn pty_subscription(
    rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
) -> Subscription<Message> {
    struct PtySubscription;

    Subscription::run_with_id(
        std::any::TypeId::of::<PtySubscription>(),
        stream::unfold(rx, |rx| async move {
            let event = {
                let mut guard = rx.lock().await;
                guard.recv().await
            };

            match event {
                Some((block_id, PtyEvent::Output(data))) => {
                    Some((Message::PtyOutput(block_id, data), rx))
                }
                Some((block_id, PtyEvent::Exited(code))) => {
                    Some((Message::PtyExited(block_id, code), rx))
                }
                None => None,
            }
        }),
    )
}

/// Async subscription that awaits agent events.
fn agent_subscription(
    rx: Arc<Mutex<mpsc::UnboundedReceiver<AgentEvent>>>,
) -> Subscription<Message> {
    struct AgentSubscription;

    Subscription::run_with_id(
        std::any::TypeId::of::<AgentSubscription>(),
        stream::unfold(rx, |rx| async move {
            let event = {
                let mut guard = rx.lock().await;
                guard.recv().await
            };

            event.map(|agent_event| (Message::AgentEvent(agent_event), rx))
        }),
    )
}

fn execute_command(state: &mut Nexus, command: String) -> Task<Message> {
    let trimmed = command.trim().to_string();

    // Record in history (skip duplicates of last command, skip empty)
    if !trimmed.is_empty() {
        if state.command_history.last() != Some(&trimmed) {
            state.command_history.push(trimmed.clone());
            // Limit history size to prevent unbounded growth
            if state.command_history.len() > 1000 {
                state.command_history.remove(0);
            }
        }
    }

    // Reset history navigation state
    state.history_index = None;
    state.saved_input.clear();

    let block_id = BlockId(state.next_block_id);
    state.next_block_id += 1;

    // Handle built-in commands
    if trimmed == "clear" {
        // Cancel any running agent task
        state.agent_cancel.store(true, Ordering::SeqCst);
        // Clear shell blocks
        state.blocks.clear();
        state.block_index.clear();
        // Clear agent blocks
        state.agent_blocks.clear();
        state.agent_block_index.clear();
        state.active_agent_block = None;
        return Task::none();
    }

    if command.trim().starts_with("cd ") {
        let path = command.trim().strip_prefix("cd ").unwrap().trim();
        let new_path = if path.starts_with('/') {
            std::path::PathBuf::from(path)
        } else if path == "~" {
            home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"))
        } else {
            std::path::PathBuf::from(&state.cwd).join(path)
        };

        if let Ok(canonical) = new_path.canonicalize() {
            if canonical.is_dir() {
                state.cwd = canonical.display().to_string();
                let _ = std::env::set_current_dir(&canonical);
            }
        }
        return Task::none();
    }

    // All commands go through kernel now - this ensures outputs are stored
    // in kernel state for $_ / pipeline continuation
    execute_kernel_command(state, block_id, command)
}

/// Execute a command via the kernel (handles pipelines and native commands).
fn execute_kernel_command(state: &mut Nexus, block_id: BlockId, command: String) -> Task<Message> {
    // Check if this is a pipeline or native command - if so, use kernel
    let has_pipe = command.contains('|');
    let first_word = command.split_whitespace().next().unwrap_or("");
    let is_native = state.commands.contains(first_word);

    if has_pipe || is_native {
        // Create block for kernel command
        let mut block = Block::new(block_id, command.clone());
        block.parser = TerminalParser::new(state.terminal_size.0, state.terminal_size.1);
        let block_idx = state.blocks.len();
        state.blocks.push(block);
        state.block_index.insert(block_id, block_idx);

        // Pipeline/native execution via kernel
        // The kernel will emit events that we'll receive via the subscription
        let kernel = state.kernel.clone();
        let cwd = state.cwd.clone();
        let cmd = command.clone();

        // Spawn kernel execution in a thread, passing UI's block_id
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let mut kernel = kernel.lock().await;
                // Update kernel's cwd to match UI
                let _ = kernel.state_mut().set_cwd(std::path::PathBuf::from(&cwd));
                // Pass UI's block_id so events are routed correctly
                let _ = kernel.execute_with_block_id(&cmd, Some(block_id));
            });
        });

        // Return immediately - kernel events will update the UI
        return scrollable::snap_to(
            scrollable::Id::new(HISTORY_SCROLLABLE),
            scrollable::RelativeOffset::END,
        );
    }

    // Single external command - use PTY for interactive support
    // Create new block with current terminal size
    let mut block = Block::new(block_id, command.clone());
    block.parser = TerminalParser::new(state.terminal_size.0, state.terminal_size.1);
    let block_idx = state.blocks.len();
    state.blocks.push(block);
    state.block_index.insert(block_id, block_idx);

    // Auto-focus the new block for interactive commands
    state.focus = Focus::Block(block_id);

    // Spawn PTY with current terminal size
    let tx = state.pty_tx.clone();
    let cwd = state.cwd.clone();
    let (cols, rows) = state.terminal_size;

    match PtyHandle::spawn_with_size(&command, &cwd, block_id, tx, cols, rows) {
        Ok(handle) => {
            state.pty_handles.push(handle);
        }
        Err(e) => {
            tracing::error!("Failed to spawn PTY: {}", e);
            // O(1) lookup via HashMap
            if let Some(&idx) = state.block_index.get(&block_id) {
                if let Some(block) = state.blocks.get_mut(idx) {
                    block.state = BlockState::Failed(1);
                    block.parser.feed(format!("Error: {}\n", e).as_bytes());
                    block.version += 1;
                }
            }
            state.focus = Focus::Input;
        }
    }

    // Scroll to bottom to show new command
    scrollable::snap_to(
        scrollable::Id::new(HISTORY_SCROLLABLE),
        scrollable::RelativeOffset::END,
    )
}

fn view_block(block: &Block, font_size: f32) -> Element<'_, Message> {
    let prompt_color = iced::Color::from_rgb(0.3, 0.8, 0.5);
    let command_color = iced::Color::from_rgb(0.9, 0.9, 0.9);

    let prompt_line = row![
        text("$ ")
            .size(font_size)
            .color(prompt_color)
            .font(iced::Font::MONOSPACE),
        text(&block.command)
            .size(font_size)
            .color(command_color)
            .font(iced::Font::MONOSPACE),
    ]
    .spacing(0);

    // Check for native output first
    let output: Element<Message> = if block.collapsed {
        column![].into()
    } else if let Some(value) = &block.native_output {
        // Render structured value from native command
        render_value(value, block.id, &block.table_sort, font_size)
    } else {
        // Terminal output - only show cursor for running commands
        // For RUNNING blocks: use viewport-only grid (O(1) extraction)
        // For FINISHED blocks: use full scrollback (cached, O(1) after first extraction)
        // For alternate screen (TUI apps): always viewport only
        let grid = if block.parser.is_alternate_screen() || block.is_running() {
            // Running or alternate screen: viewport only (fast, O(1))
            block.parser.grid()
        } else {
            // Finished blocks: show all content including scrollback
            // This is cached after first extraction
            block.parser.grid_with_scrollback()
        };

        // Use GPU shader renderer for performance
        let content_rows = grid.content_rows() as usize;
        let (_cell_width, cell_height) = get_cell_metrics(font_size);
        TerminalShader::<Message>::new(&grid, font_size, 0, content_rows, cell_height)
            .widget()
            .into()
    };

    column![prompt_line, output].spacing(0).into()
}

/// Render a structured Value from a native command.
fn render_value<'a>(
    value: &'a Value,
    block_id: BlockId,
    table_sort: &'a TableSort,
    font_size: f32,
) -> Element<'a, Message> {
    use nexus_api::FileEntry;

    match value {
        Value::Unit => column![].into(),

        Value::List(items) => {
            // Check if it's a list of FileEntry
            let file_entries: Vec<&FileEntry> = items
                .iter()
                .filter_map(|v| match v {
                    Value::FileEntry(entry) => Some(entry.as_ref()),
                    _ => None,
                })
                .collect();

            if file_entries.len() == items.len() && !file_entries.is_empty() {
                // Render as file list
                render_file_list(&file_entries, font_size)
            } else {
                // Generic list rendering
                let lines: Vec<Element<Message>> = items
                    .iter()
                    .map(|item| {
                        text(item.to_text())
                            .size(font_size)
                            .color(iced::Color::from_rgb(0.8, 0.8, 0.8))
                            .font(iced::Font::MONOSPACE)
                            .into()
                    })
                    .collect();
                Column::with_children(lines).spacing(0).into()
            }
        }

        Value::Table { columns, rows } => {
            // Use interactive table with sortable headers
            interactive_table(
                columns,
                rows,
                table_sort,
                font_size,
                move |col_idx| Message::TableSort(block_id, col_idx),
                None::<fn(usize, usize, &Value) -> Message>,
            )
        }

        Value::FileEntry(entry) => {
            render_file_list(&[entry.as_ref()], font_size)
        }

        Value::Media { data, content_type, metadata } => {
            render_media(data, content_type, metadata, font_size)
        }

        // For other types, just render as text
        _ => {
            text(value.to_text())
                .size(font_size)
                .color(iced::Color::from_rgb(0.8, 0.8, 0.8))
                .font(iced::Font::MONOSPACE)
                .into()
        }
    }
}

/// Render media content (images, audio, video, documents).
fn render_media<'a>(
    data: &'a [u8],
    content_type: &'a str,
    metadata: &'a nexus_api::MediaMetadata,
    font_size: f32,
) -> Element<'a, Message> {
    use iced::widget::image;

    // Images: render inline
    if content_type.starts_with("image/") {
        let handle = image::Handle::from_bytes(data.to_vec());

        // Determine size - use metadata if available, otherwise default max
        let (width, height) = match (metadata.width, metadata.height) {
            (Some(w), Some(h)) => {
                // Scale down if too large, max 600px width
                let max_width = 600.0;
                let max_height = 400.0;
                let scale = (max_width / w as f32).min(max_height / h as f32).min(1.0);
                ((w as f32 * scale) as u16, (h as f32 * scale) as u16)
            }
            _ => (400, 300), // Default size if dimensions unknown
        };

        let img = image::Image::new(handle)
            .width(Length::Fixed(width as f32))
            .height(Length::Fixed(height as f32));

        let label = if let Some(name) = &metadata.filename {
            format!("{} ({})", name, content_type)
        } else {
            content_type.to_string()
        };

        column![
            img,
            text(label)
                .size(font_size * 0.9)
                .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
                .font(iced::Font::MONOSPACE)
        ]
        .spacing(4)
        .into()
    }
    // Audio: show info placeholder (actual player would need more work)
    else if content_type.starts_with("audio/") {
        let duration = metadata.duration_secs
            .map(|d| format!(" ({:.1}s)", d))
            .unwrap_or_default();
        let name = metadata.filename.as_deref().unwrap_or("audio");
        let size = format_file_size(data.len() as u64);

        column![
            text(format!("🔊 {}{}", name, duration))
                .size(font_size)
                .color(iced::Color::from_rgb(0.5, 0.8, 0.5)),
            text(format!("{} • {}", content_type, size))
                .size(font_size * 0.9)
                .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
                .font(iced::Font::MONOSPACE)
        ]
        .spacing(2)
        .into()
    }
    // Video: show info placeholder
    else if content_type.starts_with("video/") {
        let duration = metadata.duration_secs
            .map(|d| format!(" ({:.1}s)", d))
            .unwrap_or_default();
        let dims = match (metadata.width, metadata.height) {
            (Some(w), Some(h)) => format!(" {}x{}", w, h),
            _ => String::new(),
        };
        let name = metadata.filename.as_deref().unwrap_or("video");
        let size = format_file_size(data.len() as u64);

        column![
            text(format!("🎬 {}{}{}", name, dims, duration))
                .size(font_size)
                .color(iced::Color::from_rgb(0.5, 0.7, 0.9)),
            text(format!("{} • {}", content_type, size))
                .size(font_size * 0.9)
                .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
                .font(iced::Font::MONOSPACE)
        ]
        .spacing(2)
        .into()
    }
    // PDF and other documents
    else if content_type == "application/pdf" {
        let name = metadata.filename.as_deref().unwrap_or("document.pdf");
        let size = format_file_size(data.len() as u64);

        column![
            text(format!("📄 {}", name))
                .size(font_size)
                .color(iced::Color::from_rgb(0.9, 0.6, 0.5)),
            text(format!("{} • {}", content_type, size))
                .size(font_size * 0.9)
                .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
                .font(iced::Font::MONOSPACE)
        ]
        .spacing(2)
        .into()
    }
    // Generic binary: show type and size
    else {
        let name = metadata.filename.as_deref().unwrap_or("file");
        let size = format_file_size(data.len() as u64);

        column![
            text(format!("📎 {}", name))
                .size(font_size)
                .color(iced::Color::from_rgb(0.7, 0.7, 0.7)),
            text(format!("{} • {}", content_type, size))
                .size(font_size * 0.9)
                .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
                .font(iced::Font::MONOSPACE)
        ]
        .spacing(2)
        .into()
    }
}

fn format_file_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Render a list of file entries (simple ls-style output).
fn render_file_list(entries: &[&nexus_api::FileEntry], font_size: f32) -> Element<'static, Message> {
    use nexus_api::FileType;

    let lines: Vec<Element<Message>> = entries
        .iter()
        .map(|entry| {
            // Color based on file type
            let color = match entry.file_type {
                FileType::Directory => iced::Color::from_rgb(0.4, 0.6, 1.0), // Blue for dirs
                FileType::Symlink => iced::Color::from_rgb(0.4, 0.9, 0.9),   // Cyan for symlinks
                _ if entry.permissions & 0o111 != 0 => iced::Color::from_rgb(0.4, 0.9, 0.4), // Green for executables
                _ => iced::Color::from_rgb(0.8, 0.8, 0.8), // White for regular files
            };

            let display_name = if let Some(target) = &entry.symlink_target {
                format!("{} -> {}", entry.name, target.display())
            } else {
                entry.name.clone()
            };

            text(display_name)
                .size(font_size)
                .color(color)
                .font(iced::Font::MONOSPACE)
                .into()
        })
        .collect();

    Column::with_children(lines).spacing(0).into()
}

fn view_input(state: &Nexus) -> Element<'_, Message> {
    // Cornflower blue for path
    let path_color = iced::Color::from_rgb8(100, 149, 237);
    // Green for success, red for failure
    let prompt_color = match state.last_exit_code {
        Some(code) if code != 0 => iced::Color::from_rgb8(220, 50, 50), // Red
        _ => iced::Color::from_rgb8(50, 205, 50), // Lime green
    };

    // Shorten path (replace home with ~)
    let display_path = shorten_path(&state.cwd);

    let path_text = text(format!("{} ", display_path))
        .size(state.font_size)
        .color(path_color)
        .font(iced::Font::MONOSPACE);

    // Mode indicator - shows SHELL or AGENT mode
    let (mode_label, mode_color) = match state.input_mode {
        InputMode::Shell => ("$", prompt_color),
        InputMode::Agent => ("?", iced::Color::from_rgb(0.5, 0.6, 1.0)),
    };

    let prompt = text(format!("{} ", mode_label))
        .size(state.font_size)
        .color(mode_color)
        .font(iced::Font::MONOSPACE);

    let input = text_input("", &state.input)
        .on_input(Message::InputChanged)
        .on_submit(Message::Submit)
        .padding(0)
        .size(state.font_size)
        .style(|_theme, _status| text_input::Style {
            background: iced::Background::Color(iced::Color::TRANSPARENT),
            border: iced::Border {
                width: 0.0,
                ..Default::default()
            },
            icon: iced::Color::from_rgb(0.5, 0.5, 0.5),
            placeholder: iced::Color::from_rgb(0.4, 0.4, 0.4),
            value: iced::Color::from_rgb(0.9, 0.9, 0.9),
            selection: iced::Color::from_rgb(0.3, 0.5, 0.8),
        })
        .font(iced::Font::MONOSPACE);

    let input_row = row![path_text, prompt, input]
        .spacing(0)
        .align_y(iced::Alignment::Center);

    // Display attachments if any (Mathematica-style rich input)
    let attachments_view: Option<Element<'_, Message>> = if state.attachments.is_empty() {
        None
    } else {
        let attachment_items: Vec<Element<'_, Message>> = state.attachments
            .iter()
            .enumerate()
            .map(|(i, value)| {
                match value {
                    nexus_api::Value::Media { data, content_type, metadata } => {
                        let is_image = content_type.starts_with("image/");
                        let label = if is_image {
                            format!(
                                "Image {}x{}",
                                metadata.width.unwrap_or(0),
                                metadata.height.unwrap_or(0)
                            )
                        } else {
                            metadata.filename.clone().unwrap_or_else(|| "File".to_string())
                        };

                        // Small thumbnail for images, icon for others
                        let preview: Element<'_, Message> = if is_image {
                            // Create thumbnail preview
                            let handle = iced::widget::image::Handle::from_bytes(data.clone());
                            iced::widget::image(handle)
                                .width(Length::Fixed(60.0))
                                .height(Length::Fixed(60.0))
                                .into()
                        } else {
                            // File icon placeholder
                            text("📎")
                                .size(24.0)
                                .into()
                        };

                        let remove_btn = iced::widget::button(
                            text("×").size(14.0).color(iced::Color::WHITE)
                        )
                            .on_press(Message::RemoveAttachment(i))
                            .padding(2)
                            .style(|_theme, _status| iced::widget::button::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgb(0.6, 0.2, 0.2))),
                                text_color: iced::Color::WHITE,
                                border: iced::Border {
                                    radius: 10.0.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            });

                        let attachment_card = container(
                            column![
                                row![preview, remove_btn].spacing(4).align_y(iced::Alignment::Start),
                                text(label).size(state.font_size * 0.7).color(iced::Color::from_rgb(0.6, 0.6, 0.6))
                            ].spacing(2).align_x(iced::Alignment::Center)
                        )
                            .padding(4)
                            .style(|_| container::Style {
                                background: Some(iced::Background::Color(iced::Color::from_rgb(0.15, 0.15, 0.18))),
                                border: iced::Border {
                                    radius: 4.0.into(),
                                    width: 1.0,
                                    color: iced::Color::from_rgb(0.3, 0.3, 0.35),
                                },
                                ..Default::default()
                            });

                        attachment_card.into()
                    }
                    _ => text("?").into(),
                }
            })
            .collect();

        Some(row(attachment_items).spacing(8).into())
    };

    // Show history search popup if active
    if state.history_search_active {
        let search_label = text("(reverse-i-search)")
            .size(state.font_size * 0.9)
            .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
            .font(iced::Font::MONOSPACE);

        let search_input = text_input("type to search...", &state.history_search_query)
            .on_input(Message::HistorySearchChanged)
            .padding([4, 8])
            .size(state.font_size)
            .style(|_theme, _status| text_input::Style {
                background: iced::Background::Color(iced::Color::from_rgb(0.15, 0.15, 0.18)),
                border: iced::Border {
                    radius: 4.0.into(),
                    width: 1.0,
                    color: iced::Color::from_rgb(0.4, 0.6, 0.8),
                },
                icon: iced::Color::from_rgb(0.5, 0.5, 0.5),
                placeholder: iced::Color::from_rgb(0.4, 0.4, 0.4),
                value: iced::Color::from_rgb(0.9, 0.9, 0.9),
                selection: iced::Color::from_rgb(0.3, 0.5, 0.8),
            })
            .font(iced::Font::MONOSPACE);

        let search_header = row![search_label, search_input]
            .spacing(8)
            .align_y(iced::Alignment::Center);

        // Build result items
        let result_items: Vec<Element<Message>> = state
            .history_search_results
            .iter()
            .enumerate()
            .take(10)
            .map(|(i, entry)| {
                let is_selected = i == state.history_search_index;
                let bg_color = if is_selected {
                    iced::Color::from_rgb(0.2, 0.4, 0.6)
                } else {
                    iced::Color::from_rgb(0.12, 0.12, 0.15)
                };
                let text_color = if is_selected {
                    iced::Color::WHITE
                } else {
                    iced::Color::from_rgb(0.8, 0.8, 0.8)
                };
                let time_color = iced::Color::from_rgb(0.5, 0.5, 0.5);

                // Format timestamp as relative time
                let time_str = format_relative_time(&entry.timestamp);
                let command = entry.command.clone();

                let item_content = row![
                    text(command)
                        .size(state.font_size * 0.9)
                        .color(text_color)
                        .font(iced::Font::MONOSPACE)
                        .width(Length::Fill),
                    text(time_str)
                        .size(state.font_size * 0.8)
                        .color(time_color)
                        .font(iced::Font::MONOSPACE),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center);

                iced::widget::button(item_content)
                    .on_press(Message::HistorySearchSelect(i))
                    .padding([6, 10])
                    .width(Length::Fill)
                    .style(move |_theme, _status| iced::widget::button::Style {
                        background: Some(iced::Background::Color(bg_color)),
                        text_color,
                        border: iced::Border::default(),
                        ..Default::default()
                    })
                    .into()
            })
            .collect();

        let results_list: Element<Message> = if result_items.is_empty() {
            text("No matches found")
                .size(state.font_size * 0.9)
                .color(iced::Color::from_rgb(0.5, 0.5, 0.5))
                .font(iced::Font::MONOSPACE)
                .into()
        } else {
            Column::with_children(result_items).spacing(0).into()
        };

        let popup = container(
            column![search_header, results_list].spacing(8)
        )
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(0.1, 0.1, 0.12))),
            border: iced::Border {
                radius: 6.0.into(),
                width: 1.0,
                color: iced::Color::from_rgb(0.3, 0.5, 0.7),
            },
            ..Default::default()
        })
        .padding(10)
        .width(Length::Fill);

        return column![popup, input_row].spacing(8).into();
    }

    // Show permission denied prompt if applicable
    if let Some(ref cmd) = state.permission_denied_command {
        let warning_icon = text("⚠️")
            .size(state.font_size);
        let message = text("Permission denied")
            .size(state.font_size * 0.95)
            .color(iced::Color::from_rgb(1.0, 0.7, 0.3))
            .font(iced::Font::MONOSPACE);
        let cmd_text = text(format!("Command: {}", cmd))
            .size(state.font_size * 0.85)
            .color(iced::Color::from_rgb(0.6, 0.6, 0.6))
            .font(iced::Font::MONOSPACE);

        let retry_btn = iced::widget::button(
            text("Retry with sudo")
                .size(state.font_size * 0.9)
        )
        .on_press(Message::RetryWithSudo)
        .padding([6, 12])
        .style(|_theme, _status| iced::widget::button::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(0.3, 0.5, 0.7))),
            text_color: iced::Color::WHITE,
            border: iced::Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });

        let dismiss_btn = iced::widget::button(
            text("Dismiss")
                .size(state.font_size * 0.9)
        )
        .on_press(Message::DismissPermissionPrompt)
        .padding([6, 12])
        .style(|_theme, _status| iced::widget::button::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(0.25, 0.25, 0.28))),
            text_color: iced::Color::from_rgb(0.8, 0.8, 0.8),
            border: iced::Border {
                radius: 4.0.into(),
                ..Default::default()
            },
            ..Default::default()
        });

        let hotkey_hint = text("Ctrl+S to retry")
            .size(state.font_size * 0.75)
            .color(iced::Color::from_rgb(0.5, 0.5, 0.5))
            .font(iced::Font::MONOSPACE);

        let header = row![warning_icon, message].spacing(8).align_y(iced::Alignment::Center);
        let buttons = row![retry_btn, dismiss_btn, hotkey_hint]
            .spacing(10)
            .align_y(iced::Alignment::Center);

        let prompt = container(
            column![header, cmd_text, buttons].spacing(6)
        )
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(0.15, 0.12, 0.1))),
            border: iced::Border {
                radius: 6.0.into(),
                width: 1.0,
                color: iced::Color::from_rgb(0.6, 0.4, 0.2),
            },
            ..Default::default()
        })
        .padding(10)
        .width(Length::Fill);

        return column![prompt, input_row].spacing(8).into();
    }

    // Show completion popup if visible
    if state.completion_visible && !state.completions.is_empty() {
        let completion_items: Vec<Element<Message>> = state
            .completions
            .iter()
            .enumerate()
            .take(10) // Show max 10 items
            .map(|(i, completion)| {
                let is_selected = i == state.completion_index;
                let bg_color = if is_selected {
                    iced::Color::from_rgb(0.2, 0.4, 0.6)
                } else {
                    iced::Color::from_rgb(0.15, 0.15, 0.18)
                };
                let text_color = if is_selected {
                    iced::Color::WHITE
                } else {
                    iced::Color::from_rgb(0.8, 0.8, 0.8)
                };

                let icon = completion.kind.icon();
                let kind_color = match completion.kind {
                    CompletionKind::Directory => iced::Color::from_rgb(0.4, 0.7, 1.0),
                    CompletionKind::Executable | CompletionKind::NativeCommand => iced::Color::from_rgb(0.4, 0.9, 0.4),
                    CompletionKind::Builtin => iced::Color::from_rgb(1.0, 0.8, 0.4),
                    CompletionKind::Function => iced::Color::from_rgb(0.8, 0.6, 1.0),
                    CompletionKind::Variable => iced::Color::from_rgb(1.0, 0.6, 0.6),
                    _ => text_color,
                };

                let item_content = row![
                    text(icon).size(state.font_size * 0.9).color(kind_color),
                    text(" ").size(state.font_size * 0.9),
                    text(&completion.text).size(state.font_size * 0.9).color(text_color).font(iced::Font::MONOSPACE),
                ]
                .spacing(2)
                .align_y(iced::Alignment::Center);

                iced::widget::button(item_content)
                    .on_press(Message::SelectCompletion(i))
                    .padding([4, 8])
                    .width(Length::Fill)
                    .style(move |_theme, _status| iced::widget::button::Style {
                        background: Some(iced::Background::Color(bg_color)),
                        text_color,
                        border: iced::Border::default(),
                        ..Default::default()
                    })
                    .into()
            })
            .collect();

        let popup = container(
            Column::with_children(completion_items)
                .spacing(0)
                .width(Length::Fixed(300.0))
        )
        .style(|_| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(0.12, 0.12, 0.15))),
            border: iced::Border {
                radius: 4.0.into(),
                width: 1.0,
                color: iced::Color::from_rgb(0.3, 0.3, 0.35),
            },
            ..Default::default()
        })
        .padding(4);

        if let Some(attachments) = attachments_view {
            column![attachments, popup, input_row].spacing(4).into()
        } else {
            column![popup, input_row].spacing(4).into()
        }
    } else if let Some(attachments) = attachments_view {
        column![attachments, input_row].spacing(4).into()
    } else {
        input_row.into()
    }
}

/// Shorten a path by replacing home directory with ~.
fn shorten_path(path: &str) -> String {
    if let Some(home) = home_dir() {
        let home_str = home.display().to_string();
        if path.starts_with(&home_str) {
            return path.replacen(&home_str, "~", 1);
        }
    }
    path.to_string()
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

/// Format a timestamp as a relative time string.
fn format_relative_time(timestamp: &chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(timestamp);

    if duration.num_seconds() < 60 {
        "just now".to_string()
    } else if duration.num_minutes() < 60 {
        let mins = duration.num_minutes();
        format!("{}m ago", mins)
    } else if duration.num_hours() < 24 {
        let hours = duration.num_hours();
        format!("{}h ago", hours)
    } else if duration.num_days() < 7 {
        let days = duration.num_days();
        format!("{}d ago", days)
    } else {
        timestamp.format("%b %d").to_string()
    }
}
