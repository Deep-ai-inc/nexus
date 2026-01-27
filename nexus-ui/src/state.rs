//! Application state for the Nexus UI.
//!
//! State is organized into domains for better encapsulation:
//! - `InputState`: Text input, completion, history search
//! - `TerminalState`: Blocks, PTY handles, kernel
//! - `AgentState`: Agent blocks, active agent
//! - `WindowState`: Window dimensions, font size

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use iced::widget::text_editor;
use tokio::sync::{broadcast, mpsc, Mutex};

use nexus_api::{BlockId, ShellEvent, Value};
use nexus_kernel::{CommandRegistry, Completion, HistoryEntry, Kernel};

use crate::agent_adapter::{AgentEvent, PermissionResponse};
use crate::agent_block::AgentBlock;
use crate::blocks::{Block, Focus, InputMode, PtyEvent};
use crate::constants::{CHAR_WIDTH_RATIO, DEFAULT_FONT_SIZE, LINE_HEIGHT_FACTOR};
use crate::pty::PtyHandle;
use crate::widgets::job_indicator::VisualJob;

// =============================================================================
// Input Domain State
// =============================================================================

/// State for the input area (typing, completion, history search).
pub struct InputState {
    /// Current editor content (multi-line support).
    pub content: text_editor::Content,
    /// Current input mode (Shell or Agent).
    pub mode: InputMode,
    /// Pending attachments (images/files) for rich input.
    pub attachments: Vec<Value>,
    /// Shell command history for Up/Down navigation.
    pub shell_history: Vec<String>,
    /// Current position in shell history (None = new command being typed).
    pub shell_history_index: Option<usize>,
    /// Agent query history for Up/Down navigation.
    pub agent_history: Vec<String>,
    /// Current position in agent history (None = new query being typed).
    pub agent_history_index: Option<usize>,
    /// Saved input when browsing history (to restore on Down).
    pub saved_input: String,
    /// Tab completion candidates (filtered view of all_completions).
    pub completions: Vec<Completion>,
    /// All completion candidates (unfiltered, for backspace recovery).
    pub all_completions: Vec<Completion>,
    /// Selected completion index.
    pub completion_index: usize,
    /// Start position of the word being completed.
    pub completion_start: usize,
    /// Original text when completion was triggered (for reliable insertion).
    pub completion_original_text: String,
    /// Whether completion popup is visible.
    pub completion_visible: bool,
    /// History search active (Ctrl+R mode).
    pub search_active: bool,
    /// History search query.
    pub search_query: String,
    /// History search results.
    pub search_results: Vec<HistoryEntry>,
    /// Selected history search index.
    pub search_index: usize,
    /// Character to suppress if it arrives after the shortcut handler.
    /// Only suppresses if the Changed event matches this exact character appended.
    pub suppress_char: Option<char>,
    /// Input value before current event (for shortcut character stripping).
    pub before_event: String,
}

impl InputState {
    /// Get the current input text.
    pub fn text(&self) -> String {
        self.content.text()
    }

    /// Set the input text, moving cursor to end.
    pub fn set_text(&mut self, text: &str) {
        self.content = text_editor::Content::with_text(text);
        // Move cursor to end (shell UX expectation)
        self.content
            .perform(text_editor::Action::Move(text_editor::Motion::DocumentEnd));
    }

    /// Clear the input.
    pub fn clear(&mut self) {
        self.content = text_editor::Content::new();
    }

    /// Get the number of lines in the input.
    pub fn line_count(&self) -> usize {
        self.content.line_count()
    }

    /// Get cursor position (line, column) for boundary detection.
    pub fn cursor_position(&self) -> (usize, usize) {
        self.content.cursor_position()
    }

    /// Add a shell command to history if it's not a duplicate of the last entry.
    pub fn push_shell_history(&mut self, command: &str) {
        if self.shell_history.last().map(|s| s.as_str()) != Some(command) {
            self.shell_history.push(command.to_string());
            if self.shell_history.len() > 1000 {
                self.shell_history.remove(0);
            }
        }
    }

    /// Add an agent query to history if it's not a duplicate of the last entry.
    pub fn push_agent_history(&mut self, query: &str) {
        if self.agent_history.last().map(|s| s.as_str()) != Some(query) {
            self.agent_history.push(query.to_string());
            if self.agent_history.len() > 1000 {
                self.agent_history.remove(0);
            }
        }
    }

    /// Get the current history based on mode.
    pub fn current_history(&self) -> &[String] {
        match self.mode {
            InputMode::Shell => &self.shell_history,
            InputMode::Agent => &self.agent_history,
        }
    }

    /// Get current history index based on mode.
    pub fn current_history_index(&self) -> Option<usize> {
        match self.mode {
            InputMode::Shell => self.shell_history_index,
            InputMode::Agent => self.agent_history_index,
        }
    }

    /// Set current history index based on mode.
    pub fn set_history_index(&mut self, index: Option<usize>) {
        match self.mode {
            InputMode::Shell => self.shell_history_index = index,
            InputMode::Agent => self.agent_history_index = index,
        }
    }

    /// Clear pending attachments.
    pub fn clear_attachments(&mut self) {
        self.attachments.clear();
    }

    /// Take attachments, returning them and clearing the internal list.
    pub fn take_attachments(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.attachments)
    }
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            content: text_editor::Content::new(),
            mode: InputMode::default(),
            attachments: Vec::new(),
            shell_history: Vec::new(),
            shell_history_index: None,
            agent_history: Vec::new(),
            agent_history_index: None,
            saved_input: String::new(),
            completions: Vec::new(),
            all_completions: Vec::new(),
            completion_index: 0,
            completion_start: 0,
            completion_original_text: String::new(),
            completion_visible: false,
            search_active: false,
            search_query: String::new(),
            search_results: Vec::new(),
            search_index: 0,
            suppress_char: None,
            before_event: String::new(),
        }
    }
}

// =============================================================================
// Terminal Domain State
// =============================================================================

/// State for terminal/PTY execution (blocks, commands, jobs).
pub struct TerminalState {
    /// Command blocks (ordered).
    pub blocks: Vec<Block>,
    /// Block index by ID for O(1) lookup.
    pub block_index: HashMap<BlockId, usize>,
    /// Next block ID.
    pub next_block_id: u64,
    /// Current working directory.
    pub cwd: String,
    /// Active PTY handles.
    pub pty_handles: Vec<PtyHandle>,
    /// Channel for PTY output.
    pub pty_tx: mpsc::UnboundedSender<(BlockId, PtyEvent)>,
    /// Receiver for PTY events (shared with subscription).
    /// Event sender for emitting kernel events (used for crash recovery).
    pub kernel_tx: broadcast::Sender<ShellEvent>,
    pub pty_rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
    /// Current focus state.
    pub focus: Focus,
    /// Terminal dimensions (cols, rows).
    pub terminal_size: (u16, u16),
    /// Exit code of last command (for prompt color).
    pub last_exit_code: Option<i32>,
    /// Registry of native (in-process) commands.
    pub commands: CommandRegistry,
    /// Kernel for pipeline execution.
    pub kernel: Arc<Mutex<Kernel>>,
    /// Receiver for kernel events (shared with subscription).
    pub kernel_rx: Arc<Mutex<broadcast::Receiver<ShellEvent>>>,
    /// Visual jobs for the status bar.
    pub jobs: Vec<VisualJob>,
    /// Command that failed with permission denied (for sudo retry).
    pub permission_denied_command: Option<String>,
    /// Command not found info (original command, suggestions).
    pub command_not_found: Option<(String, Vec<String>)>,
    /// Is there processed PTY data that hasn't been drawn yet?
    pub is_dirty: bool,
}

impl TerminalState {
    /// Create a new terminal state with the given kernel.
    pub fn new(
        kernel: Kernel,
        kernel_tx: broadcast::Sender<ShellEvent>,
        kernel_rx: broadcast::Receiver<ShellEvent>,
    ) -> Self {
        let (pty_tx, pty_rx) = mpsc::unbounded_channel();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~".to_string());

        Self {
            blocks: Vec::new(),
            block_index: HashMap::new(),
            next_block_id: 1,
            cwd,
            pty_handles: Vec::new(),
            pty_tx,
            pty_rx: Arc::new(Mutex::new(pty_rx)),
            kernel_tx,
            focus: Focus::Input,
            terminal_size: (120, 24),
            last_exit_code: None,
            commands: CommandRegistry::new(),
            kernel: Arc::new(Mutex::new(kernel)),
            kernel_rx: Arc::new(Mutex::new(kernel_rx)),
            jobs: Vec::new(),
            permission_denied_command: None,
            command_not_found: None,
            is_dirty: false,
        }
    }

    /// Reset terminal state, clearing all blocks.
    /// Used by ClearAll action.
    pub fn reset(&mut self) {
        self.blocks.clear();
        self.block_index.clear();
    }

    /// Apply terminal resize to all blocks and PTYs.
    pub fn apply_resize(&mut self, cols: u16, rows: u16) {
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

    /// Allocate a new block ID.
    pub fn next_id(&mut self) -> BlockId {
        let id = BlockId(self.next_block_id);
        self.next_block_id += 1;
        id
    }
}

// =============================================================================
// Agent Domain State
// =============================================================================

/// State for AI agent interactions.
pub struct AgentState {
    /// Agent conversation blocks.
    pub blocks: Vec<AgentBlock>,
    /// Agent block index by ID for O(1) lookup.
    pub block_index: HashMap<BlockId, usize>,
    /// Currently active agent block (receiving events).
    pub active_block: Option<BlockId>,
    /// Channel for agent events (receiver shared with subscription).
    pub event_rx: Arc<Mutex<mpsc::UnboundedReceiver<AgentEvent>>>,
    /// Channel for agent events (sender given to agent adapter).
    pub event_tx: mpsc::UnboundedSender<AgentEvent>,
    /// Channel for permission responses.
    pub permission_tx: Option<mpsc::UnboundedSender<(String, PermissionResponse)>>,
    /// Cancel flag for agent tasks.
    pub cancel_flag: Arc<AtomicBool>,
    /// Is there new agent output that hasn't been rendered yet?
    pub is_dirty: bool,
    /// Session ID for conversation continuity (CLI manages its own history).
    pub session_id: Option<String>,
}

impl AgentState {
    /// Create a new agent state.
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            blocks: Vec::new(),
            block_index: HashMap::new(),
            active_block: None,
            event_rx: Arc::new(Mutex::new(event_rx)),
            event_tx,
            permission_tx: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            is_dirty: false,
            session_id: None,
        }
    }

    /// Allocate a new block ID using the terminal's counter.
    pub fn add_block(&mut self, block: AgentBlock) {
        let id = block.id;
        let idx = self.blocks.len();
        self.block_index.insert(id, idx);
        self.blocks.push(block);
    }

    /// Reset agent state, cancelling any active agent and clearing blocks.
    /// Used by ClearAll action.
    pub fn reset(&mut self) {
        use std::sync::atomic::Ordering;
        self.cancel_flag.store(true, Ordering::SeqCst);
        self.blocks.clear();
        self.block_index.clear();
        self.active_block = None;
        self.session_id = None; // Clear session to start fresh
    }
}

impl Default for AgentState {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Window Domain State
// =============================================================================

/// State for window management.
pub struct WindowState {
    /// Window ID for resize operations.
    pub id: Option<iced::window::Id>,
    /// Window dimensions in pixels.
    pub dims: (f32, f32),
    /// Current font size (mutable for zoom).
    pub font_size: f32,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            id: None,
            dims: (1200.0, 800.0),
            font_size: DEFAULT_FONT_SIZE,
        }
    }
}

impl WindowState {
    /// Get current character width based on font size.
    pub fn char_width(&self) -> f32 {
        self.font_size * CHAR_WIDTH_RATIO
    }

    /// Get current line height based on font size.
    pub fn line_height(&self) -> f32 {
        self.font_size * LINE_HEIGHT_FACTOR
    }

    /// Calculate terminal dimensions based on window size and font metrics.
    pub fn calculate_terminal_size(&self) -> (u16, u16) {
        let h_padding = 16.0;  // Minimal horizontal padding
        let v_padding = 60.0;  // Minimal vertical padding (input line + spacing)

        let (width, height) = self.dims;
        let cols = ((width - h_padding) / self.char_width()) as u16;
        let rows = ((height - v_padding) / self.line_height()) as u16;

        // Clamp to reasonable ranges
        (cols.max(40).min(500), rows.max(5).min(200))
    }
}

// =============================================================================
// Main Application State
// =============================================================================

/// The main Nexus application state.
/// Composes domain-specific state structs.
pub struct Nexus {
    /// Input area state (typing, completion, history).
    pub input: InputState,
    /// Terminal state (blocks, PTY, kernel).
    pub terminal: TerminalState,
    /// Agent state (AI interactions).
    pub agent: AgentState,
    /// Window state (dimensions, font).
    pub window: WindowState,
}

impl Nexus {
    /// Get current character width based on font size.
    pub fn char_width(&self) -> f32 {
        self.window.char_width()
    }

    /// Get current line height based on font size.
    pub fn line_height(&self) -> f32 {
        self.window.line_height()
    }

    /// Recalculate terminal dimensions based on window size and font metrics.
    pub fn recalculate_terminal_size(&mut self) -> (u16, u16) {
        let (cols, rows) = self.window.calculate_terminal_size();
        self.terminal.terminal_size = (cols, rows);
        (cols, rows)
    }

    /// Apply terminal resize to all blocks and PTYs.
    pub fn apply_resize(&mut self, cols: u16, rows: u16) {
        self.terminal.apply_resize(cols, rows);
    }

    // =========================================================================
    // Convenience accessors for backwards compatibility
    // =========================================================================

    /// Get the current working directory.
    pub fn cwd(&self) -> &str {
        &self.terminal.cwd
    }

    /// Get the current font size.
    pub fn font_size(&self) -> f32 {
        self.window.font_size
    }

    /// Check if there's dirty state needing render.
    /// Checks both terminal and agent dirty flags.
    pub fn is_dirty(&self) -> bool {
        self.terminal.is_dirty || self.agent.is_dirty
    }
}

impl Default for Nexus {
    fn default() -> Self {
        // Create kernel for pipeline execution
        let (kernel, kernel_rx) = Kernel::new().expect("Failed to create kernel");
        let kernel_tx = kernel.event_sender().clone();

        // Load command history from SQLite
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

        let mut input = InputState::default();
        input.shell_history = command_history;

        Self {
            input,
            terminal: TerminalState::new(kernel, kernel_tx, kernel_rx),
            agent: AgentState::new(),
            window: WindowState::default(),
        }
    }
}
