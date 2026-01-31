//! Agent widget — owns agent blocks, streaming channels, and permission handling.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::mpsc;

use nexus_api::{BlockId, Value};

use crate::agent_adapter::{AgentEvent, PermissionResponse};
use crate::agent_block::{AgentBlock, AgentBlockState, PermissionRequest};
use crate::systems::spawn_agent_task;

/// Typed output from AgentWidget → orchestrator.
#[allow(dead_code)]
pub(crate) enum AgentOutput {
    /// Nothing happened.
    None,
    /// Orchestrator should scroll history to bottom.
    ScrollToBottom,
}

/// Manages all agent-related state: agent blocks, streaming, permissions.
pub(crate) struct AgentWidget {
    pub blocks: Vec<AgentBlock>,
    pub block_index: HashMap<BlockId, usize>,
    pub active: Option<BlockId>,
    pub event_tx: mpsc::UnboundedSender<AgentEvent>,
    pub permission_tx: Option<mpsc::UnboundedSender<(String, PermissionResponse)>>,
    pub cancel_flag: Arc<AtomicBool>,
    pub dirty: bool,
    pub session_id: Option<String>,
}

impl AgentWidget {
    pub fn new(event_tx: mpsc::UnboundedSender<AgentEvent>) -> Self {
        Self {
            blocks: Vec::new(),
            block_index: HashMap::new(),
            active: None,
            event_tx,
            permission_tx: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            dirty: false,
            session_id: None,
        }
    }

    /// Spawn an agent task.
    pub fn spawn(
        &mut self,
        block_id: BlockId,
        query: String,
        contextualized_query: String,
        attachments: Vec<Value>,
        cwd: &str,
    ) {
        let agent_block = AgentBlock::new(block_id, query);
        let idx = self.blocks.len();
        self.block_index.insert(block_id, idx);
        self.blocks.push(agent_block);
        self.active = Some(block_id);

        // Reset cancel flag
        self.cancel_flag.store(false, Ordering::SeqCst);

        let agent_tx = self.event_tx.clone();
        let cancel_flag = self.cancel_flag.clone();
        let cwd = PathBuf::from(cwd);
        let session_id = self.session_id.clone();

        tokio::spawn(async move {
            match spawn_agent_task(
                agent_tx,
                cancel_flag,
                contextualized_query,
                cwd,
                attachments,
                session_id,
            )
            .await
            {
                Ok(new_session_id) => {
                    if let Some(sid) = new_session_id {
                        tracing::info!("Agent session: {}", sid);
                    }
                }
                Err(e) => {
                    tracing::error!("Agent task failed: {}", e);
                }
            }
        });

        // Mark block as streaming
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.state = AgentBlockState::Streaming;
            }
        }
    }

    /// Handle an agent event from the streaming channel.
    pub fn handle_event(&mut self, event: AgentEvent) -> AgentOutput {
        // Handle session ID
        if let AgentEvent::SessionStarted { ref session_id } = event {
            self.session_id = Some(session_id.clone());
        }

        if let Some(block_id) = self.active {
            if let Some(&idx) = self.block_index.get(&block_id) {
                if let Some(block) = self.blocks.get_mut(idx) {
                    match event {
                        AgentEvent::SessionStarted { .. } => {}
                        AgentEvent::Started { .. } => {
                            block.state = AgentBlockState::Streaming;
                        }
                        AgentEvent::ResponseText(text) => {
                            block.append_response(&text);
                        }
                        AgentEvent::ThinkingText(text) => {
                            block.append_thinking(&text);
                        }
                        AgentEvent::ToolStarted { id, name } => {
                            block.start_tool(id, name);
                        }
                        AgentEvent::ToolParameter {
                            tool_id,
                            name,
                            value,
                        } => {
                            block.add_tool_parameter(&tool_id, name, value);
                        }
                        AgentEvent::ToolOutput { tool_id, chunk } => {
                            block.append_tool_output(&tool_id, &chunk);
                        }
                        AgentEvent::ToolEnded { .. } => {}
                        AgentEvent::ToolStatus {
                            id,
                            status,
                            message,
                            output,
                        } => {
                            block.update_tool_status(&id, status, message, output);
                        }
                        AgentEvent::ImageAdded { media_type, data } => {
                            block.add_image(media_type, data);
                        }
                        AgentEvent::PermissionRequested {
                            id,
                            tool_name,
                            tool_id,
                            description,
                            action,
                            working_dir,
                        } => {
                            block.request_permission(PermissionRequest {
                                id,
                                tool_name,
                                tool_id,
                                description,
                                action,
                                working_dir,
                            });
                        }
                        AgentEvent::Finished { .. } => {
                            block.complete();
                            self.active = None;
                        }
                        AgentEvent::Interrupted { .. } => {
                            block.state = AgentBlockState::Interrupted;
                            self.active = None;
                        }
                        AgentEvent::Error(err) => {
                            block.fail(err);
                            self.active = None;
                        }
                    }
                }
            }
        }

        AgentOutput::ScrollToBottom
    }

    /// Toggle thinking section visibility for a block.
    pub fn toggle_thinking(&mut self, id: BlockId) {
        if let Some(&idx) = self.block_index.get(&id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.toggle_thinking();
            }
        }
    }

    /// Toggle tool invocation visibility for a block.
    pub fn toggle_tool(&mut self, id: BlockId, tool_index: usize) {
        if let Some(&idx) = self.block_index.get(&id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                if let Some(tool) = block.tools.get_mut(tool_index) {
                    tool.collapsed = !tool.collapsed;
                    block.version += 1;
                }
            }
        }
    }

    /// Grant permission once.
    pub fn permission_grant(&mut self, block_id: BlockId, perm_id: String) {
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.clear_permission();
            }
        }
        if let Some(ref tx) = self.permission_tx {
            let _ = tx.send((perm_id, PermissionResponse::GrantedOnce));
        }
    }

    /// Grant permission for session.
    pub fn permission_grant_session(&mut self, block_id: BlockId, perm_id: String) {
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.clear_permission();
            }
        }
        if let Some(ref tx) = self.permission_tx {
            let _ = tx.send((perm_id, PermissionResponse::GrantedSession));
        }
    }

    /// Deny permission.
    pub fn permission_deny(&mut self, block_id: BlockId, perm_id: String) {
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.clear_permission();
                block.fail("Permission denied".to_string());
            }
        }
        if let Some(ref tx) = self.permission_tx {
            let _ = tx.send((perm_id, PermissionResponse::Denied));
        }
        self.active = None;
    }

    /// Interrupt the active agent.
    pub fn interrupt(&self) {
        if self.active.is_some() {
            self.cancel_flag.store(true, Ordering::SeqCst);
        }
    }

    /// Clear all agent blocks and cancel active agent.
    pub fn clear(&mut self) {
        if self.active.is_some() {
            self.cancel_flag.store(true, Ordering::SeqCst);
            self.active = None;
        }
        self.blocks.clear();
        self.block_index.clear();
    }

    /// Check if an agent is currently active.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }
}
