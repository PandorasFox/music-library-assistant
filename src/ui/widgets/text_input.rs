//! Text Input Widget
//!
//! A text input field with cursor navigation, selection, and rendering support.
//!
//! ## Features
//!
//! - Cursor positioning with Left/Right arrow keys
//! - Home/End to jump to start/end
//! - Backspace/Delete for character removal
//! - Character insertion at cursor position
//! - Visual cursor rendering
//! - Placeholder text support

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

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

    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self.cursor = self.value.chars().count();
        self
    }

    pub fn focused(mut self) -> Self {
        self.focused = true;
        self
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

/// Style configuration for text input
#[derive(Clone)]
pub struct TextInputStyle {
    /// Style for the text when focused
    pub focused_text: Style,
    /// Style for the text when not focused
    pub unfocused_text: Style,
    /// Style for the cursor character
    pub cursor_style: Style,
    /// Style for placeholder text
    pub placeholder_style: Style,
    /// Border color when focused
    pub focused_border: Color,
    /// Border color when not focused
    pub unfocused_border: Color,
}

impl Default for TextInputStyle {
    fn default() -> Self {
        Self {
            focused_text: Style::default().fg(Color::White),
            unfocused_text: Style::default().fg(Color::Gray),
            cursor_style: Style::default().bg(Color::White).fg(Color::Black),
            placeholder_style: Style::default().fg(Color::DarkGray),
            focused_border: Color::Cyan,
            unfocused_border: Color::DarkGray,
        }
    }
}

impl TextInputStyle {
    /// Style for search bars
    pub fn search() -> Self {
        Self {
            focused_border: Color::Yellow,
            ..Default::default()
        }
    }
}

/// A text input widget with cursor support
pub struct TextInput {
    placeholder: String,
    style: TextInputStyle,
    title: Option<String>,
}

impl TextInput {
    pub fn new() -> Self {
        Self {
            placeholder: String::new(),
            style: TextInputStyle::default(),
            title: None,
        }
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    pub fn style(mut self, style: TextInputStyle) -> Self {
        self.style = style;
        self
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Render the text input
    pub fn render(self, f: &mut Frame, area: Rect, state: &TextInputState) {
        let border_color = if state.focused {
            self.style.focused_border
        } else {
            self.style.unfocused_border
        };

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        if let Some(title) = &self.title {
            block = block.title(title.as_str());
        }

        // Build the content line
        let line = if state.value.is_empty() && !state.focused {
            // Show placeholder when empty and not focused
            Line::from(Span::styled(&self.placeholder, self.style.placeholder_style))
        } else if state.value.is_empty() && state.focused {
            // Show cursor on empty focused input
            Line::from(Span::styled(" ", self.style.cursor_style))
        } else if state.focused {
            // Show text with cursor
            let chars: Vec<char> = state.value.chars().collect();
            let mut spans = Vec::new();

            // Text before cursor
            if state.cursor > 0 {
                let before: String = chars[..state.cursor].iter().collect();
                spans.push(Span::styled(before, self.style.focused_text));
            }

            // Cursor character (or space if at end)
            if state.cursor < chars.len() {
                let cursor_char = chars[state.cursor].to_string();
                spans.push(Span::styled(cursor_char, self.style.cursor_style));

                // Text after cursor
                if state.cursor + 1 < chars.len() {
                    let after: String = chars[state.cursor + 1..].iter().collect();
                    spans.push(Span::styled(after, self.style.focused_text));
                }
            } else {
                // Cursor at end - show cursor block
                spans.push(Span::styled(" ", self.style.cursor_style));
            }

            Line::from(spans)
        } else {
            // Unfocused with text
            Line::from(Span::styled(&state.value, self.style.unfocused_text))
        };

        let paragraph = Paragraph::new(line).block(block);
        f.render_widget(paragraph, area);
    }

    /// Render inline (no border, single line) - useful for embedded inputs
    pub fn render_inline(self, f: &mut Frame, area: Rect, state: &TextInputState) {
        let line = if state.value.is_empty() && !state.focused {
            Line::from(Span::styled(&self.placeholder, self.style.placeholder_style))
        } else if state.value.is_empty() && state.focused {
            Line::from(Span::styled(" ", self.style.cursor_style))
        } else if state.focused {
            let chars: Vec<char> = state.value.chars().collect();
            let mut spans = Vec::new();

            if state.cursor > 0 {
                let before: String = chars[..state.cursor].iter().collect();
                spans.push(Span::styled(before, self.style.focused_text));
            }

            if state.cursor < chars.len() {
                let cursor_char = chars[state.cursor].to_string();
                spans.push(Span::styled(cursor_char, self.style.cursor_style));

                if state.cursor + 1 < chars.len() {
                    let after: String = chars[state.cursor + 1..].iter().collect();
                    spans.push(Span::styled(after, self.style.focused_text));
                }
            } else {
                spans.push(Span::styled(" ", self.style.cursor_style));
            }

            Line::from(spans)
        } else {
            Line::from(Span::styled(&state.value, self.style.unfocused_text))
        };

        let paragraph = Paragraph::new(line);
        f.render_widget(paragraph, area);
    }
}

impl Default for TextInput {
    fn default() -> Self {
        Self::new()
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
        let mut state = TextInputState::new().with_value("hello");
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
        let mut state = TextInputState::new().with_value("hello");

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
        let mut state = TextInputState::new().with_value("hello");
        state.move_home();

        state.delete();
        assert_eq!(state.value(), "ello");
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn test_insert_at_cursor() {
        let mut state = TextInputState::new().with_value("hllo");
        state.cursor = 1; // After 'h'

        state.insert_char('e');
        assert_eq!(state.value(), "hello");
        assert_eq!(state.cursor, 2);
    }

    #[test]
    fn test_utf8_handling() {
        let mut state = TextInputState::new().with_value("héllo");
        assert_eq!(state.cursor, 5); // 5 characters

        state.move_left();
        state.move_left();
        assert_eq!(state.cursor, 3);

        state.backspace();
        assert_eq!(state.value(), "hélo");
    }

    #[test]
    fn test_kill_commands() {
        let mut state = TextInputState::new().with_value("hello world");
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
