//! Agent domain handler.
//!
//! Handles AI agent events, widget interactions, and query spawning.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use iced::widget::scrollable;
use iced::Task;

use crate::agent_adapter::{AgentEvent, PermissionResponse};
use crate::agent_block::{AgentBlock, AgentBlockState, PermissionRequest};
use crate::agent_widgets::AgentWidgetMessage;
use crate::constants::HISTORY_SCROLLABLE;
use crate::msg::{AgentMessage, Message};
use crate::shell_context::build_shell_context;
use crate::state::Nexus;
use crate::systems::spawn_agent_task;

/// Update the agent domain state.
pub fn update(state: &mut Nexus, msg: AgentMessage) -> Task<Message> {
    match msg {
        AgentMessage::Event(evt) => handle_event(state, evt),
        AgentMessage::Widget(widget_msg) => handle_widget(state, widget_msg),
        AgentMessage::Cancel => cancel(state),
    }
}

// =============================================================================
// Agent Events
// =============================================================================

/// Handle agent events from the agent adapter.
pub fn handle_event(state: &mut Nexus, event: AgentEvent) -> Task<Message> {
    // Mark agent dirty to ensure UI updates
    state.agent.is_dirty = true;

    let agent = &mut state.agent;

    if let Some(block_id) = agent.active_block {
        if let Some(idx) = agent.block_index.get(&block_id) {
            if let Some(block) = agent.blocks.get_mut(*idx) {
                match event {
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
                    }
                    AgentEvent::Cancelled { .. } => {
                        block.fail("Cancelled".to_string());
                        agent.active_block = None;
                    }
                    AgentEvent::Error(err) => {
                        block.fail(err);
                        agent.active_block = None;
                    }
                }
            }
        }
    }
    scrollable::snap_to(
        scrollable::Id::new(HISTORY_SCROLLABLE),
        scrollable::RelativeOffset::END,
    )
}

// =============================================================================
// Agent Widget Interactions
// =============================================================================

/// Handle agent widget interactions.
pub fn handle_widget(state: &mut Nexus, widget_msg: AgentWidgetMessage) -> Task<Message> {
    let agent = &mut state.agent;

    match widget_msg {
        AgentWidgetMessage::ToggleThinking(block_id) => {
            if let Some(idx) = agent.block_index.get(&block_id) {
                if let Some(block) = agent.blocks.get_mut(*idx) {
                    block.toggle_thinking();
                }
            }
        }
        AgentWidgetMessage::ToggleTool(block_id, tool_id) => {
            if let Some(idx) = agent.block_index.get(&block_id) {
                if let Some(block) = agent.blocks.get_mut(*idx) {
                    block.toggle_tool(&tool_id);
                }
            }
        }
        AgentWidgetMessage::PermissionGranted(block_id, perm_id) => {
            if let Some(idx) = agent.block_index.get(&block_id) {
                if let Some(block) = agent.blocks.get_mut(*idx) {
                    block.clear_permission();
                }
            }
            if let Some(ref tx) = agent.permission_tx {
                let _ = tx.send((perm_id, PermissionResponse::GrantedOnce));
            }
        }
        AgentWidgetMessage::PermissionGrantedSession(block_id, perm_id) => {
            if let Some(idx) = agent.block_index.get(&block_id) {
                if let Some(block) = agent.blocks.get_mut(*idx) {
                    block.clear_permission();
                }
            }
            if let Some(ref tx) = agent.permission_tx {
                let _ = tx.send((perm_id, PermissionResponse::GrantedSession));
            }
        }
        AgentWidgetMessage::PermissionDenied(block_id, perm_id) => {
            if let Some(idx) = agent.block_index.get(&block_id) {
                if let Some(block) = agent.blocks.get_mut(*idx) {
                    block.clear_permission();
                    block.fail("Permission denied".to_string());
                }
            }
            if let Some(ref tx) = agent.permission_tx {
                let _ = tx.send((perm_id, PermissionResponse::Denied));
            }
            agent.active_block = None;
        }
        AgentWidgetMessage::CopyText(text) => {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(&text);
            }
        }
    }
    Task::none()
}

// =============================================================================
// Agent Control
// =============================================================================

/// Cancel the current agent operation.
pub fn cancel(state: &mut Nexus) -> Task<Message> {
    let agent = &mut state.agent;

    if let Some(block_id) = agent.active_block {
        if let Some(idx) = agent.block_index.get(&block_id) {
            if let Some(block) = agent.blocks.get_mut(*idx) {
                block.fail("Cancelled by user".to_string());
            }
        }
        agent.active_block = None;
    }
    Task::none()
}

/// Spawn an agent query task.
pub fn spawn_query(state: &mut Nexus, query: String) -> Task<Message> {
    // Build shell context
    let shell_context = build_shell_context(
        &state.terminal.cwd,
        &state.terminal.blocks,
        &state.input.history,
    );

    let contextualized_query = format!("{}{}", shell_context, query);
    tracing::info!("Agent query: {}", query);

    // Create block with shared ID counter
    let block_id = state.terminal.next_id();
    let agent_block = AgentBlock::new(block_id, query.clone());
    state.agent.add_block(agent_block);
    state.agent.active_block = Some(block_id);

    // Reset cancel flag
    state.agent.cancel_flag.store(false, Ordering::SeqCst);

    // Spawn agent task
    let agent_tx = state.agent.event_tx.clone();
    let cancel_flag = state.agent.cancel_flag.clone();
    let cwd = PathBuf::from(&state.terminal.cwd);

    tokio::spawn(async move {
        if let Err(e) = spawn_agent_task(agent_tx, cancel_flag, contextualized_query, cwd).await {
            tracing::error!("Agent task failed: {}", e);
        }
    });

    // Mark block as streaming
    if let Some(idx) = state.agent.block_index.get(&block_id) {
        if let Some(block) = state.agent.blocks.get_mut(*idx) {
            block.state = AgentBlockState::Streaming;
        }
    }

    scrollable::snap_to(
        scrollable::Id::new(HISTORY_SCROLLABLE),
        scrollable::RelativeOffset::END,
    )
}
