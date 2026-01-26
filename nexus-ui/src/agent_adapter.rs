//! Agent adapter for Iced UI.
//!
//! This module provides the bridge between nexus-agent and the Iced UI,
//! implementing the UserInterface trait for agent communication.

use async_trait::async_trait;
use nexus_agent::ui::{DisplayFragment, ToolStatus as AgentToolStatus, UiEvent, UIError, UserInterface};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::agent_block::ToolStatus;

/// Events sent from agent to UI.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Agent started processing.
    Started { request_id: u64 },
    /// Agent produced response text.
    ResponseText(String),
    /// Agent is thinking (reasoning).
    ThinkingText(String),
    /// Tool invocation started.
    ToolStarted { id: String, name: String },
    /// Tool parameter being streamed.
    ToolParameter { tool_id: String, name: String, value: String },
    /// Tool output chunk.
    ToolOutput { tool_id: String, chunk: String },
    /// Tool ended (before result).
    ToolEnded { id: String },
    /// Tool status update.
    ToolStatus {
        id: String,
        status: ToolStatus,
        message: Option<String>,
        output: Option<String>,
    },
    /// Image added.
    ImageAdded { media_type: String, data: String },
    /// Permission requested.
    PermissionRequested {
        id: String,
        tool_name: String,
        tool_id: String,
        description: String,
        action: String,
        working_dir: Option<String>,
    },
    /// Agent finished processing.
    Finished { request_id: u64 },
    /// Agent was cancelled.
    Cancelled { request_id: u64 },
    /// Agent encountered an error.
    Error(String),
}

/// Convert agent tool status to our tool status.
fn convert_tool_status(status: AgentToolStatus) -> ToolStatus {
    match status {
        AgentToolStatus::Pending => ToolStatus::Pending,
        AgentToolStatus::Running => ToolStatus::Running,
        AgentToolStatus::Success => ToolStatus::Success,
        AgentToolStatus::Error => ToolStatus::Error,
    }
}

/// Iced UI adapter that implements nexus_agent::ui::UserInterface.
pub struct IcedAgentUI {
    /// Channel to send events to the Iced app.
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    /// Flag to cancel streaming.
    cancel_flag: Arc<AtomicBool>,
}

impl IcedAgentUI {
    /// Create a new IcedAgentUI with a channel for events.
    pub fn new(event_tx: mpsc::UnboundedSender<AgentEvent>) -> Self {
        Self {
            event_tx,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create a new IcedAgentUI with an external cancel flag.
    pub fn with_cancel_flag(
        event_tx: mpsc::UnboundedSender<AgentEvent>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Self {
        Self {
            event_tx,
            cancel_flag,
        }
    }

    /// Get a handle to the cancel flag for external cancellation.
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        self.cancel_flag.clone()
    }

    /// Cancel the current operation.
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    /// Reset the cancel flag for a new operation.
    pub fn reset_cancel(&self) {
        self.cancel_flag.store(false, Ordering::SeqCst);
    }

    /// Send an event to the UI (skips if cancelled).
    fn send(&self, event: AgentEvent) {
        // Don't send events if cancelled (prevents issues during cleanup)
        if !self.cancel_flag.load(Ordering::Relaxed) {
            let _ = self.event_tx.send(event);
        }
    }
}

#[async_trait]
impl UserInterface for IcedAgentUI {
    async fn send_event(&self, event: UiEvent) -> Result<(), UIError> {
        match event {
            UiEvent::StreamingStarted(request_id) => {
                self.send(AgentEvent::Started { request_id });
            }
            UiEvent::StreamingStopped { id, cancelled, error } => {
                if let Some(err) = error {
                    self.send(AgentEvent::Error(err));
                } else if cancelled {
                    self.send(AgentEvent::Cancelled { request_id: id });
                } else {
                    self.send(AgentEvent::Finished { request_id: id });
                }
            }
            UiEvent::StartTool { name, id } => {
                self.send(AgentEvent::ToolStarted { id, name });
            }
            UiEvent::UpdateToolParameter { tool_id, name, value } => {
                self.send(AgentEvent::ToolParameter { tool_id, name, value });
            }
            UiEvent::UpdateToolStatus { tool_id, status, message, output } => {
                self.send(AgentEvent::ToolStatus {
                    id: tool_id,
                    status: convert_tool_status(status),
                    message,
                    output,
                });
            }
            UiEvent::EndTool { id } => {
                self.send(AgentEvent::ToolEnded { id });
            }
            UiEvent::AppendToolOutput { tool_id, chunk } => {
                self.send(AgentEvent::ToolOutput { tool_id, chunk });
            }
            UiEvent::DisplayError { message } => {
                self.send(AgentEvent::Error(message));
            }
            UiEvent::AppendToTextBlock { content } => {
                self.send(AgentEvent::ResponseText(content));
            }
            UiEvent::AppendToThinkingBlock { content } => {
                self.send(AgentEvent::ThinkingText(content));
            }
            UiEvent::AddImage { media_type, data } => {
                self.send(AgentEvent::ImageAdded { media_type, data });
            }
            _ => {
                // Other events not needed for basic UI
            }
        }
        Ok(())
    }

    fn display_fragment(&self, fragment: &DisplayFragment) -> Result<(), UIError> {
        match fragment {
            DisplayFragment::PlainText(text) => {
                self.send(AgentEvent::ResponseText(text.clone()));
            }
            DisplayFragment::ThinkingText(text) => {
                self.send(AgentEvent::ThinkingText(text.clone()));
            }
            DisplayFragment::ToolName { name, id } => {
                self.send(AgentEvent::ToolStarted {
                    id: id.clone(),
                    name: name.clone(),
                });
            }
            DisplayFragment::ToolParameter { tool_id, name, value } => {
                self.send(AgentEvent::ToolParameter {
                    tool_id: tool_id.clone(),
                    name: name.clone(),
                    value: value.clone(),
                });
            }
            DisplayFragment::ToolEnd { id } => {
                self.send(AgentEvent::ToolEnded { id: id.clone() });
            }
            DisplayFragment::ToolOutput { tool_id, chunk } => {
                self.send(AgentEvent::ToolOutput {
                    tool_id: tool_id.clone(),
                    chunk: chunk.clone(),
                });
            }
            DisplayFragment::Image { media_type, data } => {
                self.send(AgentEvent::ImageAdded {
                    media_type: media_type.clone(),
                    data: data.clone(),
                });
            }
            DisplayFragment::ReasoningSummaryStart => {
                // Start of reasoning summary item
            }
            DisplayFragment::ReasoningSummaryDelta(delta) => {
                self.send(AgentEvent::ThinkingText(delta.clone()));
            }
            DisplayFragment::ReasoningComplete => {
                // Reasoning complete
            }
            _ => {
                // Handle other fragment types as needed
            }
        }
        Ok(())
    }

    fn should_streaming_continue(&self) -> bool {
        !self.cancel_flag.load(Ordering::Relaxed)
    }

    fn notify_rate_limit(&self, seconds_remaining: u64) {
        self.send(AgentEvent::Error(format!(
            "Rate limited. Waiting {} seconds...",
            seconds_remaining
        )));
    }

    fn clear_rate_limit(&self) {
        // Rate limit cleared, continue
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Permission mediator that sends permission requests through the Iced UI.
pub struct IcedPermissionMediator {
    /// Channel to send permission requests.
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    /// Channel to receive permission responses.
    response_rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<(String, PermissionResponse)>>>,
    /// Sender for permission responses (held by UI).
    response_tx: mpsc::UnboundedSender<(String, PermissionResponse)>,
}

/// Permission response from user.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PermissionResponse {
    GrantedOnce,
    GrantedSession,
    Denied,
}

impl IcedPermissionMediator {
    /// Create a new permission mediator.
    pub fn new(event_tx: mpsc::UnboundedSender<AgentEvent>) -> Self {
        let (response_tx, response_rx) = mpsc::unbounded_channel();
        Self {
            event_tx,
            response_rx: Arc::new(tokio::sync::Mutex::new(response_rx)),
            response_tx,
        }
    }

    /// Get the response sender for the UI to use.
    pub fn response_sender(&self) -> mpsc::UnboundedSender<(String, PermissionResponse)> {
        self.response_tx.clone()
    }

    /// Request permission for an action.
    pub async fn request_permission(
        &self,
        id: String,
        tool_name: String,
        tool_id: String,
        description: String,
        action: String,
        working_dir: Option<String>,
    ) -> PermissionResponse {
        // Send request to UI
        let _ = self.event_tx.send(AgentEvent::PermissionRequested {
            id: id.clone(),
            tool_name,
            tool_id,
            description,
            action,
            working_dir,
        });

        // Wait for response
        let mut rx = self.response_rx.lock().await;
        while let Some((resp_id, response)) = rx.recv().await {
            if resp_id == id {
                return response;
            }
        }

        // Default to denied if channel closed
        PermissionResponse::Denied
    }
}
