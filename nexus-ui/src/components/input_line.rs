//! Input line component - command input with editing.

/// The input line state.
pub struct InputLine {
    /// Current input text.
    text: String,

    /// Cursor position (byte offset).
    cursor: usize,

    /// History index (-1 for current input).
    history_index: i32,

    /// Saved current input when browsing history.
    saved_input: String,
}

impl InputLine {
    /// Create a new input line.
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            history_index: -1,
            saved_input: String::new(),
        }
    }

    /// Get the current text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Get the cursor position.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Insert a character at the cursor.
    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    /// Insert a string at the cursor.
    pub fn insert_str(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            // Find the previous character boundary
            let prev = self.text[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);

            self.text.remove(prev);
            self.cursor = prev;
        }
    }

    /// Delete the character at the cursor.
    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    /// Move cursor left.
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor = self.text[..self.cursor]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    /// Move cursor right.
    pub fn move_right(&mut self) {
        if self.cursor < self.text.len() {
            self.cursor = self.text[self.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor + i)
                .unwrap_or(self.text.len());
        }
    }

    /// Move cursor to start.
    pub fn move_to_start(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to end.
    pub fn move_to_end(&mut self) {
        self.cursor = self.text.len();
    }

    /// Move cursor to previous word.
    pub fn move_word_left(&mut self) {
        // Skip whitespace, then skip word
        let text = &self.text[..self.cursor];

        // Find last non-whitespace
        let last_non_ws = text
            .char_indices()
            .rev()
            .find(|(_, c)| !c.is_whitespace())
            .map(|(i, _)| i);

        if let Some(pos) = last_non_ws {
            // Find the start of this word
            let word_start = self.text[..=pos]
                .char_indices()
                .rev()
                .find(|(_, c)| c.is_whitespace())
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0);

            self.cursor = word_start;
        } else {
            self.cursor = 0;
        }
    }

    /// Move cursor to next word.
    pub fn move_word_right(&mut self) {
        // Skip current word, then skip whitespace
        let text = &self.text[self.cursor..];

        // Find first whitespace
        let first_ws = text
            .char_indices()
            .find(|(_, c)| c.is_whitespace())
            .map(|(i, _)| self.cursor + i);

        if let Some(pos) = first_ws {
            // Find the first non-whitespace after
            let word_start = self.text[pos..]
                .char_indices()
                .find(|(_, c)| !c.is_whitespace())
                .map(|(i, _)| pos + i)
                .unwrap_or(self.text.len());

            self.cursor = word_start;
        } else {
            self.cursor = self.text.len();
        }
    }

    /// Delete word before cursor.
    pub fn delete_word_back(&mut self) {
        let old_cursor = self.cursor;
        self.move_word_left();
        self.text.drain(self.cursor..old_cursor);
    }

    /// Clear the input line.
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.history_index = -1;
    }

    /// Set the text (e.g., from history).
    pub fn set_text(&mut self, text: String) {
        self.text = text;
        self.cursor = self.text.len();
    }

    /// Take the current text, clearing the input.
    pub fn take(&mut self) -> String {
        self.cursor = 0;
        self.history_index = -1;
        std::mem::take(&mut self.text)
    }
}

impl Default for InputLine {
    fn default() -> Self {
        Self::new()
    }
}
