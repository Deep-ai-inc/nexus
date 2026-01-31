//! Message types that drive the Nexus application update loop.
//!
//! Nested enum structure: each child component has its own message type,
//! wrapped by the root `NexusMessage` enum. Cross-cutting messages stay at root.

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
    PtyInput(KeyEvent),
    /// Root resolves the target block and passes its ID.
    SendInterrupt(BlockId),
    KernelEvent(nexus_api::ShellEvent),
    KillBlock(BlockId),
    SortTable(BlockId, usize),
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
    Interrupt,
}

// =========================================================================
// Selection component messages
// =========================================================================

#[derive(Debug, Clone)]
pub enum SelectionMsg {
    Start(ContentAddress),
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
