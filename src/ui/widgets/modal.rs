//! Modal Dialog Widgets
//!
//! Centered popup dialogs for confirmations, previews, and information display.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Compute a centered rectangle within an area (percentage-based).
///
/// This is the foundation for modal positioning.
pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Compute a centered rectangle with fixed dimensions (in characters).
///
/// Use this for modals with static content that shouldn't shrink.
pub fn centered_rect_fixed(width: u16, height: u16, area: Rect) -> Rect {
    // Clamp to available area
    let width = width.min(area.width);
    let height = height.min(area.height);

    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;

    Rect::new(x, y, width, height)
}

/// Style configuration for modals
#[derive(Clone, Copy)]
pub struct ModalStyle {
    /// Border color
    pub border_color: Color,
    /// Background color
    pub background: Color,
    /// Title style
    pub title_style: Style,
}

impl Default for ModalStyle {
    fn default() -> Self {
        Self {
            border_color: Color::Yellow,
            background: Color::Black,
            title_style: Style::default().fg(Color::Yellow),
        }
    }
}

impl ModalStyle {
    pub fn warning() -> Self {
        Self {
            border_color: Color::Yellow,
            background: Color::Black,
            title_style: Style::default().fg(Color::Yellow),
        }
    }

    pub fn info() -> Self {
        Self {
            border_color: Color::Cyan,
            background: Color::Black,
            title_style: Style::default().fg(Color::Cyan),
        }
    }
}

/// Sizing mode for modals
#[derive(Clone, Copy)]
enum ModalSizing {
    /// Percentage of parent area
    Percent { width: u16, height: u16 },
    /// Fixed character dimensions (won't shrink with small windows)
    Fixed { width: u16, height: u16 },
}

/// A basic modal dialog that renders content in a centered popup.
///
/// # Example
/// ```ignore
/// Modal::new()
///     .title("Confirmation")
///     .size(50, 30)  // 50% width, 30% height
///     .content(vec![
///         Line::from("Are you sure?"),
///         Line::from(""),
///         Line::from("Press Y to confirm, N to cancel"),
///     ])
///     .render(f, area);
///
/// // Or use fixed sizing for static content:
/// Modal::new()
///     .title("Exit")
///     .fixed_size(40, 12)  // 40 chars wide, 12 lines tall
///     .content(...)
///     .render(f, area);
/// ```
pub struct Modal<'a> {
    title: String,
    content: Vec<Line<'a>>,
    sizing: ModalSizing,
    style: ModalStyle,
    alignment: Alignment,
}

impl<'a> Default for Modal<'a> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> Modal<'a> {
    pub fn new() -> Self {
        Self {
            title: String::new(),
            content: Vec::new(),
            sizing: ModalSizing::Percent { width: 50, height: 30 },
            style: ModalStyle::default(),
            alignment: Alignment::Left,
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn content(mut self, lines: Vec<Line<'a>>) -> Self {
        self.content = lines;
        self
    }

    /// Set size as fixed character dimensions (static sizing).
    /// Use this for modals with static content that shouldn't shrink.
    pub fn fixed_size(mut self, width: u16, height: u16) -> Self {
        self.sizing = ModalSizing::Fixed { width, height };
        self
    }

    pub fn style(mut self, style: ModalStyle) -> Self {
        self.style = style;
        self
    }

    pub fn centered(mut self) -> Self {
        self.alignment = Alignment::Center;
        self
    }

    /// Render the modal to the frame
    pub fn render(self, f: &mut Frame, area: Rect) {
        let popup_area = match self.sizing {
            ModalSizing::Percent { width, height } => centered_rect(width, height, area),
            ModalSizing::Fixed { width, height } => centered_rect_fixed(width, height, area),
        };

        // Clear the area behind the modal
        f.render_widget(Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.style.border_color))
            .title(self.title)
            .title_style(self.style.title_style)
            .style(Style::default().bg(self.style.background));

        let paragraph = Paragraph::new(self.content)
            .block(block)
            .alignment(self.alignment);

        f.render_widget(paragraph, popup_area);
    }
}

/// A button in a modal dialog
#[derive(Clone)]
pub struct ModalButton {
    pub label: String,
    pub key_hint: String,
    pub is_selected: bool,
    style_selected: Style,
    style_unselected: Style,
    /// Show selection indicator ("> ")
    show_indicator: bool,
}

impl ModalButton {
    pub fn new(label: impl Into<String>, key_hint: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            key_hint: key_hint.into(),
            is_selected: false,
            style_selected: Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD),
            style_unselected: Style::default().fg(Color::Gray),
            show_indicator: false,
        }
    }

    pub fn selected(mut self) -> Self {
        self.is_selected = true;
        self
    }

    /// Configure both selected and unselected styles at once
    pub fn styles(mut self, selected: Style, unselected: Style) -> Self {
        self.style_selected = selected;
        self.style_unselected = unselected;
        self
    }

    /// Enable selection indicator ("> " prefix when selected)
    pub fn with_indicator(mut self) -> Self {
        self.show_indicator = true;
        self
    }

    /// Get the current style based on selection state
    pub fn current_style(&self) -> Style {
        if self.is_selected {
            self.style_selected
        } else {
            self.style_unselected
        }
    }

    /// Render as a span (for inline button rendering)
    pub fn render_span(&self) -> Span<'static> {
        let text = if self.key_hint.is_empty() {
            format!("[ {} ]", self.label)
        } else {
            format!("[{}] {}", self.key_hint, self.label)
        };
        Span::styled(text, self.current_style())
    }

    /// Render as a span with indicator prefix
    pub fn render_with_indicator(&self) -> Vec<Span<'static>> {
        let indicator = if self.show_indicator {
            if self.is_selected { " > " } else { "   " }
        } else {
            ""
        };
        vec![
            Span::styled(indicator.to_string(), self.current_style()),
            self.render_span(),
        ]
    }
}
