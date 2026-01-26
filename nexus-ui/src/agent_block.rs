//! Agent block - represents an agent conversation turn in the UI.
//!
//! An AgentBlock contains:
//! - User's query
//! - Agent's thinking/reasoning
//! - Tool invocations with parameters and results
//! - Final response text
//! - Any images or media

use nexus_api::BlockId;
use std::collections::HashMap;
use std::time::Instant;

/// Status of a tool invocation.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolStatus {
    /// Tool is being invoked, parameters streaming in.
    Pending,
    /// Tool is actively running.
    Running,
    /// Tool completed successfully.
    Success,
    /// Tool failed with error.
    Error,
}

/// A single tool invocation within an agent turn.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    /// Unique ID for this tool call.
    pub id: String,
    /// Tool name (e.g., "read_file", "execute_command").
    pub name: String,
    /// Parameters as key-value pairs.
    pub parameters: HashMap<String, String>,
    /// Tool output (if completed).
    pub output: Option<String>,
    /// Status of the tool.
    pub status: ToolStatus,
    /// Short status message.
    pub message: Option<String>,
    /// Whether the tool UI is collapsed.
    pub collapsed: bool,
}

impl ToolInvocation {
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            parameters: HashMap::new(),
            output: None,
            status: ToolStatus::Pending,
            message: None,
            collapsed: false,
        }
    }
}

/// State of an agent block.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentBlockState {
    /// Waiting for agent response.
    Pending,
    /// Agent is streaming a response.
    Streaming,
    /// Agent is thinking/reasoning.
    Thinking,
    /// Agent is executing tools.
    Executing,
    /// Agent finished successfully.
    Completed,
    /// Agent encountered an error.
    Failed(String),
    /// Waiting for user permission.
    AwaitingPermission,
}

/// A permission request from the agent.
#[derive(Debug, Clone)]
pub struct PermissionRequest {
    /// Unique ID for this permission request.
    pub id: String,
    /// Tool requesting permission.
    pub tool_name: String,
    /// Tool invocation ID.
    pub tool_id: String,
    /// Description of what's being requested.
    pub description: String,
    /// The command or action being requested.
    pub action: String,
    /// Working directory (for commands).
    pub working_dir: Option<String>,
}

/// An agent conversation turn (query + response).
#[derive(Debug, Clone)]
pub struct AgentBlock {
    /// Block ID for the UI.
    pub id: BlockId,
    /// Original user query.
    pub query: String,
    /// Thinking/reasoning text (may be streaming).
    pub thinking: String,
    /// Response text (may be streaming).
    pub response: String,
    /// Tool invocations in this turn.
    pub tools: Vec<ToolInvocation>,
    /// Active tool ID (currently executing).
    pub active_tool_id: Option<String>,
    /// Images in the response.
    pub images: Vec<AgentImage>,
    /// Current state.
    pub state: AgentBlockState,
    /// When the query started.
    pub started_at: Instant,
    /// Duration in milliseconds (when completed).
    pub duration_ms: Option<u64>,
    /// Pending permission request.
    pub pending_permission: Option<PermissionRequest>,
    /// Whether thinking section is collapsed.
    pub thinking_collapsed: bool,
    /// Version counter for lazy invalidation.
    pub version: u64,
}

/// An image in an agent response.
#[derive(Debug, Clone)]
pub struct AgentImage {
    /// Media type (e.g., "image/png").
    pub media_type: String,
    /// Base64-encoded image data.
    pub data: String,
}

impl AgentBlock {
    /// Create a new agent block for a query.
    pub fn new(id: BlockId, query: String) -> Self {
        Self {
            id,
            query,
            thinking: String::new(),
            response: String::new(),
            tools: Vec::new(),
            active_tool_id: None,
            images: Vec::new(),
            state: AgentBlockState::Pending,
            started_at: Instant::now(),
            duration_ms: None,
            pending_permission: None,
            thinking_collapsed: false,
            version: 0,
        }
    }

    /// Append text to the response.
    pub fn append_response(&mut self, text: &str) {
        self.response.push_str(text);
        self.state = AgentBlockState::Streaming;
        self.version += 1;
    }

    /// Append text to thinking.
    pub fn append_thinking(&mut self, text: &str) {
        self.thinking.push_str(text);
        self.state = AgentBlockState::Thinking;
        self.version += 1;
    }

    /// Start a tool invocation.
    pub fn start_tool(&mut self, id: String, name: String) {
        let tool = ToolInvocation::new(id.clone(), name);
        self.tools.push(tool);
        self.active_tool_id = Some(id);
        self.state = AgentBlockState::Executing;
        self.version += 1;
    }

    /// Add a parameter to the active tool.
    /// Parameters with the same name are accumulated (for streaming chunks).
    pub fn add_tool_parameter(&mut self, tool_id: &str, name: String, value: String) {
        if let Some(tool) = self.tools.iter_mut().find(|t| t.id == tool_id) {
            // Accumulate values for same parameter name (streaming sends chunks)
            tool.parameters
                .entry(name)
                .and_modify(|v| v.push_str(&value))
                .or_insert(value);
            self.version += 1;
        }
    }

    /// Update tool status.
    pub fn update_tool_status(
        &mut self,
        tool_id: &str,
        status: ToolStatus,
        message: Option<String>,
        output: Option<String>,
    ) {
        if let Some(tool) = self.tools.iter_mut().find(|t| t.id == tool_id) {
            tool.status = status;
            tool.message = message;
            if let Some(out) = output {
                tool.output = Some(out);
            }
            self.version += 1;
        }
        // Clear active tool if completed
        if self.active_tool_id.as_deref() == Some(tool_id) {
            self.active_tool_id = None;
        }
    }

    /// Append output to a tool.
    pub fn append_tool_output(&mut self, tool_id: &str, chunk: &str) {
        if let Some(tool) = self.tools.iter_mut().find(|t| t.id == tool_id) {
            let output = tool.output.get_or_insert_with(String::new);
            output.push_str(chunk);
            self.version += 1;
        }
    }

    /// Add an image to the response.
    pub fn add_image(&mut self, media_type: String, data: String) {
        self.images.push(AgentImage { media_type, data });
        self.version += 1;
    }

    /// Request permission for an action.
    pub fn request_permission(&mut self, request: PermissionRequest) {
        self.pending_permission = Some(request);
        self.state = AgentBlockState::AwaitingPermission;
        self.version += 1;
    }

    /// Clear the permission request (after user response).
    pub fn clear_permission(&mut self) {
        self.pending_permission = None;
        self.state = AgentBlockState::Executing;
        self.version += 1;
    }

    /// Mark the block as completed.
    pub fn complete(&mut self) {
        self.state = AgentBlockState::Completed;
        self.duration_ms = Some(self.started_at.elapsed().as_millis() as u64);
        self.active_tool_id = None;
        self.version += 1;
    }

    /// Mark the block as failed.
    pub fn fail(&mut self, error: String) {
        self.state = AgentBlockState::Failed(error);
        self.duration_ms = Some(self.started_at.elapsed().as_millis() as u64);
        self.active_tool_id = None;
        self.version += 1;
    }

    /// Check if the block is still processing.
    pub fn is_running(&self) -> bool {
        matches!(
            self.state,
            AgentBlockState::Pending
                | AgentBlockState::Streaming
                | AgentBlockState::Thinking
                | AgentBlockState::Executing
                | AgentBlockState::AwaitingPermission
        )
    }

    /// Toggle thinking collapsed state.
    pub fn toggle_thinking(&mut self) {
        self.thinking_collapsed = !self.thinking_collapsed;
        self.version += 1;
    }

    /// Toggle tool collapsed state.
    pub fn toggle_tool(&mut self, tool_id: &str) {
        if let Some(tool) = self.tools.iter_mut().find(|t| t.id == tool_id) {
            tool.collapsed = !tool.collapsed;
            self.version += 1;
        }
    }
}

impl PartialEq for AgentBlock {
    fn eq(&self, other: &Self) -> bool {
        if self.id != other.id {
            return false;
        }
        // Running blocks always need redrawing
        if self.is_running() {
            return false;
        }
        self.version == other.version
    }
}
