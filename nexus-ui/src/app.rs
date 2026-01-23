//! Main Nexus application using Iced's Elm architecture.

use std::time::Instant;

use iced::keyboard::{self, Key, Modifiers};
use iced::widget::{column, container, row, scrollable, text, text_input, Column};
use iced::{event, Element, Event, Length, Subscription, Task, Theme};
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
    #[allow(dead_code)]
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

    fn is_running(&self) -> bool {
        matches!(self.state, BlockState::Running)
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
    /// Focus a specific block for input.
    FocusBlock(BlockId),
    /// Keyboard event when a block is focused.
    KeyPressed(Key, Modifiers),
    /// Tick for animations/updates.
    Tick,
    /// Generic event (for subscription).
    Event(Event),
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
    /// Currently focused block (receives keyboard input).
    focused_block: Option<BlockId>,
    /// Whether the input field is focused.
    input_focused: bool,
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
            focused_block: None,
            input_focused: true,
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
            state.input_focused = true;
            state.focused_block = None;
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
            state.pty_handles.retain(|h| h.block_id != block_id);

            // If the focused block exited, clear focus
            if state.focused_block == Some(block_id) {
                state.focused_block = None;
                state.input_focused = true;
            }
        }
        Message::ToggleBlock(block_id) => {
            if let Some(block) = state.blocks.iter_mut().find(|b| b.id == block_id) {
                block.collapsed = !block.collapsed;
            }
        }
        Message::FocusBlock(block_id) => {
            // Only allow focusing running blocks
            if state.blocks.iter().any(|b| b.id == block_id && b.is_running()) {
                state.focused_block = Some(block_id);
                state.input_focused = false;
            }
        }
        Message::KeyPressed(key, modifiers) => {
            // Handle keyboard input for focused block
            if let Some(block_id) = state.focused_block {
                if let Some(handle) = state.pty_handles.iter().find(|h| h.block_id == block_id) {
                    // Handle Ctrl+C
                    if modifiers.control() {
                        match &key {
                            Key::Character(c) if c.as_str() == "c" => {
                                let _ = handle.send_interrupt();
                                return Task::none();
                            }
                            Key::Character(c) if c.as_str() == "d" => {
                                let _ = handle.send_eof();
                                return Task::none();
                            }
                            Key::Character(c) if c.as_str() == "z" => {
                                let _ = handle.send_suspend();
                                return Task::none();
                            }
                            _ => {}
                        }
                    }

                    // Convert key to bytes and send to PTY
                    if let Some(bytes) = key_to_bytes(&key, &modifiers) {
                        let _ = handle.write(&bytes);
                    }
                }
            } else if !state.input_focused {
                // Escape returns focus to input
                if matches!(key, Key::Named(keyboard::key::Named::Escape)) {
                    state.input_focused = true;
                }
            }
        }
        Message::Event(event) => {
            // Handle keyboard events
            if let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event {
                // Only process if we have a focused block or need to handle escape
                if state.focused_block.is_some() || !state.input_focused {
                    return update(state, Message::KeyPressed(key, modifiers));
                }
            }
        }
        Message::Tick => {
            // Drain all pending PTY events from the channel
            while let Ok((block_id, event)) = state.pty_rx.try_recv() {
                match event {
                    PtyEvent::Output(data) => {
                        if let Some(block) = state.blocks.iter_mut().find(|b| b.id == block_id) {
                            block.parser.feed(&data);
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
                        state.pty_handles.retain(|h| h.block_id != block_id);

                        if state.focused_block == Some(block_id) {
                            state.focused_block = None;
                            state.input_focused = true;
                        }
                    }
                }
            }
        }
    }
    Task::none()
}

/// Convert a keyboard key to bytes to send to the PTY.
fn key_to_bytes(key: &Key, modifiers: &Modifiers) -> Option<Vec<u8>> {
    match key {
        Key::Character(c) => {
            let s = c.as_str();
            if modifiers.control() && s.len() == 1 {
                // Ctrl+letter = ASCII 1-26
                let ch = s.chars().next()?;
                if ch.is_ascii_alphabetic() {
                    let ctrl_code = (ch.to_ascii_lowercase() as u8) - b'a' + 1;
                    return Some(vec![ctrl_code]);
                }
            }
            Some(s.as_bytes().to_vec())
        }
        Key::Named(named) => {
            use keyboard::key::Named;
            match named {
                Named::Enter => Some(vec![b'\r']),
                Named::Backspace => Some(vec![0x7f]),
                Named::Tab => Some(vec![b'\t']),
                Named::Escape => Some(vec![0x1b]),
                Named::Space => Some(vec![b' ']),
                // Arrow keys (ANSI escape sequences)
                Named::ArrowUp => Some(vec![0x1b, b'[', b'A']),
                Named::ArrowDown => Some(vec![0x1b, b'[', b'B']),
                Named::ArrowRight => Some(vec![0x1b, b'[', b'C']),
                Named::ArrowLeft => Some(vec![0x1b, b'[', b'D']),
                Named::Home => Some(vec![0x1b, b'[', b'H']),
                Named::End => Some(vec![0x1b, b'[', b'F']),
                Named::PageUp => Some(vec![0x1b, b'[', b'5', b'~']),
                Named::PageDown => Some(vec![0x1b, b'[', b'6', b'~']),
                Named::Insert => Some(vec![0x1b, b'[', b'2', b'~']),
                Named::Delete => Some(vec![0x1b, b'[', b'3', b'~']),
                _ => None,
            }
        }
        _ => None,
    }
}

fn view(state: &Nexus) -> Element<'_, Message> {
    // Build all blocks as a continuous terminal output
    let mut content_elements: Vec<Element<Message>> = Vec::new();

    for block in &state.blocks {
        content_elements.push(view_block(state, block));
    }

    // Scrollable area for command history
    let history = scrollable(
        Column::with_children(content_elements)
            .spacing(4)
            .padding([10, 15])
    )
    .height(Length::Fill);

    // Input line always visible at bottom
    let input_line = container(view_input(state))
        .padding([8, 15])
        .width(Length::Fill);

    let content = column![history, input_line].spacing(0);

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(0.07, 0.07, 0.09))),
            ..Default::default()
        })
        .into()
}

fn subscription(state: &Nexus) -> Subscription<Message> {
    let tick = iced::time::every(std::time::Duration::from_millis(16))
        .map(|_| Message::Tick);

    // Only subscribe to keyboard events if we have a focused block
    if state.focused_block.is_some() {
        Subscription::batch([
            tick,
            event::listen().map(Message::Event),
        ])
    } else {
        tick
    }
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

    // Auto-focus the new block for interactive commands
    state.focused_block = Some(block_id);
    state.input_focused = false;

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
            state.focused_block = None;
            state.input_focused = true;
        }
    }

    Task::none()
}

fn view_block<'a>(_state: &'a Nexus, block: &'a Block) -> Element<'a, Message> {
    let prompt_color = iced::Color::from_rgb(0.3, 0.8, 0.5);
    let command_color = iced::Color::from_rgb(0.9, 0.9, 0.9);

    let prompt_line = row![
        text("$ ").size(14).color(prompt_color).font(iced::Font::MONOSPACE),
        text(&block.command).size(14).color(command_color).font(iced::Font::MONOSPACE),
    ]
    .spacing(0);

    // Terminal output - only show cursor for running commands
    let output: Element<Message> = if block.collapsed {
        column![].into()
    } else {
        TerminalView::new(block.parser.grid())
            .show_cursor(block.is_running())
            .into()
    };

    column![prompt_line, output]
        .spacing(0)
        .into()
}

fn view_input(state: &Nexus) -> Element<'_, Message> {
    let prompt_color = iced::Color::from_rgb(0.3, 0.8, 0.5);

    let prompt = text("$ ")
        .size(14)
        .color(prompt_color)
        .font(iced::Font::MONOSPACE);

    let input = text_input("", &state.input)
        .on_input(Message::InputChanged)
        .on_submit(Message::Submit)
        .padding(0)
        .size(14)
        .style(|_theme, _status| text_input::Style {
            background: iced::Background::Color(iced::Color::TRANSPARENT),
            border: iced::Border {
                width: 0.0,
                ..Default::default()
            },
            icon: iced::Color::from_rgb(0.5, 0.5, 0.5),
            placeholder: iced::Color::from_rgb(0.4, 0.4, 0.4),
            value: iced::Color::from_rgb(0.9, 0.9, 0.9),
            selection: iced::Color::from_rgb(0.3, 0.5, 0.8),
        })
        .font(iced::Font::MONOSPACE);

    row![prompt, input]
        .spacing(0)
        .align_y(iced::Alignment::Center)
        .into()
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}
