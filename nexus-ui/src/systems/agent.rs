//! Agent system for AI assistant integration.
//!
//! This module provides integration with Claude Code CLI, giving us access to
//! all of Claude Code's capabilities without reimplementing the agent loop.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use iced::futures::stream;
use iced::Subscription;
use tokio::sync::{mpsc, Mutex};

use crate::agent_adapter::AgentEvent;
use crate::claude_cli::spawn_claude_cli_task;

/// Spawn an agent task to process a query using Claude Code CLI.
///
/// The CLI handles:
/// - System prompt (same as Claude Code)
/// - Context compaction (automatic when context gets long)
/// - Tool execution (Read, Edit, Bash, Glob, Grep, etc.)
/// - Session management (resume via session_id)
/// - Subagents (via Task tool)
/// - MCP integration
///
/// `session_id` is used to resume a prior conversation (the CLI maintains its own history).
pub async fn spawn_agent_task(
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    cancel_flag: Arc<AtomicBool>,
    query: String,
    working_dir: PathBuf,
    attachments: Vec<nexus_api::Value>,
    session_id: Option<String>,
    permission_port: Option<u16>,
) -> anyhow::Result<Option<String>> {
    spawn_claude_cli_task(event_tx, cancel_flag, query, working_dir, session_id, attachments, permission_port).await
}

/// Async subscription that awaits agent events.
/// Returns raw AgentEvent for caller to map to messages.
pub fn agent_subscription(
    rx: Arc<Mutex<mpsc::UnboundedReceiver<AgentEvent>>>,
) -> Subscription<AgentEvent> {
    struct AgentSubscription;

    Subscription::run_with_id(
        std::any::TypeId::of::<AgentSubscription>(),
        stream::unfold(rx, |rx| async move {
            let event = {
                let mut guard = rx.lock().await;
                guard.recv().await
            };

            event.map(|agent_event| (agent_event, rx))
        }),
    )
}
