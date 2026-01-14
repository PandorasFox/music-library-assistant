//! Controls Hint Widget
//!
//! Consistent rendering of keyboard controls and status information.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
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

impl ControlsStyle {
    /// Compact style with minimal separators
    pub fn compact() -> Self {
        Self {
            key_style: Style::default().fg(Color::Cyan),
            action_style: Style::default().fg(Color::DarkGray),
            separator: ":",
            binding_separator: "  ",
        }
    }

    /// Prominent style for important controls
    pub fn prominent() -> Self {
        Self {
            key_style: Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
            action_style: Style::default().fg(Color::White),
            separator: " = ",
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
    title: Option<String>,
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
            title: None,
        }
    }

    /// Add a key binding
    pub fn binding(mut self, binding: KeyBinding) -> Self {
        self.bindings.push(binding);
        self
    }

    /// Add multiple bindings at once
    pub fn bindings(mut self, bindings: impl IntoIterator<Item = KeyBinding>) -> Self {
        self.bindings.extend(bindings);
        self
    }

    /// Set the display style
    pub fn style(mut self, style: ControlsStyle) -> Self {
        self.style = style;
        self
    }

    /// Set an optional title for block rendering
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
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

    /// Render as a string (for embedding in other content)
    pub fn render_string(&self) -> String {
        self.bindings
            .iter()
            .map(|b| format!("{}{}{}", b.key, self.style.separator, b.action))
            .collect::<Vec<_>>()
            .join(self.style.binding_separator)
    }

    /// Render as a Paragraph widget with optional block
    pub fn render_paragraph(&self) -> Paragraph<'static> {
        let line = self.render_line();
        let para = Paragraph::new(vec![line]);

        if let Some(ref title) = self.title {
            para.block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title.clone()),
            )
        } else {
            para
        }
    }

    /// Render as multiple lines (for vertical layout)
    pub fn render_lines(&self) -> Vec<Line<'static>> {
        self.bindings
            .iter()
            .map(|binding| {
                Line::from(vec![
                    Span::styled(binding.key.clone(), self.style.key_style),
                    Span::styled(self.style.separator, self.style.action_style),
                    Span::styled(binding.action.clone(), self.style.action_style),
                ])
            })
            .collect()
    }
}

/// Helper to quickly build common control sets
pub mod presets {
    use super::{ControlsHint, KeyBinding};

    /// Navigation controls for list views
    pub fn list_navigation() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("Up/Down", "Navigate"))
            .binding(KeyBinding::new("Enter", "Select"))
            .binding(KeyBinding::new("Esc", "Back"))
    }

    /// Navigation for three-pane layouts
    pub fn pane_navigation() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("Left/Right", "Switch Pane"))
            .binding(KeyBinding::new("Up/Down", "Navigate"))
            .binding(KeyBinding::new("Enter", "Select"))
    }

    /// Tag editor workflow controls
    pub fn tag_editor_workflow() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("Shift+Up/Down", "Prev/Next Track"))
            .binding(KeyBinding::new("Tab", "Next Group"))
            .binding(KeyBinding::new("Up/Down", "Navigate Fields"))
            .binding(KeyBinding::new("Enter", "Edit"))
            .binding(KeyBinding::new("Esc", "Review"))
    }

    /// Tag editor standard controls (non-workflow)
    pub fn tag_editor_standard() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("Tab/Shift+Tab", "Prev/Next Track"))
            .binding(KeyBinding::new("Up/Down", "Navigate Fields"))
            .binding(KeyBinding::new("Enter", "Edit"))
            .binding(KeyBinding::new("Right", "Action Pane"))
            .binding(KeyBinding::new("Esc", "Exit"))
    }

    /// Modal confirmation controls
    pub fn modal_confirm() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("Y", "Confirm"))
            .binding(KeyBinding::new("N", "Cancel"))
    }

    /// Scrollable content controls
    pub fn scrollable() -> ControlsHint {
        ControlsHint::new()
            .binding(KeyBinding::new("Up/Down", "Scroll"))
            .binding(KeyBinding::new("PgUp/PgDn", "Page"))
            .binding(KeyBinding::new("Enter", "Confirm"))
            .binding(KeyBinding::new("Esc", "Cancel"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_controls_hint_string() {
        let hint = ControlsHint::new()
            .binding(KeyBinding::new("A", "Action"))
            .binding(KeyBinding::new("B", "Other"));

        let s = hint.render_string();
        assert!(s.contains("A"));
        assert!(s.contains("Action"));
        assert!(s.contains("B"));
        assert!(s.contains("Other"));
    }

    #[test]
    fn test_preset_list_navigation() {
        let hint = presets::list_navigation();
        let s = hint.render_string();
        assert!(s.contains("Up/Down"));
        assert!(s.contains("Navigate"));
    }
}
