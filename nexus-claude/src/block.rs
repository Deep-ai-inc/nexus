//! Claude conversation blocks and state management.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::path::PathBuf;

/// A block in the Claude conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClaudeBlock {
    /// User prompt input.
    UserPrompt {
        text: String,
        timestamp: u64,
    },

    /// Claude's thinking/reasoning process.
    Thinking {
        content: String,
        /// Whether the block is expanded in the UI.
        expanded: bool,
        /// Whether content is still streaming in.
        streaming: bool,
    },

    /// Claude's assistant message response.
    AssistantMessage {
        markdown: String,
        /// Whether content is still streaming in.
        streaming: bool,
    },

    /// Tool execution (read_file, write_file, bash, etc.).
    ToolExecution {
        tool_name: String,
        tool_id: String,
        input: Option<JsonValue>,
        status: ToolStatus,
        output: Option<JsonValue>,
    },

    /// Code diff proposal (for file edits).
    CodeDiff {
        file_path: String,
        old_content: Option<String>,
        new_content: String,
        status: DiffStatus,
    },

    /// Permission request from Claude.
    PermissionRequest {
        tool: String,
        description: String,
        response: Option<bool>,
    },

    /// System message (errors, warnings, info).
    SystemMessage {
        level: MessageLevel,
        content: String,
    },
}

/// Status of a tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolStatus {
    Pending,
    Running,
    Success,
    Error,
}

/// Status of a code diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffStatus {
    /// Diff is proposed but not yet acted upon.
    Proposed,
    /// User approved the diff.
    Approved,
    /// Diff has been applied to the file.
    Applied,
    /// User rejected the diff.
    Rejected,
}

/// Severity level of a system message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageLevel {
    Info,
    Warning,
    Error,
}

/// State of the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConversationState {
    #[default]
    Idle,
    Thinking,
    Responding,
    WaitingForPermission,
    ExecutingTool(u32), // tool index
}

/// A file in Claude's context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextFile {
    pub path: String,
    pub added_at: u64,
    /// Whether the user explicitly added this file.
    pub user_added: bool,
}

/// The complete Claude conversation state.
#[derive(Debug, Clone)]
pub struct ClaudeConversation {
    /// All blocks in the conversation.
    pub blocks: Vec<ClaudeBlock>,

    /// Current conversation state.
    pub state: ConversationState,

    /// Current working directory.
    pub cwd: PathBuf,

    /// Files currently in Claude's context.
    pub context_files: Vec<ContextFile>,

    /// Session ID for the Claude process.
    pub session_id: Option<String>,
}

impl Default for ClaudeConversation {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeConversation {
    /// Create a new empty conversation.
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            state: ConversationState::Idle,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")),
            context_files: Vec::new(),
            session_id: None,
        }
    }

    /// Create a conversation with a specific working directory.
    pub fn with_cwd(cwd: PathBuf) -> Self {
        Self {
            blocks: Vec::new(),
            state: ConversationState::Idle,
            cwd,
            context_files: Vec::new(),
            session_id: None,
        }
    }

    /// Add a user prompt block.
    pub fn add_user_prompt(&mut self, text: String) {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.blocks.push(ClaudeBlock::UserPrompt { text, timestamp });
    }

    /// Start a new thinking block.
    pub fn start_thinking(&mut self) {
        self.blocks.push(ClaudeBlock::Thinking {
            content: String::new(),
            expanded: false,
            streaming: true,
        });
        self.state = ConversationState::Thinking;
    }

    /// Append content to the current thinking block.
    pub fn append_thinking(&mut self, content: &str) {
        if let Some(ClaudeBlock::Thinking {
            content: c,
            streaming: true,
            ..
        }) = self.blocks.last_mut()
        {
            c.push_str(content);
        }
    }

    /// End the current thinking block.
    pub fn end_thinking(&mut self) {
        if let Some(ClaudeBlock::Thinking {
            streaming, ..
        }) = self.blocks.last_mut()
        {
            *streaming = false;
        }
    }

    /// Start a new assistant message block.
    pub fn start_assistant_message(&mut self) {
        self.blocks.push(ClaudeBlock::AssistantMessage {
            markdown: String::new(),
            streaming: true,
        });
        self.state = ConversationState::Responding;
    }

    /// Append content to the current assistant message.
    pub fn append_assistant_message(&mut self, content: &str) {
        if let Some(ClaudeBlock::AssistantMessage {
            markdown: m,
            streaming: true,
            ..
        }) = self.blocks.last_mut()
        {
            m.push_str(content);
        }
    }

    /// End the current assistant message block.
    pub fn end_assistant_message(&mut self) {
        if let Some(ClaudeBlock::AssistantMessage {
            streaming, ..
        }) = self.blocks.last_mut()
        {
            *streaming = false;
        }
    }

    /// Start a tool execution.
    pub fn start_tool(&mut self, tool_name: String, tool_id: String) {
        self.blocks.push(ClaudeBlock::ToolExecution {
            tool_name,
            tool_id,
            input: None,
            status: ToolStatus::Pending,
            output: None,
        });
    }

    /// Set tool input.
    pub fn set_tool_input(&mut self, tool_id: &str, input: JsonValue) {
        for block in self.blocks.iter_mut().rev() {
            if let ClaudeBlock::ToolExecution {
                tool_id: id,
                input: inp,
                status,
                ..
            } = block
            {
                if id == tool_id {
                    *inp = Some(input);
                    *status = ToolStatus::Running;
                    return;
                }
            }
        }
    }

    /// Set tool result.
    pub fn set_tool_result(&mut self, tool_id: &str, result: JsonValue, is_error: bool) {
        for block in self.blocks.iter_mut().rev() {
            if let ClaudeBlock::ToolExecution {
                tool_id: id,
                output: out,
                status,
                ..
            } = block
            {
                if id == tool_id {
                    *out = Some(result);
                    *status = if is_error {
                        ToolStatus::Error
                    } else {
                        ToolStatus::Success
                    };
                    return;
                }
            }
        }
    }

    /// Add a context file.
    pub fn add_context_file(&mut self, path: String, user_added: bool) {
        // Avoid duplicates
        if self.context_files.iter().any(|f| f.path == path) {
            return;
        }

        let added_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        self.context_files.push(ContextFile {
            path,
            added_at,
            user_added,
        });
    }

    /// Remove a context file.
    pub fn remove_context_file(&mut self, path: &str) {
        self.context_files.retain(|f| f.path != path);
    }

    /// Check if the conversation is currently streaming.
    pub fn is_streaming(&self) -> bool {
        !matches!(self.state, ConversationState::Idle)
    }

    /// Set conversation to idle state.
    pub fn set_idle(&mut self) {
        self.state = ConversationState::Idle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_creation() {
        let conv = ClaudeConversation::new();
        assert!(conv.blocks.is_empty());
        assert_eq!(conv.state, ConversationState::Idle);
    }

    #[test]
    fn test_user_prompt() {
        let mut conv = ClaudeConversation::new();
        conv.add_user_prompt("Hello".to_string());
        assert_eq!(conv.blocks.len(), 1);
        if let ClaudeBlock::UserPrompt { text, .. } = &conv.blocks[0] {
            assert_eq!(text, "Hello");
        } else {
            panic!("Expected UserPrompt block");
        }
    }

    #[test]
    fn test_thinking_flow() {
        let mut conv = ClaudeConversation::new();
        conv.start_thinking();
        assert_eq!(conv.state, ConversationState::Thinking);

        conv.append_thinking("Part 1");
        conv.append_thinking(" Part 2");

        if let Some(ClaudeBlock::Thinking { content, streaming, .. }) = conv.blocks.last() {
            assert_eq!(content, "Part 1 Part 2");
            assert!(*streaming);
        }

        conv.end_thinking();

        if let Some(ClaudeBlock::Thinking { streaming, .. }) = conv.blocks.last() {
            assert!(!*streaming);
        }
    }

    #[test]
    fn test_context_files() {
        let mut conv = ClaudeConversation::new();
        conv.add_context_file("/path/to/file.rs".to_string(), false);
        assert_eq!(conv.context_files.len(), 1);

        // Adding duplicate should be a no-op
        conv.add_context_file("/path/to/file.rs".to_string(), false);
        assert_eq!(conv.context_files.len(), 1);

        conv.remove_context_file("/path/to/file.rs");
        assert!(conv.context_files.is_empty());
    }
}
