//! Agent widget — owns agent blocks, streaming channels, and permission handling.

pub mod events;
pub mod claude;
pub mod mcp;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use nexus_api::{BlockId, Value};
use strata::{Padding, Subscription, TextInputState};
use strata::content_address::SourceId;
use strata::event_context::KeyEvent;

use self::events::{AgentEvent, UserQuestion};
use crate::data::agent_block::{AgentBlock, AgentBlockState, PermissionRequest};
use crate::ui::widgets::AgentBlockWidget;
use crate::infra::systems::{agent_subscription, spawn_agent_task};
use crate::infra::systems::permission_server::PermissionDecision;

use crate::ui::context_menu::{ContextMenuItem, ContextTarget};
use crate::app::message::{AgentMsg, ContextMenuMsg, NexusMessage};
use crate::utils::ids as source_ids;

/// Build the answer JSON that Claude Code expects for an AskUserQuestion tool_result.
/// Format: {"questions":[{"question":"...","header":"...","answers":{"Header":"Selected"}}]}
fn build_question_answer(questions: &[UserQuestion], answered_idx: usize, selected_label: &str) -> String {
    let mut q_array = Vec::new();
    for (i, q) in questions.iter().enumerate() {
        let mut q_obj = serde_json::json!({
            "question": q.question,
            "header": q.header,
        });
        if i == answered_idx {
            q_obj["answers"] = serde_json::json!({ &q.header: selected_label });
        }
        q_array.push(q_obj);
    }
    serde_json::json!({ "questions": q_array }).to_string()
}

use crate::app::update_context::UpdateContext;
use crate::data::Focus;

/// Manages all agent-related state: agent blocks, streaming, permissions.
pub(crate) struct AgentWidget {
    pub blocks: Vec<AgentBlock>,
    pub block_index: HashMap<BlockId, usize>,
    pub active: Option<BlockId>,
    pub event_tx: mpsc::UnboundedSender<AgentEvent>,
    /// Channel to send permission responses back to the TCP permission server.
    pub permission_response_tx: Option<mpsc::UnboundedSender<PermissionDecision>>,
    /// TCP port the permission server is listening on (for CLI spawns).
    pub permission_port: Option<u16>,
    pub cancel_flag: Arc<AtomicBool>,
    pub dirty: bool,
    pub session_id: Option<String>,
    /// Working directory (set during spawn).
    pub cwd: String,
    /// Text input state for free-form answers to AskUserQuestion.
    pub question_input: TextInputState,

    // --- Subscription channel (owned by this widget) ---
    event_rx: Arc<Mutex<mpsc::UnboundedReceiver<AgentEvent>>>,
}

impl AgentWidget {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            blocks: Vec::new(),
            block_index: HashMap::new(),
            active: None,
            event_tx,
            permission_response_tx: None,
            permission_port: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            dirty: false,
            session_id: None,
            cwd: String::new(),
            question_input: {
                let mut qi = TextInputState::new();
                // Default element padding: Padding::new(8.0, 12.0, 8.0, 12.0)
                qi.set_padding(Padding::new(8.0, 12.0, 8.0, 12.0));
                qi
            },
            event_rx: Arc::new(Mutex::new(event_rx)),
        }
    }

    /// Whether the agent has pending output that needs a redraw tick.
    pub fn needs_redraw(&self) -> bool {
        self.dirty
    }

    // ---- View contributions ----

    /// Push a single agent block into the given scroll column.
    pub fn push_block<'a>(
        &'a self,
        scroll: strata::ScrollColumn<'a>,
        block: &'a AgentBlock,
    ) -> strata::ScrollColumn<'a> {
        scroll.push(AgentBlockWidget {
            block,
            question_input: if block.pending_question.is_some() {
                Some(&self.question_input)
            } else {
                None
            },
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
            // User question option buttons
            if let Some(ref question) = block.pending_question {
                // Submit button for free-form text input
                if id == source_ids::agent_question_submit(block.id) {
                    return Self::make_freeform_answer(question, block.id, &self.question_input.text);
                }
                for (q_idx, q) in question.questions.iter().enumerate() {
                    for (o_idx, opt) in q.options.iter().enumerate() {
                        if id == source_ids::agent_question_option(block.id, q_idx, o_idx) {
                            let answer = build_question_answer(&question.questions, q_idx, &opt.label);
                            return Some(AgentMsg::UserQuestionAnswer(
                                block.id,
                                question.tool_use_id.clone(),
                                answer,
                            ));
                        }
                    }
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
    pub fn subscription(&self) -> Subscription<NexusMessage> {
        let rx = self.event_rx.clone();
        agent_subscription(rx).map(|evt| NexusMessage::Agent(AgentMsg::Event(evt)))
    }

    /// Start the TCP permission server (once, reused across spawns).
    fn ensure_permission_server(&mut self) {
        if self.permission_port.is_some() {
            return; // already running
        }

        let listener = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("Failed to bind permission server: {}", e);
                return;
            }
        };
        let port = listener.local_addr().unwrap().port();

        // Convert to async listener
        listener.set_nonblocking(true).unwrap();
        let async_listener = tokio::net::TcpListener::from_std(listener).unwrap();

        let (response_tx, response_rx) = mpsc::unbounded_channel();
        let event_tx = self.event_tx.clone();

        tokio::spawn(async move {
            crate::infra::systems::permission_server::run(async_listener, event_tx, response_rx).await;
        });

        self.permission_response_tx = Some(response_tx);
        self.permission_port = Some(port);
        tracing::info!("Permission server listening on port {}", port);
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
        self.cwd = cwd.to_string();

        // Reset cancel flag
        self.cancel_flag.store(false, Ordering::SeqCst);

        // Ensure permission server is running
        self.ensure_permission_server();

        let agent_tx = self.event_tx.clone();
        let cancel_flag = self.cancel_flag.clone();
        let cwd = PathBuf::from(cwd);
        let session_id = self.session_id.clone();
        let permission_port = self.permission_port;

        tokio::spawn(async move {
            match spawn_agent_task(
                agent_tx,
                cancel_flag,
                contextualized_query,
                cwd,
                attachments,
                session_id,
                permission_port,
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

    /// Handle a message, applying cross-cutting effects via UpdateContext.
    pub fn update(&mut self, msg: AgentMsg, uctx: &mut UpdateContext) {
        match msg {
            AgentMsg::Event(evt) => {
                self.dirty = true;
                self.handle_event(evt, uctx);
            }
            AgentMsg::ToggleThinking(id) => { self.toggle_thinking(id); }
            AgentMsg::ToggleTool(id, idx) => { self.toggle_tool(id, idx); }
            AgentMsg::ExpandAllTools => { self.expand_all_tools(); }
            AgentMsg::PermissionGrant(block_id, perm_id) => { self.permission_grant(block_id, perm_id); }
            AgentMsg::PermissionGrantSession(block_id, perm_id) => { self.permission_grant_session(block_id, perm_id); }
            AgentMsg::PermissionDeny(block_id, perm_id) => { self.permission_deny(block_id, perm_id); }
            AgentMsg::UserQuestionAnswer(block_id, tool_use_id, answer_json) => {
                self.answer_question(block_id, tool_use_id, answer_json);
                uctx.hint_bottom();
            }
            AgentMsg::QuestionInputKey(event) => {
                self.handle_question_key(&event, uctx);
            }
            AgentMsg::QuestionInputMouse(action) => {
                self.question_input.apply_mouse(action);
            }
            AgentMsg::Interrupt => { self.interrupt(); }
        }
    }

    /// Handle an agent event from the streaming channel.
    fn handle_event(&mut self, event: AgentEvent, uctx: &mut UpdateContext) {
        // Handle session ID
        if let AgentEvent::SessionStarted { ref session_id } = event {
            self.session_id = Some(session_id.clone());
        }

        // UserQuestionRequested arrives AFTER Finished (active is None).
        // Handle it on the last block instead.
        if let AgentEvent::UserQuestionRequested { tool_use_id, questions } = event {
            if let Some(block) = self.blocks.last_mut() {
                block.pending_question = Some(crate::data::agent_block::PendingUserQuestion {
                    tool_use_id,
                    questions,
                });
                block.state = AgentBlockState::AwaitingPermission;
                // Clear the error-path response text that the CLI generated
                // after AskUserQuestion failed. We'll get fresh output on resume.
                block.response.clear();
                block.version += 1;
            }
            // Clear stale text; focus will be set by the orchestrator.
            self.question_input.text.clear();
            self.question_input.cursor = 0;
            uctx.set_focus(Focus::AgentInput);
            uctx.hint_bottom();
            return;
        }

        if let Some(block_id) = self.active {
            if let Some(&idx) = self.block_index.get(&block_id) {
                if let Some(block) = self.blocks.get_mut(idx) {
                    match event {
                        AgentEvent::SessionStarted { .. } => {}
                        AgentEvent::UserQuestionRequested { .. } => unreachable!(),
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
                        AgentEvent::UsageUpdate {
                            cost_usd,
                            input_tokens,
                            output_tokens,
                        } => {
                            block.cost_usd = cost_usd;
                            block.input_tokens = input_tokens;
                            block.output_tokens = output_tokens;
                            block.version += 1;
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

        uctx.hint_bottom();
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

    /// Toggle all tools in the most recent agent block (Ctrl+O).
    /// If any are collapsed, expand all. Otherwise collapse all.
    pub fn expand_all_tools(&mut self) {
        if let Some(block) = self.blocks.last_mut() {
            let any_collapsed = block.tools.iter().any(|t| t.collapsed);
            let new_state = !any_collapsed; // If any collapsed, expand all (false); otherwise collapse all (true)
            for tool in &mut block.tools {
                tool.collapsed = new_state;
            }
            block.version += 1;
        }
    }

    /// Grant permission once.
    pub fn permission_grant(&mut self, block_id: BlockId, _perm_id: String) {
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.clear_permission();
            }
        }
        if let Some(ref tx) = self.permission_response_tx {
            let _ = tx.send(PermissionDecision::Allow);
        }
    }

    /// Grant permission for session.
    pub fn permission_grant_session(&mut self, block_id: BlockId, _perm_id: String) {
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.clear_permission();
            }
        }
        if let Some(ref tx) = self.permission_response_tx {
            let _ = tx.send(PermissionDecision::Allow);
        }
    }

    /// Deny permission.
    pub fn permission_deny(&mut self, block_id: BlockId, _perm_id: String) {
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.clear_permission();
                block.fail("Permission denied".to_string());
            }
        }
        if let Some(ref tx) = self.permission_response_tx {
            let _ = tx.send(PermissionDecision::Deny);
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

    /// Answer a pending user question via MCP permission response.
    pub fn answer_question(&mut self, block_id: BlockId, _tool_use_id: String, answer_json: String) {
        // Clear pending question from the block and reset the free-form input.
        self.question_input.text.clear();
        self.question_input.cursor = 0;
        if let Some(&idx) = self.block_index.get(&block_id) {
            if let Some(block) = self.blocks.get_mut(idx) {
                block.pending_question = None;
                block.state = AgentBlockState::Streaming;
                block.version += 1;
            }
        }

        // Send the answer back through the permission channel.
        // The permission server will inject it into updatedInput.answers.
        if let Some(ref tx) = self.permission_response_tx {
            // Parse the answer_json to extract the answers map.
            // answer_json is like: {"questions":[{"question":"...","header":"...","answers":{"Header":"Selected"}}]}
            // We need to extract all answers into a flat {"Header": "Selected"} map.
            let answers = if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&answer_json) {
                let mut map = serde_json::Map::new();
                if let Some(questions) = parsed.get("questions").and_then(|q| q.as_array()) {
                    for q in questions {
                        if let Some(answers_obj) = q.get("answers").and_then(|a| a.as_object()) {
                            for (k, v) in answers_obj {
                                map.insert(k.clone(), v.clone());
                            }
                        }
                    }
                }
                serde_json::Value::Object(map)
            } else {
                serde_json::Value::Object(Default::default())
            };

            let _ = tx.send(PermissionDecision::Answer(answers));
        }
    }

    /// Build a `UserQuestionAnswer` message from free-form text input.
    /// Returns `None` if the text is empty.
    fn make_freeform_answer(
        question: &crate::data::agent_block::PendingUserQuestion,
        block_id: BlockId,
        text: &str,
    ) -> Option<AgentMsg> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let answer = build_question_answer(&question.questions, 0, text);
        Some(AgentMsg::UserQuestionAnswer(
            block_id,
            question.tool_use_id.clone(),
            answer,
        ))
    }

    /// Handle a key event for the question free-form text input.
    fn handle_question_key(&mut self, event: &KeyEvent, uctx: &mut UpdateContext) {
        use strata::text_input_state::TextInputAction;

        match self.question_input.handle_key(event, false) {
            TextInputAction::Submit(text) => {
                if let Some(block) = self.blocks.iter().find(|b| b.pending_question.is_some()) {
                    let block_id = block.id;
                    if let Some(ref question) = block.pending_question {
                        if let Some(AgentMsg::UserQuestionAnswer(bid, tid, answer)) =
                            Self::make_freeform_answer(question, block_id, &text)
                        {
                            self.answer_question(bid, tid, answer);
                            uctx.hint_bottom();
                        }
                    }
                }
            }
            TextInputAction::Changed | TextInputAction::Blur | TextInputAction::Noop => {}
        }
    }

    /// Check if an agent is currently active.
    pub fn is_active(&self) -> bool {
        self.active.is_some()
    }

    /// Check if any agent block has a pending user question.
    pub fn has_pending_question(&self) -> bool {
        self.blocks.iter().any(|b| b.pending_question.is_some())
    }

}
