//! Semantic input action layer.
//!
//! Every physical input (keyboard, mouse scroll, paste) maps to an [`InputAction`].
//! No view should ever match on raw key events — all input flows through here.

/// Semantic input actions. Every physical input maps to one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputAction {
    // -- Content navigation (arrow keys) --
    NavUp,
    NavDown,
    NavLeft,
    NavRight,

    // -- Widget-level focus (Shift+arrow) — between panes, buttons --
    FocusUp,
    FocusDown,
    FocusLeft,
    FocusRight,

    // -- Lateral view ring (Tab / Shift+Tab) --
    CycleNext,
    CyclePrev,

    // -- Positional jumps --
    Home,
    End,
    PageUp,
    PageDown,

    // -- Semantic actions --
    Confirm, // Enter
    Cancel,  // Esc
    Toggle,  // Space (selection toggle in lists)
    Backspace,
    Delete,

    // -- Text entry --
    Char(char),    // single unmodified character keypress
    Paste(String), // bracketed paste content

    // -- Text editing (Emacs shortcuts lifted to first-class actions) --
    TextHome,    // Ctrl+A — cursor to start of text field
    TextEnd,     // Ctrl+E — cursor to end of text field
    WordLeft,    // Ctrl+Left — cursor one word left
    WordRight,   // Ctrl+Right — cursor one word right
    KillToStart, // Ctrl+U — delete from cursor to start
    KillToEnd,   // Ctrl+K — delete from cursor to end

    // -- Modified shortcuts --
    OpenFilter,     // Ctrl+/ — open filter popup
    FlagValue,      // Ctrl+F — context-specific flag/mark action
    Shortcut(char), // other Ctrl+letter, lowercase
}
