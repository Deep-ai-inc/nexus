//! Block and related types for representing command execution in the UI.

use std::collections::HashSet;
use std::collections::VecDeque;
use std::time::Instant;

use nexus_api::{BlockId, BlockState, OutputFormat, Value};
use nexus_term::TerminalParser;

use crate::agent_block::AgentBlock;

/// Sort state for a table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableSort {
    /// Which column is being sorted (by index).
    pub column: Option<usize>,
    /// Sort direction (true = ascending, false = descending).
    pub ascending: bool,
}

impl TableSort {
    pub fn new() -> Self {
        Self::default()
    }

    /// Toggle sort on a column. If already sorting by this column, reverse direction.
    /// If sorting by a different column, start ascending.
    pub fn toggle(&mut self, column_index: usize) {
        if self.column == Some(column_index) {
            self.ascending = !self.ascending;
        } else {
            self.column = Some(column_index);
            self.ascending = true;
        }
    }
}

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
pub enum UnifiedBlockRef<'a> {
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
    /// Append-only event log (ping replies, etc.). Capped at 1000 entries.
    pub stream_log: VecDeque<Value>,
    /// Latest coalesced state (progress bar, live table, etc.).
    pub stream_latest: Option<Value>,
    /// Sequence counter for ordering streaming updates.
    pub stream_seq: u64,
    /// Interactive viewer state (pager, process monitor, tree browser).
    pub view_state: Option<ViewState>,
}

impl Block {
    pub fn new(id: BlockId, command: String) -> Self {
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
            stream_log: VecDeque::new(),
            stream_latest: None,
            stream_seq: 0,
            view_state: None,
        }
    }

    pub fn is_running(&self) -> bool {
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Focus {
    /// The command input field is focused.
    Input,
    /// A specific block is focused for interaction.
    Block(BlockId),
    /// The agent question text input is focused.
    AgentInput,
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

/// PTY event types for communication with the PTY subprocess.
#[derive(Debug, Clone)]
pub enum PtyEvent {
    Output(Vec<u8>),
    Exited(i32),
}

/// A job displayed in the status bar.
#[derive(Debug, Clone)]
pub struct VisualJob {
    pub id: u32,
    pub command: String,
    pub state: VisualJobState,
}

/// Visual state of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualJobState {
    Running,
    Stopped,
}

impl VisualJob {
    pub fn new(id: u32, command: String, state: VisualJobState) -> Self {
        Self { id, command, state }
    }

    /// Get a shortened display name for the job.
    pub fn display_name(&self) -> String {
        if self.command.len() > 20 {
            format!("{}...", &self.command[..17])
        } else {
            self.command.clone()
        }
    }

    /// Get the icon for this job state.
    pub fn icon(&self) -> &'static str {
        match self.state {
            VisualJobState::Running => "●",
            VisualJobState::Stopped => "⏸",
        }
    }
}

/// Interactive viewer state attached to a block.
#[derive(Debug)]
pub enum ViewState {
    Pager {
        scroll_line: usize,
        search: Option<String>,
        current_match: usize,
    },
    ProcessMonitor {
        sort_by: ProcSort,
        sort_desc: bool,
        interval_ms: u64,
    },
    TreeBrowser {
        collapsed: HashSet<usize>,
        selected: Option<usize>,
    },
}

impl ViewState {
    /// Map a key press to a viewer message. Returns None if the key is not handled.
    pub fn handle_key(
        &self,
        id: BlockId,
        key: &strata::event_context::Key,
    ) -> Option<crate::nexus_app::message::ViewerMsg> {
        use strata::event_context::{Key, NamedKey};
        use crate::nexus_app::message::ViewerMsg;

        match self {
            ViewState::Pager { .. } => match key {
                Key::Character(c) if c == "j" => Some(ViewerMsg::ScrollDown(id)),
                Key::Character(c) if c == "k" => Some(ViewerMsg::ScrollUp(id)),
                Key::Named(NamedKey::Space) => Some(ViewerMsg::PageDown(id)),
                Key::Character(c) if c == "b" => Some(ViewerMsg::PageUp(id)),
                Key::Character(c) if c == "g" => Some(ViewerMsg::GoToTop(id)),
                Key::Character(c) if c == "G" => Some(ViewerMsg::GoToBottom(id)),
                Key::Character(c) if c == "/" => Some(ViewerMsg::SearchStart(id)),
                Key::Character(c) if c == "n" => Some(ViewerMsg::SearchNext(id)),
                Key::Character(c) if c == "q" => Some(ViewerMsg::Exit(id)),
                _ => None,
            },
            ViewState::ProcessMonitor { .. } => match key {
                Key::Character(c) if c == "c" => Some(ViewerMsg::SortBy(id, ProcSort::Cpu)),
                Key::Character(c) if c == "m" => Some(ViewerMsg::SortBy(id, ProcSort::Mem)),
                Key::Character(c) if c == "p" => Some(ViewerMsg::SortBy(id, ProcSort::Pid)),
                Key::Character(c) if c == "q" => Some(ViewerMsg::Exit(id)),
                _ => None,
            },
            ViewState::TreeBrowser { .. } => match key {
                Key::Named(NamedKey::ArrowUp) => Some(ViewerMsg::TreeUp(id)),
                Key::Character(c) if c == "k" => Some(ViewerMsg::TreeUp(id)),
                Key::Named(NamedKey::ArrowDown) => Some(ViewerMsg::TreeDown(id)),
                Key::Character(c) if c == "j" => Some(ViewerMsg::TreeDown(id)),
                Key::Named(NamedKey::Space) | Key::Named(NamedKey::Enter) => {
                    Some(ViewerMsg::TreeToggle(id))
                }
                Key::Character(c) if c == "q" => Some(ViewerMsg::Exit(id)),
                _ => None,
            },
        }
    }
}

/// Sort criteria for process monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcSort {
    Cpu,
    Mem,
    Pid,
    Command,
}
