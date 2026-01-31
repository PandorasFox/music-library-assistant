//! Controls Hint Widget
//!
//! Consistent rendering of keyboard controls and status information.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

/// A single key binding hint
#[derive(Clone)]
pub struct KeyBinding {
    /// The key(s) to press (e.g., "Tab", "Ctrl+S", "Up/Down")
    pub key: String,
    /// Description of what the key does
    pub action: String,
}

impl KeyBinding {
    pub fn new(key: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            action: action.into(),
        }
    }
}

/// Style for rendering key bindings
#[derive(Clone, Copy)]
pub struct ControlsStyle {
    /// Style for the key portion
    pub key_style: Style,
    /// Style for the action description
    pub action_style: Style,
    /// Separator between key and action
    pub separator: &'static str,
    /// Separator between bindings
    pub binding_separator: &'static str,
}

impl Default for ControlsStyle {
    fn default() -> Self {
        Self {
            key_style: Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
            action_style: Style::default().fg(Color::Gray),
            separator: " ",
            binding_separator: " | ",
        }
    }
}

/// A widget for displaying keyboard controls hints.
///
/// # Example
/// ```ignore
/// ControlsHint::new()
///     .binding(KeyBinding::new("Up/Down", "Navigate"))
///     .binding(KeyBinding::new("Enter", "Select"))
///     .binding(KeyBinding::new("Esc", "Back"))
///     .render_line()
/// ```
pub struct ControlsHint {
    bindings: Vec<KeyBinding>,
    style: ControlsStyle,
}

impl Default for ControlsHint {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlsHint {
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
            style: ControlsStyle::default(),
        }
    }

    /// Add a key binding
    pub fn binding(mut self, binding: KeyBinding) -> Self {
        self.bindings.push(binding);
        self
    }

    /// Render as a single Line (for inline use)
    pub fn render_line(&self) -> Line<'static> {
        let mut spans: Vec<Span> = Vec::new();

        for (i, binding) in self.bindings.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(
                    self.style.binding_separator,
                    self.style.action_style,
                ));
            }
            spans.push(Span::styled(binding.key.clone(), self.style.key_style));
            spans.push(Span::styled(
                self.style.separator,
                self.style.action_style,
            ));
            spans.push(Span::styled(
                binding.action.clone(),
                self.style.action_style,
            ));
        }

        Line::from(spans)
    }
}

/// Helper to quickly build common control sets
pub mod presets {
    use super::{ControlsHint, KeyBinding};

    /// Tag editor controls
    pub fn tag_editor() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("Tab", "Tracks"))
            .binding(KeyBinding::new("↑↓", "Fields"))
            .binding(KeyBinding::new("Enter", "Edit"))
            .binding(KeyBinding::new("Esc", "Exit"))
    }

    /// Deployment preview controls
    pub fn deployment_preview() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("↑↓", "Navigate"))
            .binding(KeyBinding::new("←→", "Focus"))
            .binding(KeyBinding::new("Enter", "Confirm"))
            .binding(KeyBinding::new("Tab/Shift+Tab", "Cycle"))
            .binding(KeyBinding::new("Esc", "Cancel"))
    }

    /// Exit confirmation modal controls
    pub fn exit_confirm_modal() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("←→", "Select"))
            .binding(KeyBinding::new("Enter/Space", "Confirm"))
            .binding(KeyBinding::new("Y", "Yes"))
            .binding(KeyBinding::new("N/Esc", "No"))
    }

    /// Corpus browser controls
    pub fn corpus_browser() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("↑↓", "Navigate"))
            .binding(KeyBinding::new("←→", "Expand"))
            .binding(KeyBinding::new("Enter", "Edit"))
            .binding(KeyBinding::new("Tab/Shift+Tab", "Cycle"))
            .binding(KeyBinding::new("Esc", "Exit"))
    }

    /// Insights view controls
    pub fn insights_view() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("↑↓", "Navigate"))
            .binding(KeyBinding::new("Enter", "Launch"))
            .binding(KeyBinding::new("Tab/Shift+Tab", "Cycle"))
            .binding(KeyBinding::new("Esc", "Menu"))
    }

    /// Format standardization controls
    pub fn format_standardization() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("↑↓", "Select action"))
            .binding(KeyBinding::new("←→", "Adjust bitrate"))
            .binding(KeyBinding::new("Enter", "Convert"))
            .binding(KeyBinding::new("Tab/Shift+Tab", "Cycle"))
            .binding(KeyBinding::new("Esc", "Exit"))
    }

    /// Debug view controls
    pub fn debug_view() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("Enter", "Execute"))
            .binding(KeyBinding::new("Tab/Shift+Tab", "Cycle"))
            .binding(KeyBinding::new("Esc", "Exit"))
    }

    /// Tag search controls
    pub fn tag_search() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("↑↓←→", "Navigate"))
            .binding(KeyBinding::new("Tab", "Autocomplete/Cycle"))
            .binding(KeyBinding::new("Enter", "Search/Select"))
            .binding(KeyBinding::new("Esc", "Back"))
    }

    /// Empty/loading controls
    pub fn empty() -> ControlsHint {
        ControlsHint::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_controls_hint_line() {
        let hint = ControlsHint::new()
            .binding(KeyBinding::new("A", "Action"))
            .binding(KeyBinding::new("B", "Other"));

        let line = hint.render_line();
        // Just verify it produces spans without panicking
        assert!(!line.spans.is_empty());
    }
}
