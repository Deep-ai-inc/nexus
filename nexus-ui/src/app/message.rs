//! Message types that drive the Nexus application update loop.
//!
//! Nested enum structure: each child component has its own message type,
//! wrapped by the root `NexusMessage` enum. Cross-cutting messages stay at root.

use std::path::PathBuf;

use nexus_api::BlockId;
use strata::content_address::ContentAddress;
use strata::event_context::KeyEvent;
use strata::{ScrollAction, TextInputMouseAction};

use crate::data::ProcSort;

use crate::features::agent::adapter::AgentEvent;
use crate::ui::context_menu::{ContextMenuItem, ContextTarget};

// =========================================================================
// Root message
// =========================================================================

#[derive(Debug, Clone)]
pub enum NexusMessage {
    Input(InputMsg),
    Shell(ShellMsg),
    Agent(AgentMsg),
    Selection(SelectionMsg),
    Viewer(ViewerMsg),

    // Cross-cutting (root handles directly)
    FocusBlock(BlockId),
    Scroll(ScrollAction),
    ContextMenu(ContextMenuMsg),
    Paste,
    Copy,
    ClearScreen,
    CloseWindow,
    NewWindow,
    QuitApp,
    BlurAll,
    Tick,
    ScrollToJob(u32),
    FileDrop(FileDropMsg),
    Drag(DragMsg),

    // Block navigation
    FocusPrevBlock,
    FocusNextBlock,
    FocusFirstBlock,
    FocusLastBlock,
    FocusAgentInput,
    TypeThrough(KeyEvent),

    // Zoom (stubs — rendering deferred)
    ZoomIn,
    ZoomOut,
    ZoomReset,

    /// Toggle debug layout visualization (Cmd+Shift+D in debug builds).
    #[cfg(debug_assertions)]
    ToggleDebugLayout,
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
    Start(crate::features::selection::drag::PendingIntent, strata::primitives::Point),
    /// Begin text selection immediately (no hysteresis — raw text click).
    StartSelecting(ContentAddress, crate::features::selection::drag::SelectMode),
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
    /// Batched PTY events (coalesced by the subscription to reduce render passes).
    PtyBatch(Vec<(BlockId, crate::data::PtyEvent)>),
    /// Root resolves the target block and passes its ID.
    PtyInput(BlockId, KeyEvent),
    /// Root resolves the target block and passes its ID.
    SendInterrupt(BlockId),
    KernelEvent(nexus_api::ShellEvent),
    KillBlock(BlockId),
    SortTable(BlockId, usize),
    OpenAnchor(BlockId, AnchorAction),
    /// Toggle directory expansion in tree view.
    ToggleTreeExpand(BlockId, PathBuf),
    /// Load tree children for an expanded directory.
    TreeChildrenLoaded(BlockId, PathBuf, Vec<nexus_api::FileEntry>),
}

/// Action to perform when a clickable anchor is activated.
#[derive(Debug, Clone)]
pub enum AnchorAction {
    /// Preview a file with Quick Look (`qlmanage -p`).
    QuickLook(PathBuf),
    /// Reveal a path in Finder (`open -R`).
    RevealPath(PathBuf),
    /// Open a file/URL with the default application (`open`).
    Open(PathBuf),
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
    /// Expand all collapsed tools in the most recent agent block (Ctrl+O).
    ExpandAllTools,
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
    Start(ContentAddress, crate::features::selection::drag::SelectMode),
    Extend(ContentAddress),
    End,
    Clear,
}

// =========================================================================
// Viewer messages (interactive block viewers)
// =========================================================================

#[derive(Debug, Clone)]
pub enum ViewerMsg {
    ScrollUp(BlockId),
    ScrollDown(BlockId),
    PageUp(BlockId),
    PageDown(BlockId),
    GoToTop(BlockId),
    GoToBottom(BlockId),
    SearchStart(BlockId),
    SearchNext(BlockId),
    SortBy(BlockId, ProcSort),
    TreeToggle(BlockId),
    TreeUp(BlockId),
    TreeDown(BlockId),
    DiffNextFile(BlockId),
    DiffPrevFile(BlockId),
    DiffToggleFile(BlockId),
    Exit(BlockId),
}

impl ViewerMsg {
    /// Extract the block ID from any viewer message.
    pub fn block_id(&self) -> BlockId {
        match self {
            ViewerMsg::ScrollUp(id)
            | ViewerMsg::ScrollDown(id)
            | ViewerMsg::PageUp(id)
            | ViewerMsg::PageDown(id)
            | ViewerMsg::GoToTop(id)
            | ViewerMsg::GoToBottom(id)
            | ViewerMsg::SearchStart(id)
            | ViewerMsg::SearchNext(id)
            | ViewerMsg::SortBy(id, _)
            | ViewerMsg::TreeToggle(id)
            | ViewerMsg::TreeUp(id)
            | ViewerMsg::TreeDown(id)
            | ViewerMsg::DiffNextFile(id)
            | ViewerMsg::DiffPrevFile(id)
            | ViewerMsg::DiffToggleFile(id)
            | ViewerMsg::Exit(id) => *id,
        }
    }
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
