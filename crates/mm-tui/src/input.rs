//! Semantic input action layer.
//!
//! Every physical input (keyboard, mouse scroll, paste) maps to an [`InputAction`].
//! No view should ever match on raw `KeyEvent`/`KeyCode` — all input flows through here.

pub use mm_ui::input::InputAction;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Map a raw [`KeyEvent`] to a semantic [`InputAction`].
///
/// Returns an `InputAction` for every key — unknown keys map to `Cancel`
/// which views handle gracefully (either cancel or no-op).
pub fn map_key(key: KeyEvent) -> InputAction {
    // Shift+arrow → Focus
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        match key.code {
            KeyCode::Up => return InputAction::FocusUp,
            KeyCode::Down => return InputAction::FocusDown,
            KeyCode::Left => return InputAction::FocusLeft,
            KeyCode::Right => return InputAction::FocusRight,
            _ => {}
        }
    }

    // Ctrl+non-letter → word navigation, filter (before Ctrl+letter block)
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Left => return InputAction::WordLeft,
            KeyCode::Right => return InputAction::WordRight,
            KeyCode::Char('/') => return InputAction::OpenFilter,
            _ => {}
        }
    }

    // Ctrl+letter → named text-editing actions or generic shortcuts
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('a') => InputAction::TextHome,
            KeyCode::Char('e') => InputAction::TextEnd,
            KeyCode::Char('u') => InputAction::KillToStart,
            KeyCode::Char('k') => InputAction::KillToEnd,
            KeyCode::Char('f') => InputAction::FlagValue,
            KeyCode::Char(c) => InputAction::Shortcut(c.to_ascii_lowercase()),
            _ => InputAction::Cancel,
        };
    }

    match key.code {
        KeyCode::Up => InputAction::NavUp,
        KeyCode::Down => InputAction::NavDown,
        KeyCode::Left => InputAction::NavLeft,
        KeyCode::Right => InputAction::NavRight,
        KeyCode::Enter => InputAction::Confirm,
        KeyCode::Esc => InputAction::Cancel,
        KeyCode::Char(' ') => InputAction::Toggle,
        KeyCode::Char(c) => InputAction::Char(c),
        KeyCode::Tab => InputAction::CycleNext,
        KeyCode::BackTab => InputAction::CyclePrev,
        KeyCode::Home => InputAction::Home,
        KeyCode::End => InputAction::End,
        KeyCode::PageUp => InputAction::PageUp,
        KeyCode::PageDown => InputAction::PageDown,
        KeyCode::Backspace => InputAction::Backspace,
        KeyCode::Delete => InputAction::Delete,
        _ => InputAction::Cancel,
    }
}
