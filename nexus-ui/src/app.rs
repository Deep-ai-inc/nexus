//! Main Nexus application using Iced's Elm architecture.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use iced::futures::stream;
use iced::keyboard::{self, Key, Modifiers};
use iced::widget::{column, container, row, scrollable, text, text_input, Column};
use iced::{event, Element, Event, Length, Subscription, Task, Theme};
use tokio::sync::{mpsc, Mutex};

use nexus_api::{BlockId, BlockState, OutputFormat};
use nexus_term::TerminalParser;

use crate::glyph_cache::get_cell_metrics;
use crate::pty::PtyHandle;
use crate::widgets::terminal_shader::TerminalShader;

// ============================================================================
// Terminal rendering constants - single source of truth
// ============================================================================

/// Default font size for terminal text.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;
/// Line height multiplier.
pub const LINE_HEIGHT_FACTOR: f32 = 1.4;
/// Character width ratio relative to font size.
pub const CHAR_WIDTH_RATIO: f32 = 0.607; // ~8.5/14.0, conservative for anti-aliasing

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

impl PartialEq for Block {
    fn eq(&self, other: &Self) -> bool {
        // Different blocks are never equal
        if self.id != other.id {
            return false;
        }

        // Running blocks always need redrawing (cursor, new output)
        if self.is_running() {
            return false;
        }

        // Finished blocks: check if anything visual changed
        self.version == other.version
            && self.collapsed == other.collapsed
            && self.parser.size() == other.parser.size()
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
    /// Generic event (for subscription) with window ID.
    Event(Event, iced::window::Id),
    /// Window resized.
    WindowResized(u32, u32),
    /// Global keyboard shortcut (Cmd+K, Cmd+Q, etc.)
    GlobalShortcut(GlobalShortcut),
    /// Input-specific key event (Up/Down for history)
    InputKey(Key, Modifiers),
    /// Zoom font size
    Zoom(ZoomDirection),
    /// VSync-aligned frame for batched rendering.
    /// Fires when the monitor is ready for the next frame.
    NextFrame(Instant),
}

/// Global keyboard shortcuts.
#[derive(Debug, Clone)]
pub enum GlobalShortcut {
    /// Cmd+K - Clear screen
    ClearScreen,
    /// Cmd+W - Close window
    CloseWindow,
    /// Cmd+Q - Quit application
    Quit,
    /// Cmd+C - Copy
    Copy,
    /// Cmd+V - Paste
    Paste,
}

/// Zoom direction for font size changes.
#[derive(Debug, Clone)]
pub enum ZoomDirection {
    /// Cmd++ - Increase font size
    In,
    /// Cmd+- - Decrease font size
    Out,
    /// Cmd+0 - Reset to default
    Reset,
}

/// The main Nexus application state.
pub struct Nexus {
    /// Current input text.
    input: String,
    /// Command blocks (ordered).
    blocks: Vec<Block>,
    /// Block index by ID for O(1) lookup.
    block_index: HashMap<BlockId, usize>,
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
    /// Window dimensions in pixels (for recalculating on zoom).
    window_dims: (f32, f32),
    /// Current font size (mutable for zoom).
    font_size: f32,
    /// Command history for Up/Down navigation.
    command_history: Vec<String>,
    /// Current position in history (None = new command being typed).
    history_index: Option<usize>,
    /// Saved input when browsing history (to restore on Down).
    saved_input: String,
    /// Exit code of last command (for prompt color).
    last_exit_code: Option<i32>,
    /// Window ID for resize operations.
    window_id: Option<iced::window::Id>,
    /// Suppress next input change (after Cmd shortcut to prevent typing).
    suppress_next_input: bool,
    /// Input value before current event (to detect ghost characters).
    input_before_event: String,
    /// Is there processed PTY data that hasn't been drawn yet?
    /// Used for 60 FPS throttling during high-throughput output.
    is_dirty: bool,
}

#[derive(Debug, Clone)]
pub enum PtyEvent {
    Output(Vec<u8>),
    Exited(i32),
}

impl Nexus {
    /// Get current character width based on font size.
    fn char_width(&self) -> f32 {
        self.font_size * CHAR_WIDTH_RATIO
    }

    /// Get current line height based on font size.
    fn line_height(&self) -> f32 {
        self.font_size * LINE_HEIGHT_FACTOR
    }

    /// Recalculate terminal dimensions based on window size and font metrics.
    /// Returns (cols, rows) and updates terminal_size.
    fn recalculate_terminal_size(&mut self) -> (u16, u16) {
        let h_padding = 30.0; // Left + right padding
        let v_padding = 80.0; // Top/bottom padding + input area

        let (width, height) = self.window_dims;
        let cols = ((width - h_padding) / self.char_width()) as u16;
        let rows = ((height - v_padding) / self.line_height()) as u16;

        // Clamp to reasonable ranges
        let cols = cols.max(40).min(500);
        let rows = rows.max(5).min(200);

        self.terminal_size = (cols, rows);
        (cols, rows)
    }

    /// Apply terminal resize to all blocks and PTYs.
    fn apply_resize(&mut self, cols: u16, rows: u16) {
        for block in &mut self.blocks {
            if block.is_running() {
                block.parser.resize(cols, rows);
            } else {
                // Finished blocks: two-step resize for reflow
                let (_, current_rows) = block.parser.size();
                block.parser.resize(cols, current_rows);
                let needed_rows = block.parser.content_height() as u16;
                block.parser.resize(cols, needed_rows.max(1));
            }
        }

        // Resize all active PTYs
        for handle in &self.pty_handles {
            let _ = handle.resize(cols, rows);
        }
    }
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
            block_index: HashMap::new(),
            next_block_id: 1,
            cwd,
            pty_handles: Vec::new(),
            pty_tx,
            pty_rx: Arc::new(Mutex::new(pty_rx)),
            focus: Focus::Input,
            terminal_size: (120, 24),
            window_dims: (1200.0, 800.0), // Match initial window size
            font_size: DEFAULT_FONT_SIZE,
            command_history: Vec::new(),
            history_index: None,
            saved_input: String::new(),
            last_exit_code: None,
            window_id: None,
            suppress_next_input: false,
            input_before_event: String::new(),
            is_dirty: false,
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
            // Suppress input if a Cmd shortcut just fired (prevents typing shortcut char)
            if state.suppress_next_input {
                state.suppress_next_input = false;
                return Task::none();
            }
            // Track previous value for ghost character detection
            state.input_before_event = state.input.clone();
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
            // O(1) lookup via HashMap
            if let Some(&idx) = state.block_index.get(&block_id) {
                if let Some(block) = state.blocks.get_mut(idx) {
                    block.parser.feed(&data);
                    block.version += 1; // Invalidate lazy cache
                }
            }

            // VSYNC-BATCHED THROTTLING:
            // For small updates (typing echo), draw immediately for 0ms latency.
            // For large updates (streaming output), batch via dirty flag and
            // let NextFrame handle it (VSync-aligned for smooth rendering).
            if data.len() < 128 {
                // Small data = likely typing echo, draw immediately
                state.is_dirty = false; // We're handling it now
                return scrollable::snap_to(
                    scrollable::Id::new(HISTORY_SCROLLABLE),
                    scrollable::RelativeOffset::END,
                );
            } else {
                // Large data = streaming, mark dirty for VSync batching
                state.is_dirty = true;
                return Task::none();
            }
        }
        Message::PtyExited(block_id, exit_code) => {
            // O(1) lookup via HashMap
            if let Some(&idx) = state.block_index.get(&block_id) {
                if let Some(block) = state.blocks.get_mut(idx) {
                    block.state = if exit_code == 0 {
                        BlockState::Success
                    } else {
                        BlockState::Failed(exit_code)
                    };
                    block.duration_ms = Some(block.started_at.elapsed().as_millis() as u64);
                    block.version += 1;
                }
            }
            state.pty_handles.retain(|h| h.block_id != block_id);

            // Track last exit code for prompt color
            state.last_exit_code = Some(exit_code);

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
                        if let Key::Character(c) = &key {
                            match c.to_lowercase().as_str() {
                                "c" => {
                                    let _ = handle.send_interrupt();
                                    return Task::none();
                                }
                                "d" => {
                                    let _ = handle.send_eof();
                                    return Task::none();
                                }
                                "z" => {
                                    let _ = handle.send_suspend();
                                    return Task::none();
                                }
                                _ => {}
                            }
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
        Message::Event(event, window_id) => {
            // Capture window ID for resize operations
            if state.window_id.is_none() {
                state.window_id = Some(window_id);
            }

            match event {
                Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                    // Global shortcuts (Cmd+K, Cmd+Q, etc.) work regardless of focus
                    if modifiers.command() {
                        if let Key::Character(c) = &key {
                            let ch = c.to_lowercase();
                            let task = match ch.as_str() {
                                "k" => Some(update(state, Message::GlobalShortcut(GlobalShortcut::ClearScreen))),
                                "w" => Some(update(state, Message::GlobalShortcut(GlobalShortcut::CloseWindow))),
                                "q" => Some(update(state, Message::GlobalShortcut(GlobalShortcut::Quit))),
                                "c" => Some(update(state, Message::GlobalShortcut(GlobalShortcut::Copy))),
                                "v" => Some(update(state, Message::GlobalShortcut(GlobalShortcut::Paste))),
                                "=" | "+" => Some(update(state, Message::Zoom(ZoomDirection::In))),
                                "-" => Some(update(state, Message::Zoom(ZoomDirection::Out))),
                                "0" => Some(update(state, Message::Zoom(ZoomDirection::Reset))),
                                _ => None,
                            };
                            if let Some(task) = task {
                                // Suppress next input to prevent shortcut char from being typed
                                state.suppress_next_input = true;
                                return task;
                            }
                        }
                    }

                    // Ctrl+C in input clears the line (like traditional shell)
                    if modifiers.control() && matches!(state.focus, Focus::Input) {
                        if let Key::Character(c) = &key {
                            if c.to_lowercase().as_str() == "c" {
                                state.input.clear();
                                state.history_index = None;
                                state.saved_input.clear();
                                return Task::none();
                            }
                        }
                    }

                    // Focus-dependent key handling
                    match state.focus {
                        Focus::Input => {
                            // Up/Down for history navigation
                            match &key {
                                Key::Named(keyboard::key::Named::ArrowUp) if !modifiers.shift() => {
                                    return update(state, Message::InputKey(key, modifiers));
                                }
                                Key::Named(keyboard::key::Named::ArrowDown) if !modifiers.shift() => {
                                    return update(state, Message::InputKey(key, modifiers));
                                }
                                _ => {}
                            }
                        }
                        Focus::Block(_) => {
                            // Forward key events to PTY
                            return update(state, Message::KeyPressed(key, modifiers));
                        }
                    }
                }
                Event::Window(iced::window::Event::Resized(size)) => {
                    return update(state, Message::WindowResized(size.width as u32, size.height as u32));
                }
                _ => {}
            }
        }
        Message::WindowResized(width, height) => {
            // Store window dimensions for zoom recalculation
            state.window_dims = (width as f32, height as f32);

            let old_cols = state.terminal_size.0;
            let (cols, rows) = state.recalculate_terminal_size();

            // Only resize if column count changed
            if cols != old_cols {
                state.apply_resize(cols, rows);
            }
        }
        Message::GlobalShortcut(shortcut) => {
            // Strip the shortcut character ONLY if text_input just typed it
            // (compare current input to input_before_event to avoid false positives)
            let strip_char = match &shortcut {
                GlobalShortcut::ClearScreen => Some('k'),
                GlobalShortcut::CloseWindow => Some('w'),
                GlobalShortcut::Quit => Some('q'),
                GlobalShortcut::Copy => Some('c'),
                GlobalShortcut::Paste => Some('v'),
            };
            if let Some(ch) = strip_char {
                // Only pop if input grew by exactly this character
                let expected_lower = format!("{}{}", state.input_before_event, ch);
                let expected_upper = format!("{}{}", state.input_before_event, ch.to_ascii_uppercase());
                if state.input == expected_lower || state.input == expected_upper {
                    state.input.pop();
                }
            }

            match shortcut {
                GlobalShortcut::ClearScreen => {
                    state.blocks.clear();
                    state.block_index.clear();
                }
                GlobalShortcut::CloseWindow | GlobalShortcut::Quit => {
                    return iced::exit();
                }
                GlobalShortcut::Copy => {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        // Copy the input (after stripping the 'c')
                        let _ = clipboard.set_text(&state.input);
                    }
                }
                GlobalShortcut::Paste => {
                    if let Ok(mut clipboard) = arboard::Clipboard::new() {
                        if let Ok(text) = clipboard.get_text() {
                            state.input.push_str(&text);
                        }
                    }
                }
            }
        }
        Message::Zoom(direction) => {
            // Strip the shortcut character ONLY if text_input just typed it
            let strip_chars: &[char] = match &direction {
                ZoomDirection::In => &['=', '+'],
                ZoomDirection::Out => &['-'],
                ZoomDirection::Reset => &['0'],
            };
            for &ch in strip_chars {
                let expected = format!("{}{}", state.input_before_event, ch);
                if state.input == expected {
                    state.input.pop();
                    break;
                }
            }

            let old_size = state.font_size;
            state.font_size = match direction {
                ZoomDirection::In => (state.font_size + 1.0).min(32.0),
                ZoomDirection::Out => (state.font_size - 1.0).max(8.0),
                ZoomDirection::Reset => DEFAULT_FONT_SIZE,
            };

            // Only resize if font size actually changed
            if (state.font_size - old_size).abs() > 0.001 {
                // Keep same terminal dimensions, resize window proportionally
                let (cols, rows) = state.terminal_size;
                let new_char_width = state.font_size * CHAR_WIDTH_RATIO;
                let new_line_height = state.font_size * LINE_HEIGHT_FACTOR;

                let h_padding = 30.0;
                let v_padding = 80.0;

                let new_width = (cols as f32 * new_char_width) + h_padding;
                let new_height = (rows as f32 * new_line_height) + v_padding;

                // Update cached window dims
                state.window_dims = (new_width, new_height);

                // Resize window if we have the ID
                if let Some(window_id) = state.window_id {
                    return iced::window::resize(
                        window_id,
                        iced::Size::new(new_width, new_height),
                    );
                }
            }
        }
        Message::NextFrame(_timestamp) => {
            // VSync-aligned frame - fires when monitor is ready for next frame
            // Only subscribed when is_dirty is true
            if state.is_dirty {
                state.is_dirty = false;
                // Request redraw by scrolling to bottom (this triggers view refresh)
                return scrollable::snap_to(
                    scrollable::Id::new(HISTORY_SCROLLABLE),
                    scrollable::RelativeOffset::END,
                );
            }
        }
        Message::InputKey(key, _modifiers) => {
            match &key {
                Key::Named(keyboard::key::Named::ArrowUp) => {
                    // Navigate to previous history entry
                    if state.command_history.is_empty() {
                        return Task::none();
                    }

                    match state.history_index {
                        None => {
                            // First press: save current input before browsing
                            state.saved_input = state.input.clone();
                            state.history_index = Some(state.command_history.len() - 1);
                        }
                        Some(0) => {
                            // Already at oldest, do nothing
                        }
                        Some(i) => {
                            state.history_index = Some(i - 1);
                        }
                    }

                    if let Some(i) = state.history_index {
                        state.input = state.command_history[i].clone();
                    }
                }
                Key::Named(keyboard::key::Named::ArrowDown) => {
                    match state.history_index {
                        None => {
                            // Not in history mode, do nothing
                        }
                        Some(i) if i >= state.command_history.len() - 1 => {
                            // At newest entry, restore saved input
                            state.history_index = None;
                            state.input = state.saved_input.clone();
                            state.saved_input.clear();
                        }
                        Some(i) => {
                            state.history_index = Some(i + 1);
                            state.input = state.command_history[i + 1].clone();
                        }
                    }
                }
                _ => {}
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
    let font_size = state.font_size;
    let content_elements: Vec<Element<Message>> = state
        .blocks
        .iter()
        .map(|block| view_block(block, font_size))
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

    let result = container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_theme| container::Style {
            background: Some(iced::Background::Color(iced::Color::from_rgb(
                0.07, 0.07, 0.09,
            ))),
            ..Default::default()
        })
        .into();

    result
}

fn subscription(state: &Nexus) -> Subscription<Message> {
    let mut subscriptions = vec![pty_subscription(state.pty_rx.clone())];

    // Listen for all events with window ID - focus-dependent routing happens in update()
    subscriptions.push(
        event::listen_with(|event, _status, window_id| {
            Some(Message::Event(event, window_id))
        })
    );

    // VSYNC SUBSCRIPTION for throttled rendering:
    // Only subscribe to frame events if we have pending changes (is_dirty).
    // This fires when the monitor is ready for the next frame (VSync-aligned),
    // adapting to the user's monitor refresh rate (60Hz, 120Hz, 144Hz, etc.).
    // When idle (is_dirty = false), we don't subscribe, ensuring 0% CPU usage.
    if state.is_dirty {
        subscriptions.push(
            iced::window::frames().map(|_| Message::NextFrame(Instant::now()))
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
    let trimmed = command.trim().to_string();

    // Record in history (skip duplicates of last command, skip empty)
    if !trimmed.is_empty() {
        if state.command_history.last() != Some(&trimmed) {
            state.command_history.push(trimmed.clone());
            // Limit history size to prevent unbounded growth
            if state.command_history.len() > 1000 {
                state.command_history.remove(0);
            }
        }
    }

    // Reset history navigation state
    state.history_index = None;
    state.saved_input.clear();

    let block_id = BlockId(state.next_block_id);
    state.next_block_id += 1;

    // Handle built-in commands
    if trimmed == "clear" {
        state.blocks.clear();
        state.block_index.clear();
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
    let block_idx = state.blocks.len();
    state.blocks.push(block);
    state.block_index.insert(block_id, block_idx);

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
            // O(1) lookup via HashMap
            if let Some(&idx) = state.block_index.get(&block_id) {
                if let Some(block) = state.blocks.get_mut(idx) {
                    block.state = BlockState::Failed(1);
                    block.parser.feed(format!("Error: {}\n", e).as_bytes());
                    block.version += 1;
                }
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

fn view_block(block: &Block, font_size: f32) -> Element<'_, Message> {
    let prompt_color = iced::Color::from_rgb(0.3, 0.8, 0.5);
    let command_color = iced::Color::from_rgb(0.9, 0.9, 0.9);

    let prompt_line = row![
        text("$ ")
            .size(font_size)
            .color(prompt_color)
            .font(iced::Font::MONOSPACE),
        text(&block.command)
            .size(font_size)
            .color(command_color)
            .font(iced::Font::MONOSPACE),
    ]
    .spacing(0);

    // Terminal output - only show cursor for running commands
    // For RUNNING blocks: use viewport-only grid (O(1) extraction)
    // For FINISHED blocks: use full scrollback (cached, O(1) after first extraction)
    // For alternate screen (TUI apps): always viewport only
    let output: Element<Message> = if block.collapsed {
        column![].into()
    } else {
        let grid = if block.parser.is_alternate_screen() || block.is_running() {
            // Running or alternate screen: viewport only (fast, O(1))
            block.parser.grid()
        } else {
            // Finished blocks: show all content including scrollback
            // This is cached after first extraction
            block.parser.grid_with_scrollback()
        };

        // Use GPU shader renderer for performance
        let content_rows = grid.content_rows() as usize;
        let (_cell_width, cell_height) = get_cell_metrics(font_size);
        TerminalShader::<Message>::new(&grid, font_size, 0, content_rows, cell_height)
            .widget()
            .into()
    };

    column![prompt_line, output].spacing(0).into()
}

fn view_input(state: &Nexus) -> Element<'_, Message> {
    // Cornflower blue for path
    let path_color = iced::Color::from_rgb8(100, 149, 237);
    // Green for success, red for failure
    let prompt_color = match state.last_exit_code {
        Some(code) if code != 0 => iced::Color::from_rgb8(220, 50, 50), // Red
        _ => iced::Color::from_rgb8(50, 205, 50), // Lime green
    };

    // Shorten path (replace home with ~)
    let display_path = shorten_path(&state.cwd);

    let path_text = text(format!("{} ", display_path))
        .size(state.font_size)
        .color(path_color)
        .font(iced::Font::MONOSPACE);

    let prompt = text("$ ")
        .size(state.font_size)
        .color(prompt_color)
        .font(iced::Font::MONOSPACE);

    let input = text_input("", &state.input)
        .on_input(Message::InputChanged)
        .on_submit(Message::Submit)
        .padding(0)
        .size(state.font_size)
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

    row![path_text, prompt, input]
        .spacing(0)
        .align_y(iced::Alignment::Center)
        .into()
}

/// Shorten a path by replacing home directory with ~.
fn shorten_path(path: &str) -> String {
    if let Some(home) = home_dir() {
        let home_str = home.display().to_string();
        if path.starts_with(&home_str) {
            return path.replacen(&home_str, "~", 1);
        }
    }
    path.to_string()
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}
