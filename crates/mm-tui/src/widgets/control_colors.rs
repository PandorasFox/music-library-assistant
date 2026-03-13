//! Centralized Control Hint Colors
//!
//! Semantic color definitions for control hints across all modals and views.
//! This ensures consistent styling throughout the application.
//!
//! ## Color Semantics
//!
//! - **NAV** (Yellow): Navigation keys (arrows, ^/v, </>)
//! - **TOGGLE** (Magenta): Toggle/selection keys (Space, checkboxes)
//! - **EDIT** (Magenta): Edit operation keys (E)
//! - **ACTION** (Yellow): Action description text
//! - **CONFIRM** (Green): Confirm/apply keys (Enter)
//! - **REVIEW** (Blue): Review mode keys (^R)
//! - **CANCEL** (DarkGray): Cancel/escape keys (Esc)
//!
//! ## General UI Guidelines
//!
//! - Magenta for unfocused editable fields
//! - Yellow for focusable fields and actively focused list entries

use ratatui::style::Style;
use ratatui::text::Span;

/// Semantic color constants for control hints.
pub mod colors {
    use ratatui::style::Color;

    /// Navigation keys (^/v, </>, arrows)
    pub const NAV: Color = Color::Yellow;
    /// Toggle/selection keys (Space, checkboxes)
    pub const TOGGLE: Color = Color::Magenta;
    /// Edit operation keys (E)
    pub const EDIT: Color = Color::Magenta;
    /// Action description text
    pub const ACTION: Color = Color::Yellow;
    /// Confirm/apply keys (Enter)
    pub const CONFIRM: Color = Color::Green;
    /// Review mode keys (^R)
    pub const REVIEW: Color = Color::Blue;
    /// Cancel/escape keys (Esc)
    pub const CANCEL: Color = Color::DarkGray;
}

/// Create a styled span for a navigation key.
pub fn nav(key: &str) -> Span<'static> {
    Span::styled(key.to_string(), Style::default().fg(colors::NAV))
}

/// Create a styled span for a toggle key.
pub fn toggle(key: &str) -> Span<'static> {
    Span::styled(key.to_string(), Style::default().fg(colors::TOGGLE))
}

/// Create a styled span for an edit key.
pub fn edit(key: &str) -> Span<'static> {
    Span::styled(key.to_string(), Style::default().fg(colors::EDIT))
}

/// Create a styled span for action text.
pub fn action(text: &str) -> Span<'static> {
    Span::styled(text.to_string(), Style::default().fg(colors::ACTION))
}

/// Create a styled span for a confirm key.
pub fn confirm(key: &str) -> Span<'static> {
    Span::styled(key.to_string(), Style::default().fg(colors::CONFIRM))
}

/// Create a styled span for a review key.
pub fn review(key: &str) -> Span<'static> {
    Span::styled(key.to_string(), Style::default().fg(colors::REVIEW))
}

/// Create a styled span for a cancel key.
pub fn cancel(key: &str) -> Span<'static> {
    Span::styled(key.to_string(), Style::default().fg(colors::CANCEL))
}

/// Create a plain text span (for separators and descriptions).
pub fn text(s: &str) -> Span<'static> {
    Span::raw(s.to_string())
}
