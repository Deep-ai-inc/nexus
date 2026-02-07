//! Claude Code CLI wrapper.
//!
//! This module provides a Rust interface to the Claude Code CLI, giving us access to
//! all of Claude Code's capabilities: the system prompt, context compaction, subagents,
//! MCP integration, session management, and permissions.
//!
//! Instead of reimplementing the agent loop, we spawn the CLI with `--output-format stream-json`
//! and parse its NDJSON output stream.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Deserialize;
use tokio::sync::mpsc;

use super::events::AgentEvent;
use crate::data::agent_block::ToolStatus;

// =============================================================================
// Helpers
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

// =============================================================================
// CLI Message Types (from Claude Code's stream-json output)
// =============================================================================

/// Top-level message from Claude Code CLI.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CliMessage {
    /// System message - session initialization.
    System(SystemMessage),
    /// Assistant message - Claude's response.
    Assistant(AssistantMessage),
    /// User message - tool results (we don't usually see these).
    User(UserMessage),
    /// Result message - query completion.
    Result(ResultMessage),
}

#[derive(Debug, Clone, Deserialize)]
pub struct SystemMessage {
    pub subtype: String, // "init"
    pub session_id: String,
    /// List of available tool names.
    #[serde(default)]
    pub tools: Vec<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub mcp_servers: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssistantMessage {
    /// The inner message object from the API.
    pub message: AssistantMessageInner,
    /// Session ID for this message.
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssistantMessageInner {
    /// Message content blocks.
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    /// Unique message ID.
    pub id: Option<String>,
    /// Stop reason (end_turn, tool_use, etc).
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Text content from Claude.
    Text {
        text: String,
    },
    /// Extended thinking block.
    Thinking {
        thinking: String,
    },
    /// Tool use request.
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Tool result (in user messages).
    ToolResult {
        tool_use_id: String,
        /// Content can be a plain string or an array of {type:"text",text:"..."} objects.
        #[serde(default, deserialize_with = "deserialize_tool_content")]
        content: Option<String>,
        is_error: Option<bool>,
    },
    /// Server tool use (MCP).
    ServerToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Server tool result.
    ServerToolResult {
        tool_use_id: String,
        #[serde(default, deserialize_with = "deserialize_tool_content")]
        content: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserMessage {
    /// The inner message object from the API.
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
// Claude CLI Wrapper
// =============================================================================

/// Wrapper around the Claude Code CLI process.
pub struct ClaudeCli {
    child: Child,
    session_id: Option<String>,
    cancel_flag: Arc<AtomicBool>,
}

impl ClaudeCli {
    /// Spawn a new Claude Code CLI process with the given prompt and options.
    pub fn spawn(prompt: &str, options: CliOptions) -> std::io::Result<Self> {
        let mut cmd = Command::new("claude");

        // Basic flags for non-interactive streaming
        cmd.args(["-p", prompt]);
        cmd.args(["--output-format", "stream-json"]);
        cmd.arg("--verbose");

        // Tool permissions
        if !options.allowed_tools.is_empty() {
            cmd.args(["--allowedTools", &options.allowed_tools.join(",")]);
        }
        if !options.disallowed_tools.is_empty() {
            cmd.args(["--disallowedTools", &options.disallowed_tools.join(",")]);
        }

        // Max turns
        if let Some(max) = options.max_turns {
            cmd.args(["--max-turns", &max.to_string()]);
        }

        // Model selection
        if let Some(model) = &options.model {
            cmd.args(["--model", model]);
        }

        // Session management
        if let Some(session_id) = &options.resume {
            cmd.args(["--resume", session_id]);
        } else if options.continue_session {
            cmd.arg("--continue");
        }

        // MCP configuration
        if let Some(mcp_config) = &options.mcp_config {
            cmd.args(["--mcp-config", &mcp_config.display().to_string()]);
        }

        // System prompt customization
        if let Some(prompt) = &options.append_system_prompt {
            cmd.args(["--append-system-prompt", prompt]);
        }

        // Permission mode
        if let Some(mode) = &options.permission_mode {
            cmd.args(["--permission-mode", mode]);
        }

        // MCP permission prompt tool
        if let Some(tool) = &options.permission_prompt_tool {
            cmd.args(["--permission-prompt-tool", tool]);
        }

        // Working directory
        if let Some(dir) = &options.working_dir {
            cmd.current_dir(dir);
        }

        // Set up pipes
        cmd.stdin(Stdio::null()); // We don't send input in print mode
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let child = cmd.spawn()?;

        Ok(Self {
            child,
            session_id: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Get the cancel flag for external cancellation.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel_flag.clone()
    }

    /// Get the child process ID for direct signal sending.
    pub fn child_pid(&self) -> u32 {
        self.child.id()
    }

    /// Cancel the running CLI process.
    pub fn cancel(&mut self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
        // Send SIGINT to gracefully stop
        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;
            let _ = kill(Pid::from_raw(self.child.id() as i32), Signal::SIGINT);
        }
        #[cfg(not(unix))]
        {
            let _ = self.child.kill();
        }
    }

    /// Get the session ID (available after init message).
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Process the CLI output stream and send AgentEvents.
    ///
    /// This is the main loop that reads NDJSON from the CLI and converts
    /// it to AgentEvents for the UI.
    pub fn process_stream(
        mut self,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
    ) -> std::io::Result<Option<String>> {
        let stdout = self.child.stdout.take().expect("stdout not captured");
        let reader = BufReader::new(stdout);

        // Track tool states for proper event generation
        let mut active_tools: HashMap<String, String> = HashMap::new(); // id -> name
        let mut session_id: Option<String> = None;
        let request_id = 0u64; // We don't have request IDs in CLI mode

        // Send started event
        let _ = event_tx.send(AgentEvent::Started { request_id });

        for line in reader.lines() {
            // Check for cancellation
            if self.cancel_flag.load(Ordering::Relaxed) {
                self.cancel();
                let _ = event_tx.send(AgentEvent::Interrupted {
                    request_id,
                    messages: vec![], // CLI manages its own history
                });
                break;
            }

            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    let _ = event_tx.send(AgentEvent::Error(format!("Read error: {}", e)));
                    break;
                }
            };

            // Skip empty lines
            if line.trim().is_empty() {
                continue;
            }

            // Parse the JSON message
            let msg: CliMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(e) => {
                    tracing::warn!("Failed to parse CLI message: {} - line: {}", e, line);
                    continue;
                }
            };

            // Convert to AgentEvents
            match msg {
                CliMessage::System(sys) => {
                    session_id = Some(sys.session_id.clone());
                    self.session_id = Some(sys.session_id.clone());
                    // Send session ID to UI for conversation continuity
                    let _ = event_tx.send(AgentEvent::SessionStarted {
                        session_id: sys.session_id,
                    });
                }

                CliMessage::Assistant(asst) => {
                    for block in asst.message.content {
                        match block {
                            ContentBlock::Text { text } => {
                                let _ = event_tx.send(AgentEvent::ResponseText(text));
                            }
                            ContentBlock::Thinking { thinking } => {
                                let _ = event_tx.send(AgentEvent::ThinkingText(thinking));
                            }
                            ContentBlock::ToolUse { id, name, input } => {
                                // Tool started
                                active_tools.insert(id.clone(), name.clone());
                                let _ = event_tx.send(AgentEvent::ToolStarted {
                                    id: id.clone(),
                                    name: name.clone(),
                                });

                                // Send input as parameters
                                if let serde_json::Value::Object(obj) = input {
                                    for (key, value) in obj {
                                        let value_str = match value {
                                            serde_json::Value::String(s) => s,
                                            other => other.to_string(),
                                        };
                                        let _ = event_tx.send(AgentEvent::ToolParameter {
                                            tool_id: id.clone(),
                                            name: key,
                                            value: value_str,
                                        });
                                    }
                                }
                            }
                            ContentBlock::ToolResult {
                                tool_use_id,
                                content,
                                is_error,
                            } => {
                                // Tool completed
                                if let Some(output) = content {
                                    let _ = event_tx.send(AgentEvent::ToolOutput {
                                        tool_id: tool_use_id.clone(),
                                        chunk: output.clone(),
                                    });
                                    let status = if is_error.unwrap_or(false) {
                                        ToolStatus::Error
                                    } else {
                                        ToolStatus::Success
                                    };
                                    let _ = event_tx.send(AgentEvent::ToolStatus {
                                        id: tool_use_id.clone(),
                                        status,
                                        message: None,
                                        output: Some(output),
                                    });
                                }
                                let _ =
                                    event_tx.send(AgentEvent::ToolEnded { id: tool_use_id });
                            }
                            ContentBlock::ServerToolUse { id, name, input } => {
                                // MCP tool - treat like regular tool
                                active_tools.insert(id.clone(), name.clone());
                                let _ = event_tx.send(AgentEvent::ToolStarted {
                                    id: id.clone(),
                                    name: format!("mcp:{}", name),
                                });

                                if let serde_json::Value::Object(obj) = input {
                                    for (key, value) in obj {
                                        let value_str = match value {
                                            serde_json::Value::String(s) => s,
                                            other => other.to_string(),
                                        };
                                        let _ = event_tx.send(AgentEvent::ToolParameter {
                                            tool_id: id.clone(),
                                            name: key,
                                            value: value_str,
                                        });
                                    }
                                }
                            }
                            ContentBlock::ServerToolResult {
                                tool_use_id,
                                content,
                            } => {
                                if let Some(output) = content {
                                    let _ = event_tx.send(AgentEvent::ToolOutput {
                                        tool_id: tool_use_id.clone(),
                                        chunk: output.clone(),
                                    });
                                    let _ = event_tx.send(AgentEvent::ToolStatus {
                                        id: tool_use_id.clone(),
                                        status: ToolStatus::Success,
                                        message: None,
                                        output: Some(output),
                                    });
                                }
                                let _ =
                                    event_tx.send(AgentEvent::ToolEnded { id: tool_use_id });
                            }
                        }
                    }
                }

                CliMessage::User(user) => {
                    // User messages contain tool results
                    for block in user.message.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } = block
                        {
                            if let Some(output) = content {
                                let _ = event_tx.send(AgentEvent::ToolOutput {
                                    tool_id: tool_use_id.clone(),
                                    chunk: output.clone(),
                                });
                                let status = if is_error.unwrap_or(false) {
                                    ToolStatus::Error
                                } else {
                                    ToolStatus::Success
                                };
                                let _ = event_tx.send(AgentEvent::ToolStatus {
                                    id: tool_use_id.clone(),
                                    status,
                                    message: None,
                                    output: Some(output),
                                });
                            }
                        }
                    }
                }

                CliMessage::Result(result) => {
                    // Query completed
                    session_id = Some(result.session_id.clone());

                    // NOTE: We do NOT send result.result as ResponseText here because
                    // the CLI already emits the final response as an Assistant message
                    // before the Result message. Sending it again would double the text.

                    // Send usage/cost data before finishing
                    let (input_tokens, output_tokens) = result.usage
                        .as_ref()
                        .map(|u| (u.input_tokens, u.output_tokens))
                        .unwrap_or((None, None));
                    if result.cost_usd.is_some() || input_tokens.is_some() || output_tokens.is_some() {
                        let _ = event_tx.send(AgentEvent::UsageUpdate {
                            cost_usd: result.cost_usd,
                            input_tokens,
                            output_tokens,
                        });
                    }

                    // Send finished event
                    let _ = event_tx.send(AgentEvent::Finished {
                        request_id,
                        messages: vec![], // CLI manages its own history via session
                    });

                    // Check for AskUserQuestion in permission_denials.
                    // The CLI can't handle interactive tools in -p mode, so it
                    // errors them out. We intercept here and emit a UI event so
                    // we can show the question, do JSONL surgery, and resume.
                    for denial in &result.permission_denials {
                        if denial.tool_name == "AskUserQuestion" {
                            if let Some(questions) = parse_user_questions(&denial.tool_input) {
                                let _ = event_tx.send(AgentEvent::UserQuestionRequested {
                                    tool_use_id: denial.tool_use_id.clone(),
                                    questions,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Check if we were cancelled (SIGINT sent) - send Interrupted event
        if self.cancel_flag.load(Ordering::Relaxed) {
            let _ = event_tx.send(AgentEvent::Interrupted {
                request_id,
                messages: vec![],
            });
        }

        // Wait for process to exit
        let _ = self.child.wait();

        Ok(session_id)
    }
}

impl Drop for ClaudeCli {
    fn drop(&mut self) {
        // Ensure the child process is cleaned up
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// =============================================================================
// AskUserQuestion Parsing
// =============================================================================

use super::events::{UserQuestion, UserQuestionOption};

/// Parse the AskUserQuestion tool input into our UserQuestion types.
pub fn parse_user_questions(tool_input: &serde_json::Value) -> Option<Vec<UserQuestion>> {
    let questions = tool_input.get("questions")?.as_array()?;
    let mut result = Vec::new();
    for q in questions {
        let question = q.get("question")?.as_str()?.to_string();
        let header = q.get("header").and_then(|h| h.as_str()).unwrap_or("").to_string();
        let multi_select = q.get("multiSelect").and_then(|m| m.as_bool()).unwrap_or(false);
        let options = q.get("options")
            .and_then(|o| o.as_array())
            .map(|arr| {
                arr.iter().filter_map(|opt| {
                    Some(UserQuestionOption {
                        label: opt.get("label")?.as_str()?.to_string(),
                        description: opt.get("description").and_then(|d| d.as_str()).unwrap_or("").to_string(),
                    })
                }).collect()
            })
            .unwrap_or_default();
        result.push(UserQuestion { question, header, options, multi_select });
    }
    Some(result)
}

// =============================================================================
// JSONL Surgery
// =============================================================================

/// Patch a Claude Code session JSONL file: replace the error tool_result for
/// `tool_use_id` with a success result containing `answer_content`, and
/// truncate everything after (the assistant's error-path response).
pub fn patch_session_jsonl(
    path: &std::path::Path,
    tool_use_id: &str,
    answer_content: &str,
) -> std::io::Result<()> {
    use std::io::{BufRead, BufReader, Write};

    let file = std::fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines: Vec<serde_json::Value> = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() { continue; }
        match serde_json::from_str(&line) {
            Ok(v) => lines.push(v),
            Err(_) => continue,
        }
    }

    // Find the user message containing the error tool_result for this tool_use_id
    let mut truncate_at = None;
    for (i, line) in lines.iter_mut().enumerate() {
        if line.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }
        let content = match line.pointer_mut("/message/content") {
            Some(serde_json::Value::Array(arr)) => arr,
            _ => continue,
        };
        for block in content.iter_mut() {
            if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                continue;
            }
            if block.get("tool_use_id").and_then(|t| t.as_str()) != Some(tool_use_id) {
                continue;
            }
            // Found it — replace error with success
            block["content"] = serde_json::Value::String(answer_content.to_string());
            block["is_error"] = serde_json::Value::Bool(false);
            // Also fix the top-level tool_use_result if present
            if line.get("tool_use_result").is_some() {
                line["tool_use_result"] = serde_json::json!({
                    "type": "text",
                    "text": answer_content,
                });
            }
            truncate_at = Some(i + 1); // keep up to and including this line
            break;
        }
        if truncate_at.is_some() { break; }
    }

    let truncate_at = truncate_at.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "tool_result not found in session")
    })?;

    // Truncate and write back
    lines.truncate(truncate_at);
    let mut file = std::fs::File::create(path)?;
    for line in &lines {
        writeln!(file, "{}", serde_json::to_string(line).unwrap())?;
    }

    Ok(())
}

/// Compute the Claude Code session file path for a given working directory and session ID.
pub fn session_file_path(cwd: &str, session_id: &str) -> std::path::PathBuf {
    let encoded = cwd.replace('/', "-");
    let home = std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
    home.join(".claude/projects")
        .join(&encoded)
        .join(format!("{}.jsonl", session_id))
}

// =============================================================================
// Spawn Helper for UI Integration
// =============================================================================

/// Write a temporary MCP config file that points the Claude CLI at our
/// permission proxy subcommand, which communicates with the UI over TCP.
fn write_mcp_config(port: u16) -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("nexus"));
    let config = serde_json::json!({
        "mcpServers": {
            "nexus_perm": {
                "type": "stdio",
                "command": exe.to_str().unwrap_or("nexus"),
                "args": ["mcp-proxy", "--port", port.to_string()]
            }
        }
    });
    let path = std::env::temp_dir().join(format!("nexus-mcp-{}.json", std::process::id()));
    std::fs::write(&path, serde_json::to_string(&config).unwrap()).unwrap();
    path
}

// =============================================================================
// Tests
// =============================================================================

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
    // parse_user_questions tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_parse_user_questions_basic() {
        let input = json!({
            "questions": [{
                "question": "Which option do you prefer?",
                "header": "Choice",
                "multiSelect": false,
                "options": [
                    {"label": "Option A", "description": "First choice"},
                    {"label": "Option B", "description": "Second choice"}
                ]
            }]
        });
        let result = parse_user_questions(&input).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].question, "Which option do you prefer?");
        assert_eq!(result[0].header, "Choice");
        assert!(!result[0].multi_select);
        assert_eq!(result[0].options.len(), 2);
        assert_eq!(result[0].options[0].label, "Option A");
        assert_eq!(result[0].options[0].description, "First choice");
    }

    #[test]
    fn test_parse_user_questions_multi_select() {
        let input = json!({
            "questions": [{
                "question": "Select all that apply",
                "header": "Multi",
                "multiSelect": true,
                "options": [
                    {"label": "A", "description": ""},
                    {"label": "B", "description": ""}
                ]
            }]
        });
        let result = parse_user_questions(&input).unwrap();
        assert!(result[0].multi_select);
    }

    #[test]
    fn test_parse_user_questions_multiple_questions() {
        let input = json!({
            "questions": [
                {
                    "question": "First question?",
                    "header": "Q1",
                    "options": [{"label": "Yes", "description": ""}]
                },
                {
                    "question": "Second question?",
                    "header": "Q2",
                    "options": [{"label": "No", "description": ""}]
                }
            ]
        });
        let result = parse_user_questions(&input).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].question, "First question?");
        assert_eq!(result[1].question, "Second question?");
    }

    #[test]
    fn test_parse_user_questions_no_questions_field() {
        let input = json!({"other": "data"});
        let result = parse_user_questions(&input);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_user_questions_empty_questions() {
        let input = json!({"questions": []});
        let result = parse_user_questions(&input).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_user_questions_missing_optional_fields() {
        let input = json!({
            "questions": [{
                "question": "What?",
                "options": [{"label": "X"}]
            }]
        });
        let result = parse_user_questions(&input).unwrap();
        assert_eq!(result[0].header, ""); // default
        assert!(!result[0].multi_select); // default false
        assert_eq!(result[0].options[0].description, ""); // default
    }

    #[test]
    fn test_parse_user_questions_invalid_question_missing_text() {
        // When a question is missing required "question" field, the ? operator
        // causes early return of None from the function
        let input = json!({
            "questions": [{
                "header": "H",
                "options": []
            }]
        });
        let result = parse_user_questions(&input);
        assert!(result.is_none());
    }

    // -------------------------------------------------------------------------
    // session_file_path tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_session_file_path_basic() {
        let path = session_file_path("/home/user/project", "abc123");
        let path_str = path.to_string_lossy();
        assert!(path_str.ends_with("abc123.jsonl"));
        assert!(path_str.contains(".claude/projects"));
        assert!(path_str.contains("-home-user-project")); // slashes replaced with dashes
    }

    #[test]
    fn test_session_file_path_root() {
        let path = session_file_path("/", "session-id");
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("-")); // "/" becomes "-"
        assert!(path_str.ends_with("session-id.jsonl"));
    }

    #[test]
    fn test_session_file_path_nested() {
        let path = session_file_path("/a/b/c/d/e", "test");
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("-a-b-c-d-e"));
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

/// Spawn a Claude Code CLI query and stream events to the UI.
///
/// This replaces the old `spawn_agent_task` function that used nexus-agent directly.
pub async fn spawn_claude_cli_task(
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    cancel_flag: Arc<AtomicBool>,
    query: String,
    working_dir: PathBuf,
    session_id: Option<String>,
    attachments: Vec<nexus_api::Value>,
    permission_port: Option<u16>,
) -> anyhow::Result<Option<String>> {
    use tokio::task::spawn_blocking;

    // When a permission port is provided, set up MCP config and use default
    // permission mode so dangerous tools go through our permission proxy.
    let (mcp_config, permission_prompt_tool, permission_mode) = if let Some(port) = permission_port
    {
        let config_path = write_mcp_config(port);
        (
            Some(config_path),
            Some("mcp__nexus_perm__permission_prompt".to_string()),
            None, // default mode — CLI will call permission tool for dangerous ops
        )
    } else {
        (
            None,
            None,
            Some("acceptEdits".to_string()), // legacy: accept everything
        )
    };

    // Build options
    let options = CliOptions {
        // Safe read-only tools are always allowed without prompting.
        // Dangerous tools (Bash, Edit, Write) go through the MCP permission prompt.
        allowed_tools: vec![
            "Read".to_string(),
            "Glob".to_string(),
            "Grep".to_string(),
            "Task".to_string(),
            "TodoWrite".to_string(),
            "WebSearch".to_string(),
            "WebFetch".to_string(),
        ],
        // Disallow plan-mode tools (no interactive terminal).
        // AskUserQuestion goes through MCP permission prompt — the permission
        // server shows the question dialog and returns answers via updatedInput.
        disallowed_tools: vec![
            "EnterPlanMode".to_string(),
            "ExitPlanMode".to_string(),
        ],
        max_turns: Some(100),
        resume: session_id,
        working_dir: Some(working_dir),
        mcp_config,
        permission_prompt_tool,
        permission_mode,
        ..Default::default()
    };

    // Build prompt with attachments if any
    let prompt = if !attachments.is_empty() {
        // Note: CLI doesn't support image attachments directly in -p mode
        // We'd need to save them as files and reference them
        // For now, just use the text query
        tracing::warn!(
            "Attachments not yet supported in CLI mode, {} attachments ignored",
            attachments.len()
        );
        query
    } else {
        query
    };

    // Spawn CLI in blocking task (it does synchronous I/O)
    let result = spawn_blocking(move || {
        match ClaudeCli::spawn(&prompt, options) {
            Ok(cli) => {
                // Get child PID for direct SIGINT
                let child_pid = cli.child_pid();

                // Share cancel flag
                let cli_cancel = cli.cancel_flag();

                // Set up cancellation forwarding with direct SIGINT
                let cancel_flag_clone = cancel_flag.clone();
                std::thread::spawn(move || {
                    while !cancel_flag_clone.load(Ordering::Relaxed) {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    // Set the flag
                    cli_cancel.store(true, Ordering::SeqCst);
                    // Also send SIGINT directly to unblock any waiting I/O
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{kill, Signal};
                        use nix::unistd::Pid;
                        let _ = kill(Pid::from_raw(child_pid as i32), Signal::SIGINT);
                    }
                });

                cli.process_stream(event_tx)
            }
            Err(e) => {
                let _ = event_tx.send(AgentEvent::Error(format!(
                    "Failed to spawn Claude CLI: {}. Is 'claude' installed?",
                    e
                )));
                Ok(None)
            }
        }
    })
    .await?;

    result.map_err(|e| anyhow::anyhow!("CLI error: {}", e))
}
