//! Keyboard handling and key-to-byte conversion.

use iced::keyboard::{self, Key, Modifiers};

/// Convert a keyboard key to bytes to send to the PTY.
pub fn key_to_bytes(key: &Key, modifiers: &Modifiers) -> Option<Vec<u8>> {
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
