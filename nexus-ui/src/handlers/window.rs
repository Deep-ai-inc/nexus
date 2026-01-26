//! Window domain handler.
//!
//! Handles window resize, zoom, global shortcuts, and event routing.

use std::sync::atomic::Ordering;

use iced::keyboard::{self, Key};
use iced::{Event, Task};

use crate::blocks::Focus;
use crate::constants::{CHAR_WIDTH_RATIO, DEFAULT_FONT_SIZE, LINE_HEIGHT_FACTOR};
use crate::msg::{GlobalShortcut, InputMessage, Message, WindowMessage, ZoomDirection};
use crate::state::Nexus;

/// Try to paste an image file from clipboard file list.
/// Detects image type by file content (magic bytes), not extension.
fn try_paste_image_file(clipboard: &mut arboard::Clipboard) -> Option<Task<Message>> {
    // Use arboard's native file list API
    let files = match clipboard.get().file_list() {
        Ok(files) => files,
        Err(e) => {
            tracing::debug!("No file list in clipboard: {}", e);
            return None;
        }
    };

    tracing::debug!("Clipboard contains {} files", files.len());

    // Try each file - detect image by content, not extension
    for path in files {
        tracing::debug!("Trying to paste file: {:?}", path);

        if !path.exists() {
            tracing::debug!("File does not exist: {:?}", path);
            continue;
        }
        if !path.is_file() {
            tracing::debug!("Path is not a file: {:?}", path);
            continue;
        }

        // Read file and try to decode as image (auto-detects format by magic bytes)
        let file_data = match std::fs::read(&path) {
            Ok(data) => {
                tracing::debug!("Read {} bytes from {:?}", data.len(), path);
                data
            }
            Err(e) => {
                tracing::warn!("Failed to read file {:?}: {}", path, e);
                continue;
            }
        };

        // image::load_from_memory detects format from file header, not extension
        let img = match image::load_from_memory(&file_data) {
            Ok(img) => img,
            Err(e) => {
                tracing::debug!("Not an image file {:?}: {}", path, e);
                continue;
            }
        };

        let width = img.width();
        let height = img.height();
        tracing::info!("Pasting image {}x{} from {:?}", width, height, path);

        // Convert to PNG for consistent handling
        let mut png_data = Vec::new();
        if let Err(e) = img.write_to(
            &mut std::io::Cursor::new(&mut png_data),
            image::ImageFormat::Png,
        ) {
            tracing::warn!("Failed to encode image as PNG: {}", e);
            continue;
        }

        return Some(Task::done(Message::Input(InputMessage::PasteImage(
            png_data, width, height,
        ))));
    }

    None
}

/// Update the window domain state.
pub fn update(state: &mut Nexus, msg: WindowMessage) -> Task<Message> {
    match msg {
        WindowMessage::Event(evt, id) => handle_event(state, evt, id),
        WindowMessage::Resized(w, h) => resize(state, w, h),
        WindowMessage::Shortcut(sc) => global_shortcut(state, sc),
        WindowMessage::Zoom(dir) => zoom(state, dir),
    }
}

// =============================================================================
// Event Routing
// =============================================================================

/// Handle all window/keyboard events and route to appropriate handlers.
pub fn handle_event(
    state: &mut Nexus,
    event: Event,
    window_id: iced::window::Id,
) -> Task<Message> {
    // Capture window ID
    if state.window.id.is_none() {
        state.window.id = Some(window_id);
    }

    match event {
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            // Global shortcuts (Cmd+K, Cmd+Q, etc.)
            if modifiers.command() {
                if let Key::Character(c) = &key {
                    let ch = c.to_lowercase();
                    let task = match ch.as_str() {
                        "k" => Some(global_shortcut(state, GlobalShortcut::ClearScreen)),
                        "w" => Some(global_shortcut(state, GlobalShortcut::CloseWindow)),
                        "q" => Some(global_shortcut(state, GlobalShortcut::Quit)),
                        "c" => Some(global_shortcut(state, GlobalShortcut::Copy)),
                        "v" => Some(global_shortcut(state, GlobalShortcut::Paste)),
                        "=" | "+" => Some(zoom(state, ZoomDirection::In)),
                        "-" => Some(zoom(state, ZoomDirection::Out)),
                        "0" => Some(zoom(state, ZoomDirection::Reset)),
                        "." => {
                            // Toggle input mode
                            state.input.mode = match state.input.mode {
                                crate::blocks::InputMode::Shell => crate::blocks::InputMode::Agent,
                                crate::blocks::InputMode::Agent => crate::blocks::InputMode::Shell,
                            };
                            state.input.suppress_next = true;
                            return Task::none();
                        }
                        _ => None,
                    };
                    if let Some(task) = task {
                        state.input.suppress_next = true;
                        return task;
                    }
                }
            }

            // Ctrl+C in input clears the line
            if modifiers.control() && matches!(state.terminal.focus, Focus::Input) {
                if let Key::Character(c) = &key {
                    match c.to_lowercase().as_str() {
                        "c" => {
                            state.input.buffer.clear();
                            state.input.history_index = None;
                            state.input.saved_input.clear();
                            state.input.search_active = false;
                            state.terminal.permission_denied_command = None;
                            return Task::none();
                        }
                        "r" => {
                            // Start history search - call through InputMessage
                            return Task::done(Message::Input(InputMessage::HistorySearchStart));
                        }
                        "s" => {
                            if state.terminal.permission_denied_command.is_some() {
                                return super::terminal::retry_sudo(state);
                            }
                        }
                        _ => {}
                    }
                }
            }

            // Focus-dependent key handling
            match state.terminal.focus {
                Focus::Input => {
                    // Use the new handle_focus_key which returns Option<InputMessage>
                    if let Some(input_msg) = super::input::handle_focus_key(
                        &mut state.input,
                        key,
                        modifiers,
                    ) {
                        return Task::done(Message::Input(input_msg));
                    }
                }
                Focus::Block(_) => {
                    return super::terminal::handle_key(state, key, modifiers);
                }
            }
        }
        Event::Window(iced::window::Event::Resized(size)) => {
            return resize(state, size.width as u32, size.height as u32);
        }
        _ => {}
    }
    Task::none()
}

// =============================================================================
// Window Resize
// =============================================================================

/// Handle window resize.
pub fn resize(state: &mut Nexus, width: u32, height: u32) -> Task<Message> {
    state.window.dims = (width as f32, height as f32);
    let old_cols = state.terminal.terminal_size.0;
    let (cols, rows) = state.recalculate_terminal_size();
    if cols != old_cols {
        state.apply_resize(cols, rows);
    }
    Task::none()
}

// =============================================================================
// Global Shortcuts
// =============================================================================

/// Handle global shortcuts (Cmd+K, Cmd+Q, etc.).
pub fn global_shortcut(state: &mut Nexus, shortcut: GlobalShortcut) -> Task<Message> {
    // Strip the shortcut character if text_input just typed it
    let strip_char = match &shortcut {
        GlobalShortcut::ClearScreen => Some('k'),
        GlobalShortcut::CloseWindow => Some('w'),
        GlobalShortcut::Quit => Some('q'),
        GlobalShortcut::Copy => Some('c'),
        GlobalShortcut::Paste => Some('v'),
    };
    if let Some(ch) = strip_char {
        let expected_lower = format!("{}{}", state.input.before_event, ch);
        let expected_upper = format!("{}{}", state.input.before_event, ch.to_ascii_uppercase());
        if state.input.buffer == expected_lower || state.input.buffer == expected_upper {
            state.input.buffer.pop();
        }
    }

    match shortcut {
        GlobalShortcut::ClearScreen => {
            // Cancel agent and clear everything
            state.agent.cancel_flag.store(true, Ordering::SeqCst);
            state.terminal.blocks.clear();
            state.terminal.block_index.clear();
            state.agent.blocks.clear();
            state.agent.block_index.clear();
            state.agent.active_block = None;
        }
        GlobalShortcut::CloseWindow | GlobalShortcut::Quit => {
            return iced::exit();
        }
        GlobalShortcut::Copy => {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                let _ = clipboard.set_text(&state.input.buffer);
            }
        }
        GlobalShortcut::Paste => {
            if let Ok(mut clipboard) = arboard::Clipboard::new() {
                // First check if clipboard text is an image file path
                if let Some(task) = try_paste_image_file(&mut clipboard) {
                    return task;
                }

                // Try clipboard image data (e.g., from screenshot or copied image content)
                if let Ok(img) = clipboard.get_image() {
                    let width = img.width as u32;
                    let height = img.height as u32;

                    let mut png_data = Vec::new();
                    {
                        use image::{ImageBuffer, RgbaImage};
                        let img_buf: RgbaImage =
                            ImageBuffer::from_raw(width, height, img.bytes.into_owned())
                                .unwrap_or_else(|| ImageBuffer::new(1, 1));

                        img_buf
                            .write_to(
                                &mut std::io::Cursor::new(&mut png_data),
                                image::ImageFormat::Png,
                            )
                            .ok();
                    }

                    if !png_data.is_empty() {
                        return Task::done(Message::Input(InputMessage::PasteImage(
                            png_data, width, height,
                        )));
                    }
                }

                // Fall back to text
                if let Ok(text) = clipboard.get_text() {
                    state.input.buffer.push_str(&text);
                }
            }
        }
    }
    Task::none()
}

// =============================================================================
// Zoom
// =============================================================================

/// Handle zoom (font size) changes.
pub fn zoom(state: &mut Nexus, direction: ZoomDirection) -> Task<Message> {
    // Strip shortcut character
    let strip_chars: &[char] = match &direction {
        ZoomDirection::In => &['=', '+'],
        ZoomDirection::Out => &['-'],
        ZoomDirection::Reset => &['0'],
    };
    for &ch in strip_chars {
        let expected = format!("{}{}", state.input.before_event, ch);
        if state.input.buffer == expected {
            state.input.buffer.pop();
            break;
        }
    }

    let old_size = state.window.font_size;
    state.window.font_size = match direction {
        ZoomDirection::In => (state.window.font_size + 1.0).min(32.0),
        ZoomDirection::Out => (state.window.font_size - 1.0).max(8.0),
        ZoomDirection::Reset => DEFAULT_FONT_SIZE,
    };

    if (state.window.font_size - old_size).abs() > 0.001 {
        let (cols, rows) = state.terminal.terminal_size;
        let new_char_width = state.window.font_size * CHAR_WIDTH_RATIO;
        let new_line_height = state.window.font_size * LINE_HEIGHT_FACTOR;

        let h_padding = 30.0;
        let v_padding = 80.0;

        let new_width = (cols as f32 * new_char_width) + h_padding;
        let new_height = (rows as f32 * new_line_height) + v_padding;

        state.window.dims = (new_width, new_height);

        if let Some(window_id) = state.window.id {
            return iced::window::resize(window_id, iced::Size::new(new_width, new_height));
        }
    }
    Task::none()
}
