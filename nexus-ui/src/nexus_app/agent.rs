//! Agent widget — owns agent blocks, streaming channels, and permission handling.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use nexus_api::{BlockId, Value};
use strata::Subscription;
use strata::content_address::SourceId;

use crate::agent_adapter::{AgentEvent, PermissionResponse};
use crate::agent_block::{AgentBlock, AgentBlockState, PermissionRequest};
use crate::nexus_widgets::AgentBlockWidget;
use crate::systems::{agent_subscription, spawn_agent_task};

use super::context_menu::{ContextMenuItem, ContextTarget};
use super::message::{AgentMsg, ContextMenuMsg, NexusMessage};
use super::source_ids;

/// Typed output from AgentWidget → orchestrator.
pub(crate) enum AgentOutput {
    /// Nothing happened.
    None,
    /// Orchestrator should scroll history to bottom.
    ScrollToBottom,
}

impl Default for AgentOutput {
    fn default() -> Self { Self::None }
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

    // --- Subscription channel (owned by this widget) ---
    event_rx: Arc<Mutex<mpsc::UnboundedReceiver<AgentEvent>>>,
}

impl AgentWidget {
    pub fn new(
        event_tx: mpsc::UnboundedSender<AgentEvent>,
        event_rx: Arc<Mutex<mpsc::UnboundedReceiver<AgentEvent>>>,
    ) -> Self {
        Self {
            blocks: Vec::new(),
            block_index: HashMap::new(),
            active: None,
            event_tx,
            permission_tx: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            dirty: false,
            session_id: None,
            event_rx,
        }
    }

    /// Whether the agent has pending output that needs a redraw tick.
    pub fn needs_redraw(&self) -> bool {
        self.dirty
    }

    // ---- View contributions ----

    /// Push a single agent block into the given scroll column.
    pub fn push_block(
        &self,
        scroll: strata::ScrollColumn,
        block: &AgentBlock,
    ) -> strata::ScrollColumn {
        scroll.push(AgentBlockWidget {
            block,
            thinking_toggle_id: source_ids::agent_thinking_toggle(block.id),
            stop_id: source_ids::agent_stop(block.id),
        })
    }

    // ---- Event handling ----

    /// Handle a widget click within agent-owned UI. Returns None if not our widget.
    pub fn on_click(&self, id: SourceId) -> Option<AgentMsg> {
        for block in &self.blocks {
            if id == source_ids::agent_thinking_toggle(block.id) {
                return Some(AgentMsg::ToggleThinking(block.id));
            }
            if id == source_ids::agent_stop(block.id) {
                return Some(AgentMsg::Interrupt);
            }
            for (i, _tool) in block.tools.iter().enumerate() {
                if id == source_ids::agent_tool_toggle(block.id, i) {
                    return Some(AgentMsg::ToggleTool(block.id, i));
                }
            }
            if let Some(ref perm) = block.pending_permission {
                if id == source_ids::agent_perm_deny(block.id) {
                    return Some(AgentMsg::PermissionDeny(block.id, perm.id.clone()));
                }
                if id == source_ids::agent_perm_allow(block.id) {
                    return Some(AgentMsg::PermissionGrant(block.id, perm.id.clone()));
                }
                if id == source_ids::agent_perm_always(block.id) {
                    return Some(AgentMsg::PermissionGrantSession(block.id, perm.id.clone()));
                }
            }
        }
        None
    }

    /// Build a context menu for a right-click on agent content.
    pub fn context_menu_for_source(
        &self,
        source_id: SourceId,
        x: f32,
        y: f32,
    ) -> Option<ContextMenuMsg> {
        let block_id = self.block_for_source(source_id)?;
        Some(ContextMenuMsg::Show(
            x, y,
            vec![ContextMenuItem::Copy, ContextMenuItem::SelectAll],
            ContextTarget::AgentBlock(block_id),
        ))
    }

    /// Build a fallback context menu (last block) for right-click on empty area.
    pub fn fallback_context_menu(&self, x: f32, y: f32) -> Option<ContextMenuMsg> {
        let block = self.blocks.last()?;
        Some(ContextMenuMsg::Show(
            x, y,
            vec![ContextMenuItem::Copy, ContextMenuItem::SelectAll],
            ContextTarget::AgentBlock(block.id),
        ))
    }

    /// Check if a hit address belongs to an agent block. Returns the block_id if so.
    pub fn block_for_source(&self, source_id: SourceId) -> Option<BlockId> {
        for block in &self.blocks {
            if source_id == source_ids::agent_query(block.id)
                || source_id == source_ids::agent_thinking(block.id)
                || source_id == source_ids::agent_response(block.id)
            {
                return Some(block.id);
            }
        }
        None
    }

    /// Create the subscription for agent events.
    ///
    /// Returns `Subscription<NexusMessage>` directly because iced's
    /// `Subscription::map` panics on capturing closures, so we can't
    /// return `Subscription<AgentMsg>` and `map_msg` at the root.
    pub fn subscription(&self) -> Subscription<NexusMessage> {
        let rx = self.event_rx.clone();
        Subscription::from_iced(
            agent_subscription(rx).map(|evt| NexusMessage::Agent(AgentMsg::Event(evt))),
        )
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
        self.dirty = true;

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

    /// Handle a message, returning commands and cross-cutting output.
    pub fn update(&mut self, msg: AgentMsg, _ctx: &mut strata::component::Ctx) -> (strata::Command<AgentMsg>, AgentOutput) {
        let output = match msg {
            AgentMsg::Event(evt) => {
                self.dirty = true;
                self.handle_event(evt)
            }
            AgentMsg::ToggleThinking(id) => { self.toggle_thinking(id); AgentOutput::None }
            AgentMsg::ToggleTool(id, idx) => { self.toggle_tool(id, idx); AgentOutput::None }
            AgentMsg::PermissionGrant(block_id, perm_id) => { self.permission_grant(block_id, perm_id); AgentOutput::None }
            AgentMsg::PermissionGrantSession(block_id, perm_id) => { self.permission_grant_session(block_id, perm_id); AgentOutput::None }
            AgentMsg::PermissionDeny(block_id, perm_id) => { self.permission_deny(block_id, perm_id); AgentOutput::None }
            AgentMsg::Interrupt => { self.interrupt(); AgentOutput::None }
        };
        (strata::Command::none(), output)
    }

    /// Handle an agent event from the streaming channel.
    fn handle_event(&mut self, event: AgentEvent) -> AgentOutput {
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
