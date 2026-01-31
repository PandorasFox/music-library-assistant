//! Text Input State
//!
//! State management for text input fields with cursor navigation.
//!
//! ## Features
//!
//! - Cursor positioning with Left/Right arrow keys
//! - Home/End to jump to start/end
//! - Backspace/Delete for character removal
//! - Character insertion at cursor position
//! - Emacs-style editing (Ctrl+A/E/U/K)

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// State for a text input field
#[derive(Clone, Debug, Default)]
pub struct TextInputState {
    /// The text content
    pub value: String,
    /// Cursor position (character index, not byte index)
    pub cursor: usize,
    /// Whether the input is currently focused
    pub focused: bool,
}

impl TextInputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the value and move cursor to end
    pub fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.chars().count();
    }

    /// Clear the input
    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    /// Get the value
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    /// Insert a character at the cursor position
    pub fn insert_char(&mut self, c: char) {
        let byte_idx = self.cursor_byte_index();
        self.value.insert(byte_idx, c);
        self.cursor += 1;
    }

    /// Delete the character before the cursor (backspace)
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            let byte_idx = self.cursor_byte_index();
            let char_len = self.value[byte_idx..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            self.value.drain(byte_idx..byte_idx + char_len);
        }
    }

    /// Delete the character at the cursor (delete key)
    pub fn delete(&mut self) {
        let char_count = self.value.chars().count();
        if self.cursor < char_count {
            let byte_idx = self.cursor_byte_index();
            let char_len = self.value[byte_idx..].chars().next().map(|c| c.len_utf8()).unwrap_or(0);
            self.value.drain(byte_idx..byte_idx + char_len);
        }
    }

    /// Move cursor left
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move cursor right
    pub fn move_right(&mut self) {
        let char_count = self.value.chars().count();
        if self.cursor < char_count {
            self.cursor += 1;
        }
    }

    /// Move cursor to start (Home)
    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to end (End)
    pub fn move_end(&mut self) {
        self.cursor = self.value.chars().count();
    }

    /// Delete from cursor to end of line (Ctrl+K)
    pub fn kill_to_end(&mut self) {
        let byte_idx = self.cursor_byte_index();
        self.value.truncate(byte_idx);
    }

    /// Delete from start to cursor (Ctrl+U)
    pub fn kill_to_start(&mut self) {
        let byte_idx = self.cursor_byte_index();
        self.value.drain(..byte_idx);
        self.cursor = 0;
    }

    /// Convert character index to byte index
    fn cursor_byte_index(&self) -> usize {
        self.value
            .char_indices()
            .nth(self.cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(self.value.len())
    }

    /// Handle a key event, returning true if the event was consumed
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    match c {
                        'u' | 'U' => self.kill_to_start(),
                        'k' | 'K' => self.kill_to_end(),
                        'a' | 'A' => self.move_home(),
                        'e' | 'E' => self.move_end(),
                        _ => return false,
                    }
                } else {
                    self.insert_char(c);
                }
                true
            }
            KeyCode::Backspace => {
                self.backspace();
                true
            }
            KeyCode::Delete => {
                self.delete();
                true
            }
            KeyCode::Left => {
                self.move_left();
                true
            }
            KeyCode::Right => {
                self.move_right();
                true
            }
            KeyCode::Home => {
                self.move_home();
                true
            }
            KeyCode::End => {
                self.move_end();
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_input() {
        let mut state = TextInputState::new();

        state.insert_char('h');
        state.insert_char('e');
        state.insert_char('l');
        state.insert_char('l');
        state.insert_char('o');

        assert_eq!(state.value(), "hello");
        assert_eq!(state.cursor, 5);
    }

    #[test]
    fn test_cursor_movement() {
        let mut state = TextInputState::new();
        state.set_value("hello");
        assert_eq!(state.cursor, 5);

        state.move_left();
        assert_eq!(state.cursor, 4);

        state.move_home();
        assert_eq!(state.cursor, 0);

        state.move_end();
        assert_eq!(state.cursor, 5);

        state.move_right(); // Should not go past end
        assert_eq!(state.cursor, 5);
    }

    #[test]
    fn test_backspace() {
        let mut state = TextInputState::new();
        state.set_value("hello");

        state.backspace();
        assert_eq!(state.value(), "hell");
        assert_eq!(state.cursor, 4);

        state.move_home();
        state.backspace(); // Should do nothing at start
        assert_eq!(state.value(), "hell");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn test_delete() {
        let mut state = TextInputState::new();
        state.set_value("hello");
        state.move_home();

        state.delete();
        assert_eq!(state.value(), "ello");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn test_insert_at_cursor() {
        let mut state = TextInputState::new();
        state.set_value("hllo");
        state.cursor = 1; // After 'h'

        state.insert_char('e');
        assert_eq!(state.value(), "hello");
        assert_eq!(state.cursor, 2);
    }

    #[test]
    fn test_utf8_handling() {
        let mut state = TextInputState::new();
        state.set_value("héllo");
        assert_eq!(state.cursor, 5); // 5 characters

        state.move_left();
        state.move_left();
        assert_eq!(state.cursor, 3);

        state.backspace();
        assert_eq!(state.value(), "hélo");
    }

    #[test]
    fn test_kill_commands() {
        let mut state = TextInputState::new();
        state.set_value("hello world");
        state.cursor = 5;

        state.kill_to_end();
        assert_eq!(state.value(), "hello");

        state.set_value("hello world");
        state.cursor = 5;

        state.kill_to_start();
        assert_eq!(state.value(), " world");
        assert_eq!(state.cursor, 0);
    }
}
