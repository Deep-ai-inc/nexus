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

use crate::agent_adapter::AgentEvent;
use crate::agent_block::ToolStatus;

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
    pub mcp_servers: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AssistantMessage {
    /// Message content blocks.
    #[serde(default)]
    pub content: Vec<ContentBlock>,
    /// Unique message ID.
    pub message_id: Option<String>,
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
        content: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserMessage {
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
    pub cost_usd: Option<f64>,
    /// Whether the session is resumable.
    pub is_resumable: Option<bool>,
    /// Total duration in ms.
    pub duration_ms: Option<u64>,
    /// Number of API turns.
    pub num_turns: Option<u32>,
    /// Token usage.
    pub usage: Option<TokenUsage>,
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
                    for block in asst.content {
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
                    // User messages contain tool results - we already handle these
                    // in the assistant message flow, but we can process them here too
                    for block in user.content {
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

                    // If there's a final result text, send it
                    if let Some(text) = result.result {
                        if !text.is_empty() {
                            let _ = event_tx.send(AgentEvent::ResponseText(text));
                        }
                    }

                    // Send finished event
                    let _ = event_tx.send(AgentEvent::Finished {
                        request_id,
                        messages: vec![], // CLI manages its own history via session
                    });
                }
            }
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
// Spawn Helper for UI Integration
// =============================================================================

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
) -> anyhow::Result<Option<String>> {
    use tokio::task::spawn_blocking;

    // Build options
    let options = CliOptions {
        // Allow common tools without prompting
        allowed_tools: vec![
            "Read".to_string(),
            "Glob".to_string(),
            "Grep".to_string(),
            "Bash".to_string(),
            "Edit".to_string(),
            "Write".to_string(),
            "Task".to_string(),
        ],
        max_turns: Some(100),
        resume: session_id,
        working_dir: Some(working_dir),
        // Accept edits by default (user can review in UI)
        permission_mode: Some("acceptEdits".to_string()),
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
                // Share cancel flag
                let cli_cancel = cli.cancel_flag();

                // Set up cancellation forwarding
                let cancel_flag_clone = cancel_flag.clone();
                std::thread::spawn(move || {
                    while !cancel_flag_clone.load(Ordering::Relaxed) {
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    cli_cancel.store(true, Ordering::SeqCst);
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
