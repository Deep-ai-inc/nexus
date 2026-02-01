//! Message types that drive the Nexus application update loop.
//!
//! Nested enum structure: each child component has its own message type,
//! wrapped by the root `NexusMessage` enum. Cross-cutting messages stay at root.

use std::path::PathBuf;

use nexus_api::BlockId;
use strata::content_address::ContentAddress;
use strata::event_context::KeyEvent;
use strata::{ScrollAction, TextInputMouseAction};

use crate::agent_adapter::AgentEvent;
use super::context_menu::{ContextMenuItem, ContextTarget};

// =========================================================================
// Root message
// =========================================================================

#[derive(Debug, Clone)]
pub enum NexusMessage {
    Input(InputMsg),
    Shell(ShellMsg),
    Agent(AgentMsg),
    Selection(SelectionMsg),

    // Cross-cutting (root handles directly)
    Scroll(ScrollAction),
    ContextMenu(ContextMenuMsg),
    Paste,
    Copy,
    ClearScreen,
    CloseWindow,
    BlurAll,
    Tick,
    ScrollToJob(u32),
    FileDrop(FileDropMsg),
    Drag(DragMsg),
}

/// File drop messages (from OS → Nexus).
#[derive(Debug, Clone)]
pub enum FileDropMsg {
    /// A file is being hovered over a drop zone.
    Hovered(PathBuf, DropZone),
    /// A file was dropped onto a drop zone.
    Dropped(PathBuf, DropZone),
    /// Hover left the window.
    HoverLeft,
    /// Async file read completed (for agent panel drops).
    FileLoaded(PathBuf, Vec<u8>),
    /// Async file read failed.
    FileLoadFailed(PathBuf, String),
}

/// Where a file drop is targeting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropZone {
    /// The input bar.
    InputBar,
    /// The agent panel.
    AgentPanel,
    /// A shell block.
    ShellBlock(BlockId),
    /// Empty area (fallback to input bar).
    Empty,
}

/// Internal drag-and-drop messages.
#[derive(Debug, Clone)]
pub enum DragMsg {
    /// Begin a pending interaction (mouse down on draggable/ambiguous element).
    Start(super::drag_state::PendingIntent, strata::primitives::Point),
    /// Begin text selection immediately (no hysteresis — raw text click).
    StartSelecting(ContentAddress, super::drag_state::SelectMode),
    /// Mouse moved past the 5px threshold — hand off to OS native drag.
    Activate(strata::primitives::Point),
    /// Drag cancelled (mouse released before threshold, or Escape).
    Cancel,
}

// =========================================================================
// Input component messages
// =========================================================================

#[derive(Debug, Clone)]
pub enum InputMsg {
    Key(KeyEvent),
    Mouse(TextInputMouseAction),
    Submit(String),
    ToggleMode,
    HistoryUp,
    HistoryDown,
    InsertNewline,
    RemoveAttachment(usize),

    // Completion
    TabComplete,
    CompletionNav(isize),
    CompletionAccept,
    CompletionDismiss,
    CompletionDismissAndForward(KeyEvent),
    CompletionSelect(usize),
    CompletionScroll(ScrollAction),

    // History search
    HistorySearchToggle,
    HistorySearchKey(KeyEvent),
    HistorySearchAccept,
    HistorySearchDismiss,
    HistorySearchSelect(usize),
    HistorySearchAcceptIndex(usize),
    HistorySearchScroll(ScrollAction),
}

// =========================================================================
// Shell component messages
// =========================================================================

#[derive(Debug, Clone)]
pub enum ShellMsg {
    PtyOutput(BlockId, Vec<u8>),
    PtyExited(BlockId, i32),
    /// Root resolves the target block and passes its ID.
    PtyInput(BlockId, KeyEvent),
    /// Root resolves the target block and passes its ID.
    SendInterrupt(BlockId),
    KernelEvent(nexus_api::ShellEvent),
    KillBlock(BlockId),
    SortTable(BlockId, usize),
    OpenAnchor(BlockId, AnchorAction),
}

/// Action to perform when a clickable anchor is activated.
#[derive(Debug, Clone)]
pub enum AnchorAction {
    /// Reveal a path in Finder (`open -R`).
    RevealPath(PathBuf),
    /// Open a URL in the default browser.
    OpenUrl(String),
    /// Copy a string to the clipboard (PID, git hash, etc.).
    CopyToClipboard(String),
}

// =========================================================================
// Agent component messages
// =========================================================================

#[derive(Debug, Clone)]
pub enum AgentMsg {
    Event(AgentEvent),
    ToggleThinking(BlockId),
    ToggleTool(BlockId, usize),
    PermissionGrant(BlockId, String),
    PermissionGrantSession(BlockId, String),
    PermissionDeny(BlockId, String),
    /// User answered an AskUserQuestion dialog. (block_id, tool_use_id, answer_json)
    UserQuestionAnswer(BlockId, String, String),
    /// Key event for the free-form question text input.
    QuestionInputKey(strata::event_context::KeyEvent),
    /// Mouse event for the free-form question text input.
    QuestionInputMouse(strata::text_input_state::TextInputMouseAction),
    Interrupt,
}

// =========================================================================
// Selection component messages
// =========================================================================

#[derive(Debug, Clone)]
pub enum SelectionMsg {
    Start(ContentAddress, super::drag_state::SelectMode),
    Extend(ContentAddress),
    End,
    Clear,
}

// =========================================================================
// Context menu messages
// =========================================================================

#[derive(Debug, Clone)]
pub enum ContextMenuMsg {
    Show(f32, f32, Vec<ContextMenuItem>, ContextTarget),
    Action(ContextMenuItem),
    Dismiss,
}
