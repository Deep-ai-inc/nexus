//! TCP permission server — accepts requests from the MCP proxy and forwards
//! them to the UI as `AgentEvent::PermissionRequested` events.
//!
//! One TCP connection per permission request. The MCP proxy connects, sends a
//! JSON line, we emit the event, wait for the user's decision, and respond.
//!
//! Special handling for AskUserQuestion: instead of a generic permission dialog,
//! we show the question dialog and return the user's answer via `updatedInput`.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

use crate::features::agent::events::AgentEvent;
use crate::features::agent::claude::parse_user_questions;

/// Response from the UI to a permission/question request.
#[derive(Debug, Clone)]
pub enum PermissionDecision {
    /// Allow the tool call (regular permission).
    Allow,
    /// Deny the tool call.
    Deny,
    /// Answer to an AskUserQuestion — contains `{"Header": "Selected"}` map.
    Answer(serde_json::Value),
}

/// Run the permission server. Accepts connections until the listener is dropped.
pub async fn run(
    listener: TcpListener,
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    mut response_rx: mpsc::UnboundedReceiver<PermissionDecision>,
) {
    tracing::info!("[perm-server] listening on {:?}", listener.local_addr());
    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!("[perm-server] accept error: {e}");
                break;
            }
        };
        tracing::info!("[perm-server] connection from {addr}");

        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut line = String::new();

        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            tracing::warn!("[perm-server] empty read, skipping");
            continue;
        }

        tracing::info!("[perm-server] request: {}", &line[..line.len().min(300)]);

        let request: serde_json::Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[perm-server] JSON parse error: {e}");
                continue;
            }
        };

        let tool_name = request
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        // CLI sends "input", not "tool_input"
        let tool_input = request
            .get("input")
            .cloned()
            .unwrap_or(serde_json::Value::Object(Default::default()));

        // AskUserQuestion: show question dialog instead of permission dialog.
        if tool_name == "AskUserQuestion" {
            if let Some(questions) = parse_user_questions(&tool_input) {
                tracing::info!("[perm-server] AskUserQuestion with {} questions", questions.len());
                let _ = event_tx.send(AgentEvent::UserQuestionRequested {
                    tool_use_id: request
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    questions,
                });

                // Wait for the answer
                let decision = response_rx.recv().await.unwrap_or(PermissionDecision::Deny);
                tracing::info!("[perm-server] AskUserQuestion decision: {decision:?}");

                let resp = match decision {
                    PermissionDecision::Answer(answers) => {
                        // Return allow + updatedInput with answers injected
                        let mut updated = tool_input.clone();
                        updated["answers"] = answers;
                        serde_json::json!({ "allow": true, "updatedInput": updated })
                    }
                    _ => serde_json::json!({ "allow": false }),
                };
                let _ = writer
                    .write_all(format!("{}\n", serde_json::to_string(&resp).unwrap()).as_bytes())
                    .await;
                let _ = writer.flush().await;
                continue;
            }
        }

        // Regular permission request
        let description = format_description(&tool_name, &tool_input);
        let action = summarize_action(&tool_name, &tool_input);

        tracing::info!("[perm-server] emitting PermissionRequested for {tool_name}");

        let _ = event_tx.send(AgentEvent::PermissionRequested {
            id: tool_name.clone(),
            tool_name,
            tool_id: String::new(),
            description,
            action,
            working_dir: None,
        });

        // Wait for the UI to respond
        tracing::info!("[perm-server] waiting for UI response...");
        let decision = response_rx.recv().await.unwrap_or(PermissionDecision::Deny);
        tracing::info!("[perm-server] UI responded: {decision:?}");

        let allow = matches!(decision, PermissionDecision::Allow);
        let resp = serde_json::json!({ "allow": allow });
        let _ = writer
            .write_all(format!("{}\n", serde_json::to_string(&resp).unwrap()).as_bytes())
            .await;
        let _ = writer.flush().await;
    }
}

/// Human-readable description of what the tool is doing.
fn format_description(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "Bash" => {
            let cmd = input
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("(unknown command)");
            let desc = input.get("description").and_then(|v| v.as_str());
            if let Some(d) = desc {
                format!("{d}\n\n$ {cmd}")
            } else {
                format!("$ {cmd}")
            }
        }
        "Edit" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("Edit {path}")
        }
        "Write" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("Write {path}")
        }
        "NotebookEdit" => {
            let path = input
                .get("notebook_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("Edit notebook {path}")
        }
        _ => format!("{tool_name}"),
    }
}

/// Short action label for the permission dialog.
fn summarize_action(tool_name: &str, input: &serde_json::Value) -> String {
    match tool_name {
        "Bash" => input
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("run command")
            .chars()
            .take(60)
            .collect(),
        "Edit" | "Write" => input
            .get("file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("modify file")
            .to_string(),
        _ => tool_name.to_string(),
    }
}
