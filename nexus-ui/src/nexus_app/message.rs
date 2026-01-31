//! Message types that drive the Nexus application update loop.

use nexus_api::BlockId;
use strata::content_address::ContentAddress;
use strata::event_context::KeyEvent;
use strata::{ScrollAction, TextInputMouseAction};

use crate::agent_adapter::AgentEvent;
use super::context_menu::{ContextMenuItem, ContextTarget};

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
    KernelEvent(nexus_api::ShellEvent),
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
    /// Dismiss completion and forward the key event to normal input handling.
    CompletionDismissAndForward(KeyEvent),

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
