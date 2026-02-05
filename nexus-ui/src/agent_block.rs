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
    /// User interrupted (Escape). Partial response preserved.
    Interrupted,
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

/// A pending user question (from AskUserQuestion tool, awaiting JSONL surgery).
#[derive(Debug, Clone)]
pub struct PendingUserQuestion {
    /// Tool use ID for the AskUserQuestion call.
    pub tool_use_id: String,
    /// The questions to present to the user.
    pub questions: Vec<crate::agent_adapter::UserQuestion>,
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
    /// Pending user question (from AskUserQuestion tool via JSONL surgery).
    pub pending_question: Option<PendingUserQuestion>,
    /// Whether thinking section is collapsed.
    pub thinking_collapsed: bool,
    /// Cost in USD (from CLI result).
    pub cost_usd: Option<f64>,
    /// Input token count (from CLI result).
    pub input_tokens: Option<u64>,
    /// Output token count (from CLI result).
    pub output_tokens: Option<u64>,
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
            pending_question: None,
            thinking_collapsed: false,
            cost_usd: None,
            input_tokens: None,
            output_tokens: None,
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
            // Auto-collapse on success, force-expand on error
            match &status {
                ToolStatus::Success => { tool.collapsed = true; }
                ToolStatus::Error => { tool.collapsed = false; }
                _ => {}
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    // ========== ToolInvocation tests ==========

    #[test]
    fn test_tool_invocation_new() {
        let tool = ToolInvocation::new("tool-1".to_string(), "read_file".to_string());
        assert_eq!(tool.id, "tool-1");
        assert_eq!(tool.name, "read_file");
        assert!(tool.parameters.is_empty());
        assert!(tool.output.is_none());
        assert_eq!(tool.status, ToolStatus::Pending);
        assert!(tool.message.is_none());
        assert!(!tool.collapsed);
    }

    // ========== AgentBlock tests ==========

    #[test]
    fn test_agent_block_new() {
        let block = AgentBlock::new(BlockId(1), "What is Rust?".to_string());
        assert_eq!(block.id, BlockId(1));
        assert_eq!(block.query, "What is Rust?");
        assert!(block.thinking.is_empty());
        assert!(block.response.is_empty());
        assert!(block.tools.is_empty());
        assert!(block.active_tool_id.is_none());
        assert!(block.images.is_empty());
        assert_eq!(block.state, AgentBlockState::Pending);
        assert!(block.duration_ms.is_none());
        assert!(!block.thinking_collapsed);
        assert_eq!(block.version, 0);
    }

    #[test]
    fn test_agent_block_append_response() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        let v0 = block.version;

        block.append_response("Hello ");
        assert_eq!(block.response, "Hello ");
        assert_eq!(block.state, AgentBlockState::Streaming);
        assert!(block.version > v0);

        block.append_response("World!");
        assert_eq!(block.response, "Hello World!");
    }

    #[test]
    fn test_agent_block_append_thinking() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        let v0 = block.version;

        block.append_thinking("Let me think...");
        assert_eq!(block.thinking, "Let me think...");
        assert_eq!(block.state, AgentBlockState::Thinking);
        assert!(block.version > v0);
    }

    #[test]
    fn test_agent_block_start_tool() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        let v0 = block.version;

        block.start_tool("t1".to_string(), "read_file".to_string());

        assert_eq!(block.tools.len(), 1);
        assert_eq!(block.tools[0].id, "t1");
        assert_eq!(block.tools[0].name, "read_file");
        assert_eq!(block.active_tool_id, Some("t1".to_string()));
        assert_eq!(block.state, AgentBlockState::Executing);
        assert!(block.version > v0);
    }

    #[test]
    fn test_agent_block_add_tool_parameter() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        block.start_tool("t1".to_string(), "read_file".to_string());
        let v0 = block.version;

        block.add_tool_parameter("t1", "path".to_string(), "/test/".to_string());
        assert_eq!(block.tools[0].parameters.get("path"), Some(&"/test/".to_string()));
        assert!(block.version > v0);

        // Accumulate chunks for same parameter
        block.add_tool_parameter("t1", "path".to_string(), "file.txt".to_string());
        assert_eq!(block.tools[0].parameters.get("path"), Some(&"/test/file.txt".to_string()));
    }

    #[test]
    fn test_agent_block_add_tool_parameter_unknown_tool() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        let v0 = block.version;

        // Adding param to non-existent tool should not crash
        block.add_tool_parameter("unknown", "key".to_string(), "value".to_string());
        assert_eq!(block.version, v0); // Version unchanged
    }

    #[test]
    fn test_agent_block_update_tool_status_success() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        block.start_tool("t1".to_string(), "bash".to_string());

        block.update_tool_status(
            "t1",
            ToolStatus::Success,
            Some("Done".to_string()),
            Some("output".to_string()),
        );

        let tool = &block.tools[0];
        assert_eq!(tool.status, ToolStatus::Success);
        assert_eq!(tool.message, Some("Done".to_string()));
        assert_eq!(tool.output, Some("output".to_string()));
        assert!(tool.collapsed); // Auto-collapsed on success
        assert!(block.active_tool_id.is_none()); // Cleared on completion
    }

    #[test]
    fn test_agent_block_update_tool_status_error() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        block.start_tool("t1".to_string(), "bash".to_string());

        block.update_tool_status(
            "t1",
            ToolStatus::Error,
            Some("Failed".to_string()),
            None,
        );

        let tool = &block.tools[0];
        assert_eq!(tool.status, ToolStatus::Error);
        assert!(!tool.collapsed); // Force-expanded on error
    }

    #[test]
    fn test_agent_block_append_tool_output() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        block.start_tool("t1".to_string(), "bash".to_string());

        block.append_tool_output("t1", "line 1\n");
        assert_eq!(block.tools[0].output, Some("line 1\n".to_string()));

        block.append_tool_output("t1", "line 2\n");
        assert_eq!(block.tools[0].output, Some("line 1\nline 2\n".to_string()));
    }

    #[test]
    fn test_agent_block_add_image() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        let v0 = block.version;

        block.add_image("image/png".to_string(), "base64data".to_string());

        assert_eq!(block.images.len(), 1);
        assert_eq!(block.images[0].media_type, "image/png");
        assert_eq!(block.images[0].data, "base64data");
        assert!(block.version > v0);
    }

    #[test]
    fn test_agent_block_request_permission() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());

        let request = PermissionRequest {
            id: "p1".to_string(),
            tool_name: "bash".to_string(),
            tool_id: "t1".to_string(),
            description: "Execute command".to_string(),
            action: "rm -rf /".to_string(),
            working_dir: Some("/home".to_string()),
        };

        block.request_permission(request);

        assert!(block.pending_permission.is_some());
        assert_eq!(block.state, AgentBlockState::AwaitingPermission);
    }

    #[test]
    fn test_agent_block_clear_permission() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        block.request_permission(PermissionRequest {
            id: "p1".to_string(),
            tool_name: "bash".to_string(),
            tool_id: "t1".to_string(),
            description: "test".to_string(),
            action: "ls".to_string(),
            working_dir: None,
        });

        block.clear_permission();

        assert!(block.pending_permission.is_none());
        assert_eq!(block.state, AgentBlockState::Executing);
    }

    #[test]
    fn test_agent_block_complete() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        block.start_tool("t1".to_string(), "test".to_string());

        block.complete();

        assert_eq!(block.state, AgentBlockState::Completed);
        assert!(block.duration_ms.is_some());
        assert!(block.active_tool_id.is_none());
    }

    #[test]
    fn test_agent_block_fail() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        block.start_tool("t1".to_string(), "test".to_string());

        block.fail("Connection lost".to_string());

        assert_eq!(block.state, AgentBlockState::Failed("Connection lost".to_string()));
        assert!(block.duration_ms.is_some());
        assert!(block.active_tool_id.is_none());
    }

    #[test]
    fn test_agent_block_is_running() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());

        assert!(block.is_running()); // Pending

        block.state = AgentBlockState::Streaming;
        assert!(block.is_running());

        block.state = AgentBlockState::Thinking;
        assert!(block.is_running());

        block.state = AgentBlockState::Executing;
        assert!(block.is_running());

        block.state = AgentBlockState::AwaitingPermission;
        assert!(block.is_running());

        block.state = AgentBlockState::Completed;
        assert!(!block.is_running());

        block.state = AgentBlockState::Failed("err".to_string());
        assert!(!block.is_running());

        block.state = AgentBlockState::Interrupted;
        assert!(!block.is_running());
    }

    #[test]
    fn test_agent_block_toggle_thinking() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        assert!(!block.thinking_collapsed);

        block.toggle_thinking();
        assert!(block.thinking_collapsed);

        block.toggle_thinking();
        assert!(!block.thinking_collapsed);
    }

    #[test]
    fn test_agent_block_toggle_tool() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        block.start_tool("t1".to_string(), "read_file".to_string());
        assert!(!block.tools[0].collapsed);

        block.toggle_tool("t1");
        assert!(block.tools[0].collapsed);

        block.toggle_tool("t1");
        assert!(!block.tools[0].collapsed);
    }

    #[test]
    fn test_agent_block_toggle_tool_unknown() {
        let mut block = AgentBlock::new(BlockId(1), "test".to_string());
        let v0 = block.version;

        // Should not crash or change version for unknown tool
        block.toggle_tool("unknown");
        assert_eq!(block.version, v0);
    }

    #[test]
    fn test_agent_block_partial_eq_different_ids() {
        let block1 = AgentBlock::new(BlockId(1), "test".to_string());
        let block2 = AgentBlock::new(BlockId(2), "test".to_string());
        assert_ne!(block1, block2);
    }

    #[test]
    fn test_agent_block_partial_eq_running() {
        let block1 = AgentBlock::new(BlockId(1), "test".to_string());
        let block2 = AgentBlock::new(BlockId(1), "test".to_string());
        // Running blocks always return false
        assert_ne!(block1, block2);
    }

    #[test]
    fn test_agent_block_partial_eq_completed_same_version() {
        let mut block1 = AgentBlock::new(BlockId(1), "test".to_string());
        let mut block2 = AgentBlock::new(BlockId(1), "test".to_string());
        block1.state = AgentBlockState::Completed;
        block2.state = AgentBlockState::Completed;
        // Same version = equal
        assert_eq!(block1, block2);
    }

    #[test]
    fn test_agent_block_partial_eq_different_version() {
        let mut block1 = AgentBlock::new(BlockId(1), "test".to_string());
        let mut block2 = AgentBlock::new(BlockId(1), "test".to_string());
        block1.state = AgentBlockState::Completed;
        block2.state = AgentBlockState::Completed;
        block2.version = 5;
        assert_ne!(block1, block2);
    }
}
