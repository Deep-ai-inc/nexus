//! Message types for the Nexus application.
//!
//! Messages are organized into domains for better encapsulation:
//! - `InputMessage`: Text input, completion, history search
//! - `TerminalMessage`: PTY/Kernel execution, blocks, tables
//! - `AgentMessage`: AI agent interactions
//! - `WindowMessage`: Window management, shortcuts, zoom

use std::time::Instant;

use iced::keyboard::{Key, Modifiers};
use iced::widget::text_editor;
use iced::Event;

use nexus_api::{BlockId, ShellEvent, Value};

use crate::agent_adapter::AgentEvent;
use crate::agent_widgets::AgentWidgetMessage;

// =============================================================================
// Top-Level Message (Router)
// =============================================================================

/// Top-level messages for the Nexus application.
/// This enum routes to domain-specific handlers.
#[derive(Debug, Clone)]
pub enum Message {
    /// Input domain: typing, completion, history search.
    Input(InputMessage),
    /// Terminal domain: PTY, kernel, blocks.
    Terminal(TerminalMessage),
    /// Agent domain: AI agent interactions.
    Agent(AgentMessage),
    /// Window domain: resize, zoom, shortcuts.
    Window(WindowMessage),
    /// Render loop tick (VSync-aligned frame).
    Tick(Instant),
}

// =============================================================================
// Input Domain
// =============================================================================

/// Messages related to text input, completion, and history.
#[derive(Debug, Clone)]
pub enum InputMessage {
    /// Editor action (typing, cursor movement, etc.).
    EditorAction(text_editor::Action),
    /// User submitted a command (Enter key).
    Submit,
    /// Tab key pressed - trigger completion (forward).
    TabCompletion,
    /// Shift+Tab pressed - cycle completion backwards.
    TabCompletionPrev,
    /// Select a completion item by index (applies the completion).
    SelectCompletion(usize),
    /// Navigate to a completion item (changes selection without applying).
    CompletionNavigate(usize),
    /// Cancel completion popup (Escape).
    CancelCompletion,
    /// Arrow key for history navigation.
    HistoryKey(Key, Modifiers),
    /// Start history search (Ctrl+R).
    HistorySearchStart,
    /// History search query changed.
    HistorySearchChanged(String),
    /// Select a history search result.
    HistorySearchSelect(usize),
    /// Cancel history search (Escape).
    HistorySearchCancel,
    /// Toggle between Shell and Agent input modes.
    ToggleMode,
    /// An image was pasted from clipboard.
    PasteImage(Vec<u8>, u32, u32),
    /// Remove an attachment by index.
    RemoveAttachment(usize),
}

// =============================================================================
// Terminal Domain
// =============================================================================

/// Messages related to terminal/PTY and kernel execution.
#[derive(Debug, Clone)]
pub enum TerminalMessage {
    /// PTY output received.
    PtyOutput(BlockId, Vec<u8>),
    /// PTY exited with code.
    PtyExited(BlockId, i32),
    /// Keyboard event when a block is focused.
    KeyPressed(Key, Modifiers),
    /// Kernel event (from pipeline execution).
    KernelEvent(ShellEvent),
    /// Sort table by column in a specific block.
    TableSort(BlockId, usize),
    /// User clicked a cell in a table.
    TableCellClick(BlockId, usize, usize, Value),
    /// User clicked a job in the status bar.
    JobClicked(u32),
    /// Retry last command with sudo.
    RetryWithSudo,
    /// Dismiss the permission denied prompt.
    DismissPermissionPrompt,
    /// Run a suggested command (command not found recovery).
    RunSuggestedCommand(String),
    /// Dismiss the command not found prompt.
    DismissCommandNotFound,
    /// Force kill a running PTY block.
    KillBlock(BlockId),
}

// =============================================================================
// Agent Domain
// =============================================================================

/// Messages related to the AI agent.
#[derive(Debug, Clone)]
pub enum AgentMessage {
    /// Agent event received from the agent adapter.
    Event(AgentEvent),
    /// Agent widget interaction (toggle, permission response, etc.)
    Widget(AgentWidgetMessage),
    /// Interrupt the current agent (Escape key). Preserves partial response.
    Interrupt,
    /// Cancel the current agent operation (hard stop).
    Cancel,
}

// =============================================================================
// Window Domain
// =============================================================================

/// Messages related to window management and global actions.
#[derive(Debug, Clone)]
pub enum WindowMessage {
    /// Generic event (for subscription) with window ID.
    Event(Event, iced::window::Id),
    /// Window resized.
    Resized(u32, u32),
    /// Global keyboard shortcut (Cmd+K, Cmd+Q, etc.)
    Shortcut(GlobalShortcut),
    /// Zoom font size.
    Zoom(ZoomDirection),
    /// Click on background - refocus input.
    BackgroundClicked,
}

// =============================================================================
// Supporting Types
// =============================================================================

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

// =============================================================================
// Cross-Domain Actions
// =============================================================================

/// Actions that require cross-domain coordination.
/// Returned by handlers when they need to affect other domains.
#[derive(Debug, Clone)]
pub enum Action {
    /// Execute a shell command through the terminal.
    ExecuteCommand(String),
    /// Spawn an agent query.
    SpawnAgentQuery(String),
    /// Clear all blocks (terminal and agent).
    ClearAll,
    /// Set focus to input.
    FocusInput,
}

// =============================================================================
// Convenience Constructors
// =============================================================================

impl Message {
    /// Create an input message.
    pub fn input(msg: InputMessage) -> Self {
        Message::Input(msg)
    }

    /// Create a terminal message.
    pub fn terminal(msg: TerminalMessage) -> Self {
        Message::Terminal(msg)
    }

    /// Create an agent message.
    pub fn agent(msg: AgentMessage) -> Self {
        Message::Agent(msg)
    }

    /// Create a window message.
    pub fn window(msg: WindowMessage) -> Self {
        Message::Window(msg)
    }
}
