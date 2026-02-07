//! Claude Code CLI wrapper.
//!
//! This module provides a Rust interface to the Claude Code CLI, giving us access to
//! all of Claude Code's capabilities: the system prompt, context compaction, subagents,
//! MCP integration, session management, and permissions.
//!
//! Instead of reimplementing the agent loop, we spawn the CLI with `--output-format stream-json`
//! and parse its NDJSON output stream.

pub mod types;
pub mod session;

pub use types::*;
pub use session::{parse_user_questions, patch_session_jsonl, session_file_path};

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;

use super::events::AgentEvent;
use crate::data::agent_block::ToolStatus;

// =============================================================================
// Helpers
// =============================================================================

/// Process an assistant message: dispatch text, thinking, and tool events.
fn handle_assistant_message(
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    asst: AssistantMessage,
) {
    for block in asst.message.content {
        match block {
            ContentBlock::Text { text } => {
                let _ = event_tx.send(AgentEvent::ResponseText(text));
            }
            ContentBlock::Thinking { thinking } => {
                let _ = event_tx.send(AgentEvent::ThinkingText(thinking));
            }
            ContentBlock::ToolUse { id, name, input } => {
                send_tool_started(event_tx, &id, name, input);
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => {
                send_tool_result(event_tx, &tool_use_id, content, is_error);
                let _ = event_tx.send(AgentEvent::ToolEnded { id: tool_use_id });
            }
            ContentBlock::ServerToolUse { id, name, input } => {
                send_tool_started(event_tx, &id, format!("mcp:{}", name), input);
            }
            ContentBlock::ServerToolResult {
                tool_use_id,
                content,
            } => {
                send_tool_result(event_tx, &tool_use_id, content, None);
                let _ = event_tx.send(AgentEvent::ToolEnded { id: tool_use_id });
            }
        }
    }
}

/// Process a result message: emit usage, finished, and question events.
/// Returns the session ID from the result.
fn handle_result_message(
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    result: ResultMessage,
) -> String {
    // Send usage/cost data before finishing
    let (input_tokens, output_tokens) = result
        .usage
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
        request_id: 0,
        messages: vec![], // CLI manages its own history via session
    });

    // Check for AskUserQuestion in permission_denials.
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

    result.session_id
}

/// Send ToolStarted + ToolParameter events for a tool invocation.
fn send_tool_started(
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    id: &str,
    name: String,
    input: serde_json::Value,
) {
    let _ = event_tx.send(AgentEvent::ToolStarted {
        id: id.to_string(),
        name,
    });
    if let serde_json::Value::Object(obj) = input {
        for (key, value) in obj {
            let value_str = match value {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            let _ = event_tx.send(AgentEvent::ToolParameter {
                tool_id: id.to_string(),
                name: key,
                value: value_str,
            });
        }
    }
}

/// Send ToolOutput + ToolStatus events for a tool result.
fn send_tool_result(
    event_tx: &mpsc::UnboundedSender<AgentEvent>,
    tool_use_id: &str,
    content: Option<String>,
    is_error: Option<bool>,
) {
    if let Some(output) = content {
        let _ = event_tx.send(AgentEvent::ToolOutput {
            tool_id: tool_use_id.to_string(),
            chunk: output.clone(),
        });
        let status = if is_error.unwrap_or(false) {
            ToolStatus::Error
        } else {
            ToolStatus::Success
        };
        let _ = event_tx.send(AgentEvent::ToolStatus {
            id: tool_use_id.to_string(),
            status,
            message: None,
            output: Some(output),
        });
    }
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

        let mut session_id: Option<String> = None;

        // request_id is always 0 — CLI mode doesn't have request tracking
        let _ = event_tx.send(AgentEvent::Started { request_id: 0 });

        for line in reader.lines() {
            // Check for cancellation
            if self.cancel_flag.load(Ordering::Relaxed) {
                self.cancel();
                let _ = event_tx.send(AgentEvent::Interrupted {
                    request_id: 0,
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
                    handle_assistant_message(&event_tx, asst);
                }

                CliMessage::User(user) => {
                    for block in user.message.content {
                        if let ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            is_error,
                        } = block
                        {
                            send_tool_result(&event_tx, &tool_use_id, content, is_error);
                        }
                    }
                }

                CliMessage::Result(result) => {
                    session_id = Some(handle_result_message(&event_tx, result));
                }
            }
        }

        // Check if we were cancelled (SIGINT sent) - send Interrupted event
        if self.cancel_flag.load(Ordering::Relaxed) {
            let _ = event_tx.send(AgentEvent::Interrupted {
                request_id: 0,
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
