//! Main Nexus application using Iced's Elm architecture.

use std::time::Instant;

use iced::widget::{column, container, row, scrollable, text, text_input, Column};
use iced::{Element, Length, Subscription, Task, Theme};
use tokio::sync::mpsc;

use nexus_api::{BlockId, BlockState, OutputFormat};
use nexus_term::TerminalParser;

use crate::pty::PtyHandle;
use crate::widgets::terminal_view::TerminalView;

/// A command block containing input and output.
#[derive(Debug)]
pub struct Block {
    pub id: BlockId,
    pub command: String,
    pub parser: TerminalParser,
    pub state: BlockState,
    pub format: OutputFormat,
    pub collapsed: bool,
    pub started_at: Instant,
    pub duration_ms: Option<u64>,
}

impl Block {
    fn new(id: BlockId, command: String) -> Self {
        Self {
            id,
            command,
            parser: TerminalParser::new(120, 24),
            state: BlockState::Running,
            format: OutputFormat::PlainText,
            collapsed: false,
            started_at: Instant::now(),
            duration_ms: None,
        }
    }
}

/// Messages for the Nexus application.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum Message {
    /// Input text changed.
    InputChanged(String),
    /// User submitted a command.
    Submit,
    /// PTY output received.
    PtyOutput(BlockId, Vec<u8>),
    /// PTY exited.
    PtyExited(BlockId, i32),
    /// Toggle block collapsed state.
    ToggleBlock(BlockId),
    /// Window resized.
    WindowResized(u32, u32),
    /// Tick for animations/updates.
    Tick,
}

/// The main Nexus application state.
pub struct Nexus {
    /// Current input text.
    input: String,
    /// Command blocks.
    blocks: Vec<Block>,
    /// Next block ID.
    next_block_id: u64,
    /// Current working directory.
    cwd: String,
    /// Active PTY handles.
    pty_handles: Vec<PtyHandle>,
    /// Channel for PTY output.
    pty_tx: mpsc::UnboundedSender<(BlockId, PtyEvent)>,
    /// Receiver for PTY events.
    pty_rx: mpsc::UnboundedReceiver<(BlockId, PtyEvent)>,
}

#[derive(Debug, Clone)]
pub enum PtyEvent {
    Output(Vec<u8>),
    Exited(i32),
}

impl Default for Nexus {
    fn default() -> Self {
        let (pty_tx, pty_rx) = mpsc::unbounded_channel();
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| "~".to_string());

        Self {
            input: String::new(),
            blocks: Vec::new(),
            next_block_id: 1,
            cwd,
            pty_handles: Vec::new(),
            pty_tx,
            pty_rx,
        }
    }
}

/// Run the Nexus application.
pub fn run() -> iced::Result {
    iced::application("Nexus", update, view)
        .subscription(subscription)
        .theme(|_| Theme::Dark)
        .window_size(iced::Size::new(1200.0, 800.0))
        .antialiasing(true)
        .run()
}

fn update(state: &mut Nexus, message: Message) -> Task<Message> {
    match message {
        Message::InputChanged(value) => {
            state.input = value;
        }
        Message::Submit => {
            if !state.input.trim().is_empty() {
                let command = state.input.clone();
                state.input.clear();
                return execute_command(state, command);
            }
        }
        Message::PtyOutput(block_id, data) => {
            if let Some(block) = state.blocks.iter_mut().find(|b| b.id == block_id) {
                block.parser.feed(&data);
                // Detect format from output
                let grid_text = block.parser.grid().to_string();
                if block.format == OutputFormat::PlainText && grid_text.trim_start().starts_with('{') {
                    block.format = OutputFormat::Json;
                }
            }
        }
        Message::PtyExited(block_id, exit_code) => {
            if let Some(block) = state.blocks.iter_mut().find(|b| b.id == block_id) {
                block.state = if exit_code == 0 {
                    BlockState::Success
                } else {
                    BlockState::Failed(exit_code)
                };
                block.duration_ms = Some(block.started_at.elapsed().as_millis() as u64);
            }
            // Remove the PTY handle
            state.pty_handles.retain(|h| h.block_id != block_id);
        }
        Message::ToggleBlock(block_id) => {
            if let Some(block) = state.blocks.iter_mut().find(|b| b.id == block_id) {
                block.collapsed = !block.collapsed;
            }
        }
        Message::WindowResized(_width, _height) => {
            // Could resize terminal parsers here
        }
        Message::Tick => {
            // Drain all pending PTY events from the channel
            while let Ok((block_id, event)) = state.pty_rx.try_recv() {
                match event {
                    PtyEvent::Output(data) => {
                        if let Some(block) = state.blocks.iter_mut().find(|b| b.id == block_id) {
                            block.parser.feed(&data);
                            // Detect format from output
                            let grid_text = block.parser.grid().to_string();
                            if block.format == OutputFormat::PlainText && grid_text.trim_start().starts_with('{') {
                                block.format = OutputFormat::Json;
                            }
                        }
                    }
                    PtyEvent::Exited(exit_code) => {
                        if let Some(block) = state.blocks.iter_mut().find(|b| b.id == block_id) {
                            block.state = if exit_code == 0 {
                                BlockState::Success
                            } else {
                                BlockState::Failed(exit_code)
                            };
                            block.duration_ms = Some(block.started_at.elapsed().as_millis() as u64);
                        }
                        // Remove the PTY handle
                        state.pty_handles.retain(|h| h.block_id != block_id);
                    }
                }
            }
        }
    }
    Task::none()
}

fn view(state: &Nexus) -> Element<'_, Message> {
    let blocks_view: Element<Message> = if state.blocks.is_empty() {
        container(
            text("Welcome to Nexus. Type a command to get started.")
                .size(16)
        )
        .padding(20)
        .into()
    } else {
        let blocks: Vec<Element<Message>> = state
            .blocks
            .iter()
            .map(|block| view_block(state, block))
            .collect();

        scrollable(
            Column::with_children(blocks)
                .spacing(8)
                .padding(10)
        )
        .height(Length::Fill)
        .into()
    };

    let input_line = view_input(state);

    let content = column![
        blocks_view,
        input_line,
    ]
    .spacing(0);

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(container::dark)
        .into()
}

fn subscription(_state: &Nexus) -> Subscription<Message> {
    // Poll at 60fps to check for PTY events
    iced::time::every(std::time::Duration::from_millis(16))
        .map(|_| Message::Tick)
}

fn execute_command(state: &mut Nexus, command: String) -> Task<Message> {
    let block_id = BlockId(state.next_block_id);
    state.next_block_id += 1;

    // Handle built-in commands
    if command.trim() == "clear" {
        state.blocks.clear();
        return Task::none();
    }

    if command.trim().starts_with("cd ") {
        let path = command.trim().strip_prefix("cd ").unwrap().trim();
        let new_path = if path.starts_with('/') {
            std::path::PathBuf::from(path)
        } else if path == "~" {
            home_dir().unwrap_or_else(|| std::path::PathBuf::from("/"))
        } else {
            std::path::PathBuf::from(&state.cwd).join(path)
        };

        if let Ok(canonical) = new_path.canonicalize() {
            if canonical.is_dir() {
                state.cwd = canonical.display().to_string();
                let _ = std::env::set_current_dir(&canonical);
            }
        }
        return Task::none();
    }

    // Create new block
    let block = Block::new(block_id, command.clone());
    state.blocks.push(block);

    // Spawn PTY
    let tx = state.pty_tx.clone();
    let cwd = state.cwd.clone();

    match PtyHandle::spawn(&command, &cwd, block_id, tx) {
        Ok(handle) => {
            state.pty_handles.push(handle);
        }
        Err(e) => {
            tracing::error!("Failed to spawn PTY: {}", e);
            if let Some(block) = state.blocks.iter_mut().find(|b| b.id == block_id) {
                block.state = BlockState::Failed(1);
                block.parser.feed(format!("Error: {}\n", e).as_bytes());
            }
        }
    }

    Task::none()
}

fn view_block<'a>(_state: &'a Nexus, block: &'a Block) -> Element<'a, Message> {
    let status_color = match block.state {
        BlockState::Running => iced::Color::from_rgb(0.2, 0.6, 1.0),
        BlockState::Success => iced::Color::from_rgb(0.3, 0.8, 0.5),
        BlockState::Failed(_) => iced::Color::from_rgb(0.9, 0.3, 0.3),
        BlockState::Killed(_) => iced::Color::from_rgb(0.8, 0.6, 0.2),
    };

    let status_icon = match block.state {
        BlockState::Running => "⟳",
        BlockState::Success => "✓",
        BlockState::Failed(_) => "✗",
        BlockState::Killed(_) => "⚡",
    };

    let duration_text = block
        .duration_ms
        .map(|ms| format!(" ({}ms)", ms))
        .unwrap_or_default();

    let header = iced::widget::button(
        row![
            text(status_icon).color(status_color),
            text(" ").size(14),
            text(&block.command).size(14),
            text(duration_text).size(12),
        ]
        .spacing(4)
        .align_y(iced::Alignment::Center)
    )
    .style(iced::widget::button::text)
    .on_press(Message::ToggleBlock(block.id))
    .padding(8);

    let content: Element<Message> = if block.collapsed {
        text("").into()
    } else {
        TerminalView::new(block.parser.grid())
            .into()
    };

    container(
        column![header, content].spacing(0)
    )
    .style(|_theme| container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgb(0.12, 0.12, 0.14))),
        border: iced::Border {
            color: iced::Color::from_rgb(0.2, 0.2, 0.22),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..Default::default()
    })
    .width(Length::Fill)
    .into()
}

fn view_input(state: &Nexus) -> Element<'_, Message> {
    let prompt = text(format!("{} > ", state.cwd))
        .size(14)
        .color(iced::Color::from_rgb(0.3, 0.7, 1.0));

    let input = text_input("Type a command...", &state.input)
        .on_input(Message::InputChanged)
        .on_submit(Message::Submit)
        .padding(10)
        .size(14);

    container(
        row![prompt, input]
            .spacing(8)
            .align_y(iced::Alignment::Center)
    )
    .padding(10)
    .style(|_theme| container::Style {
        background: Some(iced::Background::Color(iced::Color::from_rgb(0.08, 0.08, 0.1))),
        border: iced::Border {
            color: iced::Color::from_rgb(0.2, 0.2, 0.22),
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .width(Length::Fill)
    .into()
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}
