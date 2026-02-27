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

use crate::ui::input::InputAction;

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

    /// Move cursor one word to the left (Ctrl+Left).
    ///
    /// Skips whitespace backward, then skips word characters backward.
    /// Word characters: alphanumeric or underscore.
    pub fn move_word_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let chars: Vec<char> = self.value.chars().collect();
        let mut pos = self.cursor;
        // Skip whitespace/non-word chars backward
        while pos > 0 && !Self::is_word_char(chars[pos - 1]) {
            pos -= 1;
        }
        // Skip word chars backward
        while pos > 0 && Self::is_word_char(chars[pos - 1]) {
            pos -= 1;
        }
        self.cursor = pos;
    }

    /// Move cursor one word to the right (Ctrl+Right).
    ///
    /// Skips word characters forward, then skips whitespace forward.
    /// Word characters: alphanumeric or underscore.
    pub fn move_word_right(&mut self) {
        let chars: Vec<char> = self.value.chars().collect();
        let len = chars.len();
        if self.cursor >= len {
            return;
        }
        let mut pos = self.cursor;
        // Skip word chars forward
        while pos < len && Self::is_word_char(chars[pos]) {
            pos += 1;
        }
        // Skip whitespace/non-word chars forward
        while pos < len && !Self::is_word_char(chars[pos]) {
            pos += 1;
        }
        self.cursor = pos;
    }

    /// Whether a character is a "word" character for word navigation.
    fn is_word_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
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

    /// Split value into (before_cursor, cursor_char, after_cursor) for rendering.
    ///
    /// Returns UTF-8 safe slices based on the character-level cursor position.
    /// The cursor_char is the character under the cursor (or space if at end).
    pub fn cursor_splits(&self) -> (&str, char, &str) {
        let byte_idx = self.cursor_byte_index();
        let (before, rest) = self.value.split_at(byte_idx);
        let cursor_char = rest.chars().next().unwrap_or(' ');
        let after = if rest.len() > cursor_char.len_utf8() {
            &rest[cursor_char.len_utf8()..]
        } else {
            ""
        };
        (before, cursor_char, after)
    }

    /// Convert character index to byte index
    fn cursor_byte_index(&self) -> usize {
        self.value
            .char_indices()
            .nth(self.cursor)
            .map(|(idx, _)| idx)
            .unwrap_or(self.value.len())
    }

    /// Insert a string at cursor position (for paste support).
    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert_char(c);
        }
    }

    /// Handle a semantic input action, returning true if the event was consumed.
    pub fn handle_input(&mut self, action: &InputAction) -> bool {
        match action {
            InputAction::Char(c) => { self.insert_char(*c); true }
            // Space is mapped to Toggle globally for list selection, but in
            // text fields it's a space character.
            InputAction::Toggle => { self.insert_char(' '); true }
            InputAction::Paste(text) => { self.insert_str(text); true }
            InputAction::Backspace => { self.backspace(); true }
            InputAction::Delete => { self.delete(); true }
            InputAction::NavLeft => { self.move_left(); true }
            InputAction::NavRight => { self.move_right(); true }
            InputAction::WordLeft => { self.move_word_left(); true }
            InputAction::WordRight => { self.move_word_right(); true }
            InputAction::Home | InputAction::TextHome => { self.move_home(); true }
            InputAction::End | InputAction::TextEnd => { self.move_end(); true }
            InputAction::KillToStart => { self.kill_to_start(); true }
            InputAction::KillToEnd => { self.kill_to_end(); true }
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

    #[test]
    fn test_cursor_splits_multibyte() {
        let mut state = TextInputState::new();
        // 4-byte mathematical bold fraktur characters
        state.set_value("𝕮𝖆𝖒");
        assert_eq!(state.cursor, 3); // 3 chars

        // Cursor at end: cursor_char should be space (past end)
        let (before, ch, after) = state.cursor_splits();
        assert_eq!(before, "𝕮𝖆𝖒");
        assert_eq!(ch, ' ');
        assert_eq!(after, "");

        // Cursor at start
        state.cursor = 0;
        let (before, ch, after) = state.cursor_splits();
        assert_eq!(before, "");
        assert_eq!(ch, '𝕮');
        assert_eq!(after, "𝖆𝖒");

        // Cursor in middle
        state.cursor = 1;
        let (before, ch, after) = state.cursor_splits();
        assert_eq!(before, "𝕮");
        assert_eq!(ch, '𝖆');
        assert_eq!(after, "𝖒");
    }

    #[test]
    fn test_cursor_splits_mixed_unicode() {
        let mut state = TextInputState::new();
        state.set_value("HHSU 𓃚 𝕮");
        // 'H','H','S','U',' ','𓃚',' ','𝕮' = 8 chars
        assert_eq!(state.cursor, 8);

        // Cursor on the hieroglyph
        state.cursor = 5;
        let (before, ch, after) = state.cursor_splits();
        assert_eq!(before, "HHSU ");
        assert_eq!(ch, '𓃚');
        assert_eq!(after, " 𝕮");
    }

    #[test]
    fn test_word_navigation() {
        let mut state = TextInputState::new();
        state.set_value("hello world foo_bar");
        assert_eq!(state.cursor, 19); // at end

        // Word left from end: skip to start of "foo_bar"
        state.move_word_left();
        assert_eq!(state.cursor, 12);

        // Word left again: skip to start of "world"
        state.move_word_left();
        assert_eq!(state.cursor, 6);

        // Word left again: skip to start of "hello"
        state.move_word_left();
        assert_eq!(state.cursor, 0);

        // Word left at start: stay at 0
        state.move_word_left();
        assert_eq!(state.cursor, 0);

        // Word right from start: skip to start of "world"
        state.move_word_right();
        assert_eq!(state.cursor, 6);

        // Word right: skip to start of "foo_bar"
        state.move_word_right();
        assert_eq!(state.cursor, 12);

        // Word right: skip to end
        state.move_word_right();
        assert_eq!(state.cursor, 19);

        // Word right at end: stay at end
        state.move_word_right();
        assert_eq!(state.cursor, 19);
    }

    #[test]
    fn test_word_navigation_multiple_spaces() {
        let mut state = TextInputState::new();
        state.set_value("hello   world");
        state.cursor = 13; // at end

        state.move_word_left();
        assert_eq!(state.cursor, 8); // start of "world"

        state.move_word_left();
        assert_eq!(state.cursor, 0); // start of "hello"
    }

    #[test]
    fn test_handle_input_word_nav() {
        let mut state = TextInputState::new();
        state.set_value("one two three");

        let consumed = state.handle_input(&InputAction::WordLeft);
        assert!(consumed);
        assert_eq!(state.cursor, 8); // start of "three"

        let consumed = state.handle_input(&InputAction::WordRight);
        assert!(consumed);
        assert_eq!(state.cursor, 13); // end
    }
}
