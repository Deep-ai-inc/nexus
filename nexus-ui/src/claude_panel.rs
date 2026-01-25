//! Claude Panel - Native Claude Code UI widget.
//!
//! Renders Claude's structured output as a series of blocks:
//! - User prompts
//! - Thinking blocks (collapsible)
//! - Assistant messages (markdown)
//! - Tool executions (with status)
//! - Permission requests (interactive buttons)

use iced::widget::{
    button, column, container, row, scrollable, text, text_input, Column,
};
use iced::{Alignment, Element, Length, Padding, Task, Theme};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

use nexus_claude::{
    ClaudeBlock, ClaudeConversation, ClaudeOutput, ClaudeSession,
    ConversationState, DiffStatus, MessageLevel, ReaderMessage, ToolStatus,
    ContentBlock,
};

/// Command sent to the Claude worker thread.
#[derive(Debug)]
pub enum ClaudeCommand {
    SendMessage(String),
    Close,
}

/// Messages for the Claude panel.
#[derive(Debug, Clone)]
pub enum ClaudePanelMessage {
    /// Input text changed.
    InputChanged(String),

    /// User submitted a prompt.
    Submit,

    /// Toggle thinking block expansion.
    ToggleThinking(usize),

    /// Respond to permission request.
    PermissionResponse { block_index: usize, allow: bool },

    /// Apply a code diff.
    ApplyDiff(usize),

    /// Reject a code diff.
    RejectDiff(usize),

    /// Close the panel.
    Close,

    /// Output from Claude process.
    ClaudeOutput(ClaudeOutputWrapper),

    /// Claude process closed.
    ProcessClosed,

    /// Error from Claude process.
    ProcessError(String),

    /// Session is ready.
    SessionReady,

    /// Failed to create session.
    SessionFailed(String),
}

/// Wrapper for ClaudeOutput to implement Clone.
#[derive(Debug, Clone)]
pub struct ClaudeOutputWrapper(pub Arc<ClaudeOutput>);

/// State for the Claude panel.
pub struct ClaudePanel {
    /// Current input text.
    input: String,

    /// Conversation state.
    conversation: ClaudeConversation,

    /// Whether the panel is visible.
    visible: bool,

    /// Working directory.
    cwd: PathBuf,

    /// Receiver for Claude events (shared with subscription).
    event_rx: Option<Arc<Mutex<mpsc::UnboundedReceiver<ReaderMessage>>>>,

    /// Sender for commands to worker thread.
    command_tx: Option<mpsc::UnboundedSender<ClaudeCommand>>,

    /// Whether we're waiting for a response.
    waiting: bool,

    /// Whether session is ready.
    session_ready: bool,
}

impl ClaudePanel {
    /// Create a new Claude panel.
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            input: String::new(),
            conversation: ClaudeConversation::with_cwd(cwd.clone()),
            visible: false,
            cwd,
            event_rx: None,
            command_tx: None,
            waiting: false,
            session_ready: false,
        }
    }

    /// Open the panel and optionally set an initial prompt.
    pub fn open(&mut self, initial_prompt: Option<String>) -> Task<ClaudePanelMessage> {
        self.visible = true;

        if let Some(prompt) = initial_prompt {
            self.input = prompt;
        }

        // If no session, create one via background worker
        if self.command_tx.is_none() {
            tracing::info!("Opening Claude panel, spawning worker...");

            // Create channels
            let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<ClaudeCommand>();
            let (event_tx, event_rx) = mpsc::unbounded_channel::<ReaderMessage>();

            self.command_tx = Some(cmd_tx);
            self.event_rx = Some(Arc::new(Mutex::new(event_rx)));

            let cwd = self.cwd.clone();

            // Spawn worker thread that owns the session
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().expect("Failed to create runtime");
                rt.block_on(async move {
                    claude_worker(cwd, cmd_rx, event_tx).await;
                });
            });

            // Return a task that waits a bit for session to initialize
            // The actual ready signal comes via the event channel
            return Task::none();
        }

        Task::none()
    }

    /// Close the panel.
    pub fn close(&mut self) {
        self.visible = false;
        if let Some(tx) = self.command_tx.take() {
            let _ = tx.send(ClaudeCommand::Close);
        }
        self.event_rx = None;
        self.session_ready = false;
        self.waiting = false;
    }

    /// Check if the panel is visible.
    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Get the event receiver for subscription.
    pub fn event_receiver(&self) -> Option<Arc<Mutex<mpsc::UnboundedReceiver<ReaderMessage>>>> {
        self.event_rx.clone()
    }

    /// Update the panel state based on a message.
    pub fn update(&mut self, message: ClaudePanelMessage) -> Task<ClaudePanelMessage> {
        match message {
            ClaudePanelMessage::InputChanged(text) => {
                self.input = text;
            }

            ClaudePanelMessage::Submit => {
                if !self.input.trim().is_empty() && !self.waiting {
                    let prompt = std::mem::take(&mut self.input);
                    self.conversation.add_user_prompt(prompt.clone());
                    self.waiting = true;

                    // Send message to worker
                    if let Some(tx) = &self.command_tx {
                        if tx.send(ClaudeCommand::SendMessage(prompt)).is_err() {
                            self.waiting = false;
                            self.conversation.blocks.push(ClaudeBlock::SystemMessage {
                                level: MessageLevel::Error,
                                content: "Failed to send message - session closed".to_string(),
                            });
                        }
                    } else {
                        self.waiting = false;
                        self.conversation.blocks.push(ClaudeBlock::SystemMessage {
                            level: MessageLevel::Error,
                            content: "No active session".to_string(),
                        });
                    }
                }
            }

            ClaudePanelMessage::ToggleThinking(index) => {
                if let Some(ClaudeBlock::Thinking { expanded, .. }) =
                    self.conversation.blocks.get_mut(index)
                {
                    *expanded = !*expanded;
                }
            }

            ClaudePanelMessage::PermissionResponse { block_index, allow } => {
                if let Some(ClaudeBlock::PermissionRequest { response, .. }) =
                    self.conversation.blocks.get_mut(block_index)
                {
                    *response = Some(allow);
                }
            }

            ClaudePanelMessage::ApplyDiff(index) => {
                if let Some(ClaudeBlock::CodeDiff { status, .. }) =
                    self.conversation.blocks.get_mut(index)
                {
                    *status = DiffStatus::Approved;
                }
            }

            ClaudePanelMessage::RejectDiff(index) => {
                if let Some(ClaudeBlock::CodeDiff { status, .. }) =
                    self.conversation.blocks.get_mut(index)
                {
                    *status = DiffStatus::Rejected;
                }
            }

            ClaudePanelMessage::Close => {
                self.close();
            }

            ClaudePanelMessage::ClaudeOutput(wrapper) => {
                self.handle_claude_output(&wrapper.0);
            }

            ClaudePanelMessage::ProcessClosed => {
                self.waiting = false;
                self.conversation.set_idle();
            }

            ClaudePanelMessage::ProcessError(error) => {
                self.waiting = false;
                self.conversation.blocks.push(ClaudeBlock::SystemMessage {
                    level: MessageLevel::Error,
                    content: error,
                });
            }

            ClaudePanelMessage::SessionReady => {
                self.session_ready = true;
                tracing::info!("Claude session is ready");
            }

            ClaudePanelMessage::SessionFailed(error) => {
                self.conversation.blocks.push(ClaudeBlock::SystemMessage {
                    level: MessageLevel::Error,
                    content: format!("Failed to create session: {}", error),
                });
            }
        }

        Task::none()
    }

    /// Handle a Claude output message.
    fn handle_claude_output(&mut self, output: &ClaudeOutput) {
        tracing::debug!("Claude output: {:?}", output.message_type());

        match output {
            ClaudeOutput::System(system_msg) => {
                // System message - try to parse as init
                if let Some(init) = system_msg.as_init() {
                    self.session_ready = true;
                    let info = format!(
                        "Session: {} | Model: {} | CWD: {}",
                        init.session_id,
                        init.model.as_deref().unwrap_or("unknown"),
                        init.cwd.as_deref().unwrap_or("unknown")
                    );
                    self.conversation.blocks.push(ClaudeBlock::SystemMessage {
                        level: MessageLevel::Info,
                        content: info,
                    });
                }
            }

            ClaudeOutput::Assistant(assistant_msg) => {
                // Process content blocks
                for block in &assistant_msg.message.content {
                    match block {
                        ContentBlock::Text(text_block) => {
                            // Ensure we have an assistant message block
                            if !matches!(self.conversation.state, ConversationState::Responding) {
                                self.conversation.start_assistant_message();
                            }
                            self.conversation.append_assistant_message(&text_block.text);
                        }

                        ContentBlock::Thinking(thinking_block) => {
                            if !matches!(self.conversation.state, ConversationState::Thinking) {
                                self.conversation.start_thinking();
                            }
                            self.conversation.append_thinking(&thinking_block.thinking);
                        }

                        ContentBlock::ToolUse(tool_use) => {
                            self.conversation.start_tool(
                                tool_use.name.clone(),
                                tool_use.id.clone(),
                            );
                            // Tool input is already a Value
                            self.conversation.set_tool_input(&tool_use.id, tool_use.input.clone());
                        }

                        ContentBlock::ToolResult(tool_result) => {
                            let is_error = tool_result.is_error.unwrap_or(false);
                            self.conversation.set_tool_result(
                                &tool_result.tool_use_id,
                                serde_json::json!({ "content": tool_result.content }),
                                is_error,
                            );
                        }

                        _ => {
                            // Other block types (Image, etc.)
                            tracing::debug!("Unhandled content block type");
                        }
                    }
                }

                // End the message if stop_reason is present
                if assistant_msg.message.stop_reason.is_some() {
                    self.conversation.end_assistant_message();
                    self.waiting = false;
                }
            }

            ClaudeOutput::Result(result_msg) => {
                // Final result - conversation turn complete
                self.waiting = false;
                self.conversation.set_idle();

                // Log cost info
                if result_msg.total_cost_usd > 0.0 {
                    tracing::info!("Turn cost: ${:.4}", result_msg.total_cost_usd);
                }
            }

            ClaudeOutput::User(_) => {
                // Echo of user message, ignore
            }

            ClaudeOutput::ControlRequest(_) | ClaudeOutput::ControlResponse(_) => {
                // Control messages for permissions, ignore for now
                tracing::debug!("Control message: {:?}", output.message_type());
            }
        }
    }

    /// Render the panel.
    pub fn view(&self, font_size: f32) -> Element<'static, ClaudePanelMessage> {
        if !self.visible {
            return column![].into();
        }

        let header = self.view_header();
        let conversation_view = self.view_conversation(font_size);
        let input_area = self.view_input(font_size);

        let content = column![header, conversation_view, input_area]
            .spacing(10)
            .padding(Padding::from([10, 15]));

        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|theme: &Theme| {
                container::Style {
                    background: Some(theme.palette().background.into()),
                    ..Default::default()
                }
            })
            .into()
    }

    fn view_header(&self) -> Element<'static, ClaudePanelMessage> {
        let title = text("Claude").size(18);
        // Show "Ready" when we have a command channel (worker is up)
        let status = if self.waiting {
            text("Thinking...").size(14).color([0.5, 0.8, 0.5])
        } else if self.command_tx.is_some() {
            text("Ready").size(14).color([0.5, 0.8, 0.5])
        } else {
            text("Connecting...").size(14).color([0.8, 0.8, 0.5])
        };
        let close_btn = button(text("X").size(14))
            .on_press(ClaudePanelMessage::Close)
            .padding(Padding::from([2, 8]));

        row![title, status, close_btn]
            .spacing(15)
            .align_y(Alignment::Center)
            .into()
    }

    fn view_conversation(&self, font_size: f32) -> Element<'static, ClaudePanelMessage> {
        let mut blocks_col = Column::new().spacing(10);

        for (index, block) in self.conversation.blocks.iter().enumerate() {
            let block_view = self.view_block(block, index, font_size);
            blocks_col = blocks_col.push(block_view);
        }

        scrollable(blocks_col)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn view_block(
        &self,
        block: &ClaudeBlock,
        index: usize,
        font_size: f32,
    ) -> Element<'static, ClaudePanelMessage> {
        match block {
            ClaudeBlock::UserPrompt { text: content, .. } => {
                self.view_user_prompt(content, font_size)
            }
            ClaudeBlock::Thinking {
                content, expanded, ..
            } => self.view_thinking(content, *expanded, index, font_size),
            ClaudeBlock::AssistantMessage { markdown, .. } => {
                self.view_assistant_message(markdown, font_size)
            }
            ClaudeBlock::ToolExecution {
                tool_name,
                status,
                output,
                ..
            } => self.view_tool_execution(tool_name, status, output, font_size),
            ClaudeBlock::CodeDiff {
                file_path,
                status,
                new_content,
                ..
            } => self.view_code_diff(file_path, new_content, status, index, font_size),
            ClaudeBlock::PermissionRequest {
                tool,
                description,
                response,
            } => self.view_permission_request(tool, description, response, index, font_size),
            ClaudeBlock::SystemMessage { level, content } => {
                self.view_system_message(level, content, font_size)
            }
        }
    }

    fn view_user_prompt(&self, content: &str, font_size: f32) -> Element<'static, ClaudePanelMessage> {
        let label = text("You").size(font_size).color([0.3, 0.7, 0.3]);
        let content_text = text(content.to_string()).size(font_size);

        container(column![label, content_text].spacing(5))
            .padding(10)
            .width(Length::Fill)
            .style(|_theme: &Theme| container::Style {
                background: Some(iced::Color::from_rgb(0.15, 0.2, 0.15).into()),
                border: iced::Border {
                    radius: 5.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    fn view_thinking(
        &self,
        content: &str,
        expanded: bool,
        index: usize,
        font_size: f32,
    ) -> Element<'static, ClaudePanelMessage> {
        let toggle_text = if expanded { "▼ Thinking" } else { "▶ Thinking" };
        let toggle_btn = button(text(toggle_text).size(font_size))
            .on_press(ClaudePanelMessage::ToggleThinking(index))
            .padding(5);

        let mut col = column![toggle_btn].spacing(5);

        if expanded {
            col = col.push(
                container(text(content.to_string()).size(font_size - 1.0))
                    .padding(10)
                    .style(|_theme: &Theme| container::Style {
                        background: Some(iced::Color::from_rgb(0.12, 0.12, 0.15).into()),
                        ..Default::default()
                    }),
            );
        }

        container(col)
            .padding(5)
            .width(Length::Fill)
            .into()
    }

    fn view_assistant_message(&self, content: &str, font_size: f32) -> Element<'static, ClaudePanelMessage> {
        let label = text("Claude").size(font_size).color([0.4, 0.6, 0.9]);
        let content_text = text(content.to_string()).size(font_size);

        container(column![label, content_text].spacing(5))
            .padding(10)
            .width(Length::Fill)
            .style(|_theme: &Theme| container::Style {
                background: Some(iced::Color::from_rgb(0.12, 0.15, 0.2).into()),
                border: iced::Border {
                    radius: 5.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    fn view_tool_execution(
        &self,
        tool_name: &str,
        status: &ToolStatus,
        _output: &Option<serde_json::Value>,
        font_size: f32,
    ) -> Element<'static, ClaudePanelMessage> {
        let status_indicator = match status {
            ToolStatus::Pending => "○",
            ToolStatus::Running => "●",
            ToolStatus::Success => "✓",
            ToolStatus::Error => "✗",
        };

        let status_color = match status {
            ToolStatus::Pending => [0.5, 0.5, 0.5],
            ToolStatus::Running => [0.9, 0.7, 0.2],
            ToolStatus::Success => [0.3, 0.8, 0.3],
            ToolStatus::Error => [0.9, 0.3, 0.3],
        };

        let header = row![
            text(status_indicator).size(font_size).color(status_color),
            text(tool_name.to_string()).size(font_size),
        ]
        .spacing(10);

        container(header)
            .padding(8)
            .width(Length::Fill)
            .style(|_theme: &Theme| container::Style {
                background: Some(iced::Color::from_rgb(0.1, 0.1, 0.12).into()),
                border: iced::Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    fn view_code_diff(
        &self,
        file_path: &str,
        _new_content: &str,
        status: &DiffStatus,
        index: usize,
        font_size: f32,
    ) -> Element<'static, ClaudePanelMessage> {
        let header = text(format!("Diff: {}", file_path)).size(font_size);

        let buttons = match status {
            DiffStatus::Proposed => {
                row![
                    button(text("Apply")).on_press(ClaudePanelMessage::ApplyDiff(index)),
                    button(text("Reject")).on_press(ClaudePanelMessage::RejectDiff(index)),
                ]
                .spacing(10)
            }
            _ => row![text(format!("{:?}", status)).size(font_size - 2.0)],
        };

        container(column![header, buttons].spacing(5))
            .padding(10)
            .width(Length::Fill)
            .into()
    }

    fn view_permission_request(
        &self,
        tool: &str,
        description: &str,
        response: &Option<bool>,
        index: usize,
        font_size: f32,
    ) -> Element<'static, ClaudePanelMessage> {
        let header = text(format!("Permission: {}", tool)).size(font_size);
        let desc = text(description.to_string()).size(font_size - 1.0);

        let buttons = if response.is_none() {
            row![
                button(text("Allow")).on_press(ClaudePanelMessage::PermissionResponse {
                    block_index: index,
                    allow: true
                }),
                button(text("Deny")).on_press(ClaudePanelMessage::PermissionResponse {
                    block_index: index,
                    allow: false
                }),
            ]
            .spacing(10)
        } else {
            row![text(if response.unwrap() { "Allowed" } else { "Denied" }).size(font_size - 2.0)]
        };

        container(column![header, desc, buttons].spacing(5))
            .padding(10)
            .width(Length::Fill)
            .style(|_theme: &Theme| container::Style {
                background: Some(iced::Color::from_rgb(0.2, 0.15, 0.1).into()),
                border: iced::Border {
                    radius: 5.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    }

    fn view_system_message(
        &self,
        level: &MessageLevel,
        content: &str,
        font_size: f32,
    ) -> Element<'static, ClaudePanelMessage> {
        let (label, color) = match level {
            MessageLevel::Info => ("INFO", [0.5, 0.7, 0.5]),
            MessageLevel::Warning => ("WARN", [0.9, 0.7, 0.2]),
            MessageLevel::Error => ("ERROR", [0.9, 0.3, 0.3]),
        };

        let label_text = text(label).size(font_size - 2.0).color(color);
        let content_text = text(content.to_string()).size(font_size - 1.0);

        container(row![label_text, content_text].spacing(10))
            .padding(5)
            .width(Length::Fill)
            .style(|_theme: &Theme| container::Style {
                background: Some(iced::Color::from_rgb(0.1, 0.1, 0.1).into()),
                ..Default::default()
            })
            .into()
    }

    fn view_input(&self, font_size: f32) -> Element<'static, ClaudePanelMessage> {
        let input_field = text_input("Ask Claude...", &self.input)
            .on_input(ClaudePanelMessage::InputChanged)
            .on_submit(ClaudePanelMessage::Submit)
            .padding(10)
            .size(font_size)
            .width(Length::Fill);

        let send_btn = button(text("Send").size(font_size))
            .on_press(ClaudePanelMessage::Submit)
            .padding(Padding::from([8, 15]));

        row![input_field, send_btn]
            .spacing(10)
            .align_y(Alignment::Center)
            .into()
    }
}

/// Background worker that owns the Claude session.
async fn claude_worker(
    cwd: PathBuf,
    mut cmd_rx: mpsc::UnboundedReceiver<ClaudeCommand>,
    event_tx: mpsc::UnboundedSender<ReaderMessage>,
) {
    tracing::info!("Claude worker starting, creating session...");

    // Create session
    let (mut session, mut session_rx) = match ClaudeSession::new(cwd).await {
        Ok((s, r)) => (s, r),
        Err(e) => {
            tracing::error!("Failed to create Claude session: {}", e);
            let _ = event_tx.send(ReaderMessage::Error(e.to_string()));
            return;
        }
    };

    tracing::info!("Claude session created, worker ready");

    loop {
        tokio::select! {
            // Handle commands from UI
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(ClaudeCommand::SendMessage(content)) => {
                        tracing::info!("Worker sending message: {}", content);
                        if let Err(e) = session.send_message(&content).await {
                            tracing::error!("Failed to send message: {}", e);
                            let _ = event_tx.send(ReaderMessage::Error(e.to_string()));
                        }
                    }
                    Some(ClaudeCommand::Close) | None => {
                        tracing::info!("Claude worker shutting down");
                        break;
                    }
                }
            }

            // Forward events from session to UI
            event = session_rx.recv() => {
                match event {
                    Some(msg) => {
                        if event_tx.send(msg).is_err() {
                            tracing::warn!("Event channel closed, shutting down worker");
                            break;
                        }
                    }
                    None => {
                        tracing::info!("Session channel closed");
                        let _ = event_tx.send(ReaderMessage::Closed);
                        break;
                    }
                }
            }
        }
    }
}
