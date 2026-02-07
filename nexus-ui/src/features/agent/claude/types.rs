//! Serde types for the Claude Code CLI JSON stream protocol.

use std::path::PathBuf;

use serde::Deserialize;

// =============================================================================
// CLI Message Types (NDJSON protocol)
// =============================================================================

/// Top-level message from the Claude Code CLI stream.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum CliMessage {
    #[serde(rename = "system")]
    System(SystemMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "result")]
    Result(ResultMessage),
}

#[derive(Debug, Clone, Deserialize)]
pub struct SystemMessage {
    pub subtype: String,
    pub session_id: String,
    #[serde(default)]
    pub tools: Vec<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssistantMessage {
    pub message: AssistantMessageInner,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssistantMessageInner {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    /// Stop reason (optional, e.g. "end_turn", "tool_use").
    #[serde(default)]
    pub stop_reason: Option<String>,
}

/// Content block in an assistant or user message.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking { thinking: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        #[serde(default, deserialize_with = "deserialize_tool_content")]
        content: Option<String>,
        is_error: Option<bool>,
    },
    #[serde(rename = "server_tool_use")]
    ServerToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    #[serde(rename = "server_tool_result")]
    ServerToolResult {
        tool_use_id: String,
        #[serde(default, deserialize_with = "deserialize_tool_content")]
        content: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserMessage {
    pub message: UserMessageInner,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserMessageInner {
    #[serde(default)]
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResultMessage {
    /// The final text result.
    pub result: Option<String>,
    /// Structured output if requested.
    pub structured_output: Option<serde_json::Value>,
    /// Session ID for resumption.
    pub session_id: String,
    /// Cost in dollars.
    #[serde(alias = "total_cost_usd")]
    pub cost_usd: Option<f64>,
    /// Whether the session is resumable.
    pub is_resumable: Option<bool>,
    /// Total duration in ms.
    pub duration_ms: Option<u64>,
    /// Number of API turns.
    pub num_turns: Option<u32>,
    /// Token usage.
    pub usage: Option<TokenUsage>,
    /// Tools that were denied permission (includes AskUserQuestion in -p mode).
    #[serde(default)]
    pub permission_denials: Vec<PermissionDenial>,
}

/// A tool call that was denied permission by the CLI.
#[derive(Debug, Clone, Deserialize)]
pub struct PermissionDenial {
    pub tool_name: String,
    pub tool_use_id: String,
    pub tool_input: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_read_tokens: Option<u64>,
    pub cache_creation_tokens: Option<u64>,
}

// =============================================================================
// CLI Options
// =============================================================================

/// Options for spawning the Claude Code CLI.
#[derive(Debug, Clone, Default)]
pub struct CliOptions {
    /// Tools to allow without prompting.
    pub allowed_tools: Vec<String>,
    /// Tools to explicitly disallow.
    pub disallowed_tools: Vec<String>,
    /// Maximum number of agent turns.
    pub max_turns: Option<u32>,
    /// Model to use (defaults to Claude Sonnet).
    pub model: Option<String>,
    /// Session ID to resume.
    pub resume: Option<String>,
    /// Continue the most recent session.
    pub continue_session: bool,
    /// MCP server configuration file.
    pub mcp_config: Option<PathBuf>,
    /// Custom system prompt addition.
    pub append_system_prompt: Option<String>,
    /// Permission mode: default, acceptEdits, bypassPermissions
    pub permission_mode: Option<String>,
    /// MCP tool name for interactive permission prompts.
    pub permission_prompt_tool: Option<String>,
    /// Working directory.
    pub working_dir: Option<PathBuf>,
}

// =============================================================================
// Custom Deserializers
// =============================================================================

/// Deserialize tool result `content` which can be either a plain string
/// or an array of `[{"type":"text","text":"..."}]` content blocks.
fn deserialize_tool_content<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde_json::Value;
    let v = Option::<Value>::deserialize(deserializer)?;
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(Value::Array(arr)) => {
            // Concatenate all text blocks
            let mut out = String::new();
            for item in &arr {
                if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(text);
                }
            }
            Ok(if out.is_empty() { None } else { Some(out) })
        }
        Some(other) => Ok(Some(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -------------------------------------------------------------------------
    // deserialize_tool_content tests
    // -------------------------------------------------------------------------

    /// Helper struct to test the custom deserializer
    #[derive(Debug, Deserialize)]
    struct TestToolResult {
        #[serde(default, deserialize_with = "deserialize_tool_content")]
        content: Option<String>,
    }

    #[test]
    fn test_deserialize_tool_content_null() {
        let json = json!({"content": null});
        let result: TestToolResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.content, None);
    }

    #[test]
    fn test_deserialize_tool_content_missing() {
        let json = json!({});
        let result: TestToolResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.content, None);
    }

    #[test]
    fn test_deserialize_tool_content_string() {
        let json = json!({"content": "hello world"});
        let result: TestToolResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.content, Some("hello world".to_string()));
    }

    #[test]
    fn test_deserialize_tool_content_array_single() {
        let json = json!({
            "content": [{"type": "text", "text": "first block"}]
        });
        let result: TestToolResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.content, Some("first block".to_string()));
    }

    #[test]
    fn test_deserialize_tool_content_array_multiple() {
        let json = json!({
            "content": [
                {"type": "text", "text": "line one"},
                {"type": "text", "text": "line two"},
                {"type": "text", "text": "line three"}
            ]
        });
        let result: TestToolResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.content, Some("line one\nline two\nline three".to_string()));
    }

    #[test]
    fn test_deserialize_tool_content_array_empty() {
        let json = json!({"content": []});
        let result: TestToolResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.content, None);
    }

    #[test]
    fn test_deserialize_tool_content_array_no_text_field() {
        let json = json!({
            "content": [{"type": "image", "url": "http://example.com"}]
        });
        let result: TestToolResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.content, None);
    }

    #[test]
    fn test_deserialize_tool_content_number() {
        let json = json!({"content": 42});
        let result: TestToolResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.content, Some("42".to_string()));
    }

    #[test]
    fn test_deserialize_tool_content_object() {
        let json = json!({"content": {"key": "value"}});
        let result: TestToolResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.content, Some("{\"key\":\"value\"}".to_string()));
    }

    // -------------------------------------------------------------------------
    // CliMessage parsing tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_system_message() {
        let json = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "sess-123",
            "tools": ["Read", "Write", "Bash"],
            "model": "claude-sonnet"
        });
        let msg: CliMessage = serde_json::from_value(json).unwrap();
        if let CliMessage::System(sys) = msg {
            assert_eq!(sys.session_id, "sess-123");
            assert_eq!(sys.subtype, "init");
            assert_eq!(sys.tools, vec!["Read", "Write", "Bash"]);
            assert_eq!(sys.model, Some("claude-sonnet".to_string()));
        } else {
            panic!("Expected System message");
        }
    }

    #[test]
    fn test_parse_assistant_message_text() {
        let json = json!({
            "type": "assistant",
            "message": {
                "content": [
                    {"type": "text", "text": "Hello, world!"}
                ],
                "id": "msg-1",
                "stop_reason": "end_turn"
            }
        });
        let msg: CliMessage = serde_json::from_value(json).unwrap();
        if let CliMessage::Assistant(asst) = msg {
            assert_eq!(asst.message.content.len(), 1);
            if let ContentBlock::Text { text } = &asst.message.content[0] {
                assert_eq!(text, "Hello, world!");
            } else {
                panic!("Expected Text block");
            }
        } else {
            panic!("Expected Assistant message");
        }
    }

    #[test]
    fn test_parse_assistant_message_tool_use() {
        let json = json!({
            "type": "assistant",
            "message": {
                "content": [
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "Read",
                        "input": {"file_path": "/tmp/test.txt"}
                    }
                ]
            }
        });
        let msg: CliMessage = serde_json::from_value(json).unwrap();
        if let CliMessage::Assistant(asst) = msg {
            if let ContentBlock::ToolUse { id, name, input } = &asst.message.content[0] {
                assert_eq!(id, "tool-1");
                assert_eq!(name, "Read");
                assert_eq!(input.get("file_path").unwrap().as_str().unwrap(), "/tmp/test.txt");
            } else {
                panic!("Expected ToolUse block");
            }
        } else {
            panic!("Expected Assistant message");
        }
    }

    #[test]
    fn test_parse_result_message() {
        let json = json!({
            "type": "result",
            "session_id": "sess-456",
            "result": "Task completed",
            "cost_usd": 0.05,
            "duration_ms": 1234,
            "num_turns": 3,
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 500
            }
        });
        let msg: CliMessage = serde_json::from_value(json).unwrap();
        if let CliMessage::Result(result) = msg {
            assert_eq!(result.session_id, "sess-456");
            assert_eq!(result.result, Some("Task completed".to_string()));
            assert_eq!(result.cost_usd, Some(0.05));
            assert_eq!(result.duration_ms, Some(1234));
            assert_eq!(result.num_turns, Some(3));
            assert_eq!(result.usage.as_ref().unwrap().input_tokens, Some(1000));
            assert_eq!(result.usage.as_ref().unwrap().output_tokens, Some(500));
        } else {
            panic!("Expected Result message");
        }
    }

    #[test]
    fn test_parse_thinking_block() {
        let json = json!({
            "type": "assistant",
            "message": {
                "content": [
                    {"type": "thinking", "thinking": "Let me think about this..."}
                ]
            }
        });
        let msg: CliMessage = serde_json::from_value(json).unwrap();
        if let CliMessage::Assistant(asst) = msg {
            if let ContentBlock::Thinking { thinking } = &asst.message.content[0] {
                assert_eq!(thinking, "Let me think about this...");
            } else {
                panic!("Expected Thinking block");
            }
        } else {
            panic!("Expected Assistant message");
        }
    }

    #[test]
    fn test_parse_tool_result_with_content_array() {
        let json = json!({
            "type": "user",
            "message": {
                "content": [
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": [
                            {"type": "text", "text": "File contents here"}
                        ],
                        "is_error": false
                    }
                ]
            }
        });
        let msg: CliMessage = serde_json::from_value(json).unwrap();
        if let CliMessage::User(user) = msg {
            if let ContentBlock::ToolResult { tool_use_id, content, is_error } = &user.message.content[0] {
                assert_eq!(tool_use_id, "tool-1");
                assert_eq!(content, &Some("File contents here".to_string()));
                assert_eq!(*is_error, Some(false));
            } else {
                panic!("Expected ToolResult block");
            }
        } else {
            panic!("Expected User message");
        }
    }

    // -------------------------------------------------------------------------
    // CliOptions tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cli_options_default() {
        let opts = CliOptions::default();
        assert!(opts.allowed_tools.is_empty());
        assert!(opts.disallowed_tools.is_empty());
        assert!(opts.max_turns.is_none());
        assert!(opts.model.is_none());
        assert!(opts.resume.is_none());
        assert!(!opts.continue_session);
    }
}
