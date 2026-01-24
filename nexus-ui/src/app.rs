//! Main Nexus application using Iced's Elm architecture.

use std::sync::Arc;
use std::time::Instant;

use iced::futures::stream;
use iced::keyboard::{self, Key, Modifiers};
use iced::widget::{column, container, row, scrollable, text, text_input, Column};
use iced::{event, Element, Event, Length, Subscription, Task, Theme};
use tokio::sync::{mpsc, Mutex};

use nexus_api::{BlockId, BlockState, OutputFormat};
use nexus_term::TerminalParser;

use crate::pty::PtyHandle;
use crate::widgets::terminal_view::TerminalView;

/// Scrollable ID for auto-scrolling.
const HISTORY_SCROLLABLE: &str = "history";

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
    /// Version counter for lazy invalidation.
    pub version: u64,
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
            version: 0,
        }
    }

    fn is_running(&self) -> bool {
        matches!(self.state, BlockState::Running)
    }
}

/// Focus state - makes illegal states unrepresentable.
#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    /// The command input field is focused.
    Input,
    /// A specific block is focused for interaction.
    Block(BlockId),
}

/// Messages for the Nexus application.
#[derive(Debug, Clone)]
pub enum Message {
    /// Input text changed.
    InputChanged(String),
    /// User submitted a command.
    Submit,
    /// PTY output received.
    PtyOutput(BlockId, Vec<u8>),
    /// PTY exited.
    PtyExited(BlockId, i32),
    /// Keyboard event when a block is focused.
    KeyPressed(Key, Modifiers),
    /// Generic event (for subscription).
    Event(Event),
    /// Window resized.
    WindowResized(u32, u32),
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
    /// Receiver for PTY events (shared with subscription).
    pty_rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
    /// Current focus state.
    focus: Focus,
    /// Terminal dimensions (cols, rows).
    terminal_size: (u16, u16),
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
            pty_rx: Arc::new(Mutex::new(pty_rx)),
            focus: Focus::Input,
            terminal_size: (120, 24),
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
            state.focus = Focus::Input;
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
                block.version += 1; // Invalidate lazy cache
            }
            // Auto-scroll to bottom
            return scrollable::snap_to(
                scrollable::Id::new(HISTORY_SCROLLABLE),
                scrollable::RelativeOffset::END,
            );
        }
        Message::PtyExited(block_id, exit_code) => {
            if let Some(block) = state.blocks.iter_mut().find(|b| b.id == block_id) {
                block.state = if exit_code == 0 {
                    BlockState::Success
                } else {
                    BlockState::Failed(exit_code)
                };
                block.duration_ms = Some(block.started_at.elapsed().as_millis() as u64);
                block.version += 1;
            }
            state.pty_handles.retain(|h| h.block_id != block_id);

            // If the focused block exited, return focus to input
            if state.focus == Focus::Block(block_id) {
                state.focus = Focus::Input;
            }
        }
        Message::KeyPressed(key, modifiers) => {
            if let Focus::Block(block_id) = state.focus {
                if let Some(handle) = state.pty_handles.iter().find(|h| h.block_id == block_id) {
                    // Handle Ctrl+C/D/Z
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

                    // Escape returns focus to input
                    if matches!(key, Key::Named(keyboard::key::Named::Escape)) {
                        state.focus = Focus::Input;
                        return Task::none();
                    }

                    // Convert key to bytes and send to PTY
                    if let Some(bytes) = key_to_bytes(&key, &modifiers) {
                        let _ = handle.write(&bytes);
                    }
                }
            }
        }
        Message::Event(event) => {
            match event {
                Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    if let Focus::Block(_) = state.focus {
                        return update(state, Message::KeyPressed(key, modifiers));
                    }
                }
                Event::Window(iced::window::Event::Resized(size)) => {
                    return update(state, Message::WindowResized(size.width as u32, size.height as u32));
                }
                _ => {}
            }
        }
        Message::WindowResized(width, height) => {
            // Calculate terminal dimensions immediately (no debounce)
            // Use 8.5 instead of 8.4 to be conservative - prevents overestimating columns
            let char_width = 8.5_f32;
            let line_height = 19.6_f32; // 14px * 1.4 line height
            let h_padding = 30.0; // Left + right padding
            let v_padding = 80.0; // Top/bottom padding + input area

            let cols = ((width as f32 - h_padding) / char_width) as u16;
            let rows = ((height as f32 - v_padding) / line_height) as u16;

            // Clamp to reasonable ranges
            let cols = cols.max(40).min(500);
            let rows = rows.max(5).min(200);

            let old_cols = state.terminal_size.0;
            state.terminal_size = (cols, rows);

            // Only resize if column count changed
            if cols != old_cols {
                for block in &mut state.blocks {
                    if block.is_running() {
                        // Running blocks: resize to window dimensions for TUIs
                        block.parser.resize(cols, rows);
                    } else {
                        // Finished blocks: two-step resize
                        // 1. First resize width only (triggers reflow)
                        let (_, current_rows) = block.parser.size();
                        block.parser.resize(cols, current_rows);

                        // 2. Calculate the actual height needed for wrapped text
                        let needed_rows = block.parser.content_height() as u16;

                        // 3. Resize parser to exact content height
                        block.parser.resize(cols, needed_rows.max(1));
                    }
                }

                // Resize all active PTYs
                for handle in &state.pty_handles {
                    let _ = handle.resize(cols, rows);
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

            // Handle modifier combinations for arrow keys
            if modifiers.control() {
                match named {
                    // Ctrl+Arrow for word navigation
                    Named::ArrowLeft => return Some(vec![0x1b, b'[', b'1', b';', b'5', b'D']),
                    Named::ArrowRight => return Some(vec![0x1b, b'[', b'1', b';', b'5', b'C']),
                    Named::ArrowUp => return Some(vec![0x1b, b'[', b'1', b';', b'5', b'A']),
                    Named::ArrowDown => return Some(vec![0x1b, b'[', b'1', b';', b'5', b'B']),
                    _ => {}
                }
            }

            if modifiers.shift() {
                match named {
                    Named::ArrowLeft => return Some(vec![0x1b, b'[', b'1', b';', b'2', b'D']),
                    Named::ArrowRight => return Some(vec![0x1b, b'[', b'1', b';', b'2', b'C']),
                    Named::ArrowUp => return Some(vec![0x1b, b'[', b'1', b';', b'2', b'A']),
                    Named::ArrowDown => return Some(vec![0x1b, b'[', b'1', b';', b'2', b'B']),
                    _ => {}
                }
            }

            if modifiers.alt() {
                match named {
                    Named::ArrowLeft => return Some(vec![0x1b, b'[', b'1', b';', b'3', b'D']),
                    Named::ArrowRight => return Some(vec![0x1b, b'[', b'1', b';', b'3', b'C']),
                    Named::ArrowUp => return Some(vec![0x1b, b'[', b'1', b';', b'3', b'A']),
                    Named::ArrowDown => return Some(vec![0x1b, b'[', b'1', b';', b'3', b'B']),
                    _ => {}
                }
            }

            match named {
                Named::Enter => Some(vec![b'\r']),
                Named::Backspace => Some(vec![0x7f]),
                Named::Tab => Some(vec![b'\t']),
                Named::Escape => Some(vec![0x1b]),
                Named::Space => Some(vec![b' ']),
                // Arrow keys
                Named::ArrowUp => Some(vec![0x1b, b'[', b'A']),
                Named::ArrowDown => Some(vec![0x1b, b'[', b'B']),
                Named::ArrowRight => Some(vec![0x1b, b'[', b'C']),
                Named::ArrowLeft => Some(vec![0x1b, b'[', b'D']),
                // Navigation
                Named::Home => Some(vec![0x1b, b'[', b'H']),
                Named::End => Some(vec![0x1b, b'[', b'F']),
                Named::PageUp => Some(vec![0x1b, b'[', b'5', b'~']),
                Named::PageDown => Some(vec![0x1b, b'[', b'6', b'~']),
                Named::Insert => Some(vec![0x1b, b'[', b'2', b'~']),
                Named::Delete => Some(vec![0x1b, b'[', b'3', b'~']),
                // Function keys
                Named::F1 => Some(vec![0x1b, b'O', b'P']),
                Named::F2 => Some(vec![0x1b, b'O', b'Q']),
                Named::F3 => Some(vec![0x1b, b'O', b'R']),
                Named::F4 => Some(vec![0x1b, b'O', b'S']),
                Named::F5 => Some(vec![0x1b, b'[', b'1', b'5', b'~']),
                Named::F6 => Some(vec![0x1b, b'[', b'1', b'7', b'~']),
                Named::F7 => Some(vec![0x1b, b'[', b'1', b'8', b'~']),
                Named::F8 => Some(vec![0x1b, b'[', b'1', b'9', b'~']),
                Named::F9 => Some(vec![0x1b, b'[', b'2', b'0', b'~']),
                Named::F10 => Some(vec![0x1b, b'[', b'2', b'1', b'~']),
                Named::F11 => Some(vec![0x1b, b'[', b'2', b'3', b'~']),
                Named::F12 => Some(vec![0x1b, b'[', b'2', b'4', b'~']),
                _ => None,
            }
        }
        _ => None,
    }
}

fn view(state: &Nexus) -> Element<'_, Message> {
    // Build all blocks
    let content_elements: Vec<Element<Message>> = state
        .blocks
        .iter()
        .map(|block| view_block(block))
        .collect();

    // Scrollable area for command history with ID for auto-scroll
    let history = scrollable(
        Column::with_children(content_elements)
            .spacing(4)
            .padding([10, 15]),
    )
    .id(scrollable::Id::new(HISTORY_SCROLLABLE))
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
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.07, 0.07, 0.09,
            ))),
            ..Default::default()
        })
        .into()
}

fn subscription(state: &Nexus) -> Subscription<Message> {
    let mut subscriptions = vec![pty_subscription(state.pty_rx.clone())];

    // Subscribe to keyboard and window events when a block is focused
    if matches!(state.focus, Focus::Block(_)) {
        subscriptions.push(event::listen().map(Message::Event));
    } else {
        // Still listen for window resize events
        subscriptions.push(
            event::listen_with(|event, _status, _id| {
                if let Event::Window(iced::window::Event::Resized(_)) = event {
                    Some(Message::Event(event))
                } else {
                    None
                }
            })
        );
    }

    Subscription::batch(subscriptions)
}

/// Async subscription that awaits PTY events instead of polling.
fn pty_subscription(
    rx: Arc<Mutex<mpsc::UnboundedReceiver<(BlockId, PtyEvent)>>>,
) -> Subscription<Message> {
    struct PtySubscription;

    Subscription::run_with_id(
        std::any::TypeId::of::<PtySubscription>(),
        stream::unfold(rx, |rx| async move {
            let event = {
                let mut guard = rx.lock().await;
                guard.recv().await
            };

            match event {
                Some((block_id, PtyEvent::Output(data))) => {
                    Some((Message::PtyOutput(block_id, data), rx))
                }
                Some((block_id, PtyEvent::Exited(code))) => {
                    Some((Message::PtyExited(block_id, code), rx))
                }
                None => None,
            }
        }),
    )
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

    // Create new block with current terminal size
    let mut block = Block::new(block_id, command.clone());
    block.parser = TerminalParser::new(state.terminal_size.0, state.terminal_size.1);
    state.blocks.push(block);

    // Auto-focus the new block for interactive commands
    state.focus = Focus::Block(block_id);

    // Spawn PTY with current terminal size
    let tx = state.pty_tx.clone();
    let cwd = state.cwd.clone();
    let (cols, rows) = state.terminal_size;

    match PtyHandle::spawn_with_size(&command, &cwd, block_id, tx, cols, rows) {
        Ok(handle) => {
            state.pty_handles.push(handle);
        }
        Err(e) => {
            tracing::error!("Failed to spawn PTY: {}", e);
            if let Some(block) = state.blocks.iter_mut().find(|b| b.id == block_id) {
                block.state = BlockState::Failed(1);
                block.parser.feed(format!("Error: {}\n", e).as_bytes());
                block.version += 1;
            }
            state.focus = Focus::Input;
        }
    }

    // Scroll to bottom to show new command
    scrollable::snap_to(
        scrollable::Id::new(HISTORY_SCROLLABLE),
        scrollable::RelativeOffset::END,
    )
}

fn view_block(block: &Block) -> Element<'_, Message> {
    let prompt_color = iced::Color::from_rgb(0.3, 0.8, 0.5);
    let command_color = iced::Color::from_rgb(0.9, 0.9, 0.9);

    let prompt_line = row![
        text("$ ")
            .size(14)
            .color(prompt_color)
            .font(iced::Font::MONOSPACE),
        text(&block.command)
            .size(14)
            .color(command_color)
            .font(iced::Font::MONOSPACE),
    ]
    .spacing(0);

    // Terminal output - only show cursor for running commands
    // Use grid_with_scrollback for finished blocks to show all content including history
    let output: Element<Message> = if block.collapsed {
        column![].into()
    } else {
        let grid = if block.is_running() {
            // Running blocks: viewport only (live updates)
            block.parser.grid()
        } else {
            // Finished blocks: all content including scrollback
            block.parser.grid_with_scrollback()
        };
        TerminalView::new(grid)
            .show_cursor(block.is_running())
            .into()
    };

    column![prompt_line, output].spacing(0).into()
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
