//! Layout Widgets
//!
//! Multi-pane layout primitives with focus management.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders},
};

/// Configuration for a single pane in a multi-pane layout.
#[derive(Clone)]
pub struct PaneConfig {
    /// Title displayed in the pane's border
    pub title: String,
    /// Percentage width (for horizontal) or height (for vertical) of this pane
    pub size_percent: u16,
    /// Whether this pane currently has focus
    pub focused: bool,
}

impl PaneConfig {
    pub fn new(title: impl Into<String>, size_percent: u16) -> Self {
        Self {
            title: title.into(),
            size_percent,
            focused: false,
        }
    }

    pub fn focused(mut self) -> Self {
        self.focused = true;
        self
    }
}

/// A pane with its computed area and block styling.
pub struct FocusablePane {
    /// The computed area for this pane
    pub area: Rect,
    /// Pre-configured block with appropriate border styling
    pub block: Block<'static>,
    /// Whether this pane has focus
    pub focused: bool,
}

impl FocusablePane {
    /// Get the inner area (inside borders) for rendering content
    pub fn inner(&self) -> Rect {
        self.block.inner(self.area)
    }
}

/// Style configuration for pane borders
#[derive(Clone, Copy)]
pub struct PaneStyle {
    /// Border color when focused
    pub focused_border: Color,
    /// Border color when not focused
    pub unfocused_border: Color,
    /// Background color (optional)
    pub background: Option<Color>,
}

impl Default for PaneStyle {
    fn default() -> Self {
        Self {
            focused_border: Color::Yellow,
            unfocused_border: Color::White,
            background: None,
        }
    }
}

/// Two-pane horizontal layout with focus management.
///
/// # Example
/// ```ignore
/// let layout = TwoPaneLayout::horizontal()
///     .left(PaneConfig::new("Items", 40).focused())
///     .right(PaneConfig::new("Details", 60))
///     .build(area);
///
/// // Render into panes
/// f.render_widget(list, layout.left.inner());
/// f.render_widget(layout.left.block.clone(), layout.left.area);
/// ```
pub struct TwoPaneLayout {
    pub left: FocusablePane,
    pub right: FocusablePane,
}

/// Builder for TwoPaneLayout
pub struct TwoPaneLayoutBuilder {
    direction: Direction,
    left_config: Option<PaneConfig>,
    right_config: Option<PaneConfig>,
    style: PaneStyle,
}

impl TwoPaneLayoutBuilder {
    fn new(direction: Direction) -> Self {
        Self {
            direction,
            left_config: None,
            right_config: None,
            style: PaneStyle::default(),
        }
    }

    /// Configure the left (or top) pane
    pub fn left(mut self, config: PaneConfig) -> Self {
        self.left_config = Some(config);
        self
    }

    /// Configure the right (or bottom) pane
    pub fn right(mut self, config: PaneConfig) -> Self {
        self.right_config = Some(config);
        self
    }

    /// Set custom border styling
    pub fn style(mut self, style: PaneStyle) -> Self {
        self.style = style;
        self
    }

    /// Build the layout for the given area
    pub fn build(self, area: Rect) -> TwoPaneLayout {
        let style = self.style;
        let left = self.left_config.unwrap_or_else(|| PaneConfig::new("", 50));
        let right = self.right_config.unwrap_or_else(|| PaneConfig::new("", 50));

        let chunks = Layout::default()
            .direction(self.direction)
            .constraints([
                Constraint::Percentage(left.size_percent),
                Constraint::Percentage(right.size_percent),
            ])
            .split(area);

        TwoPaneLayout {
            left: make_pane(chunks[0], &left, &style),
            right: make_pane(chunks[1], &right, &style),
        }
    }
}

/// Helper to create a FocusablePane from config
fn make_pane(area: Rect, config: &PaneConfig, style: &PaneStyle) -> FocusablePane {
    let border_color = if config.focused {
        style.focused_border
    } else {
        style.unfocused_border
    };

    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(config.title.clone());

    if let Some(bg) = style.background {
        block = block.style(Style::default().bg(bg));
    }

    FocusablePane {
        area,
        block,
        focused: config.focused,
    }
}

impl TwoPaneLayout {
    /// Create a horizontal two-pane layout (left | right)
    pub fn horizontal() -> TwoPaneLayoutBuilder {
        TwoPaneLayoutBuilder::new(Direction::Horizontal)
    }

    /// Create a vertical two-pane layout (top / bottom)
    pub fn vertical() -> TwoPaneLayoutBuilder {
        TwoPaneLayoutBuilder::new(Direction::Vertical)
    }
}

/// Three-pane horizontal layout with focus management.
///
/// # Example
/// ```ignore
/// let layout = ThreePaneLayout::horizontal()
///     .left(PaneConfig::new("Tracks", 25))
///     .middle(PaneConfig::new("Tags", 45).focused())
///     .right(PaneConfig::new("Actions", 30))
///     .build(area);
/// ```
pub struct ThreePaneLayout {
    pub left: FocusablePane,
    pub middle: FocusablePane,
    pub right: FocusablePane,
}

/// Builder for ThreePaneLayout
pub struct ThreePaneLayoutBuilder {
    direction: Direction,
    left_config: Option<PaneConfig>,
    middle_config: Option<PaneConfig>,
    right_config: Option<PaneConfig>,
    style: PaneStyle,
}

impl ThreePaneLayoutBuilder {
    fn new(direction: Direction) -> Self {
        Self {
            direction,
            left_config: None,
            middle_config: None,
            right_config: None,
            style: PaneStyle::default(),
        }
    }

    /// Configure the left pane
    pub fn left(mut self, config: PaneConfig) -> Self {
        self.left_config = Some(config);
        self
    }

    /// Configure the middle pane
    pub fn middle(mut self, config: PaneConfig) -> Self {
        self.middle_config = Some(config);
        self
    }

    /// Configure the right pane
    pub fn right(mut self, config: PaneConfig) -> Self {
        self.right_config = Some(config);
        self
    }

    /// Set custom border styling
    pub fn style(mut self, style: PaneStyle) -> Self {
        self.style = style;
        self
    }

    /// Build the layout for the given area
    pub fn build(self, area: Rect) -> ThreePaneLayout {
        let style = self.style;
        let left = self.left_config.unwrap_or_else(|| PaneConfig::new("", 33));
        let middle = self
            .middle_config
            .unwrap_or_else(|| PaneConfig::new("", 34));
        let right = self.right_config.unwrap_or_else(|| PaneConfig::new("", 33));

        let chunks = Layout::default()
            .direction(self.direction)
            .constraints([
                Constraint::Percentage(left.size_percent),
                Constraint::Percentage(middle.size_percent),
                Constraint::Percentage(right.size_percent),
            ])
            .split(area);

        ThreePaneLayout {
            left: make_pane(chunks[0], &left, &style),
            middle: make_pane(chunks[1], &middle, &style),
            right: make_pane(chunks[2], &right, &style),
        }
    }
}

impl ThreePaneLayout {
    /// Create a horizontal three-pane layout (left | middle | right)
    pub fn horizontal() -> ThreePaneLayoutBuilder {
        ThreePaneLayoutBuilder::new(Direction::Horizontal)
    }

    /// Create a vertical three-pane layout (top / middle / bottom)
    pub fn vertical() -> ThreePaneLayoutBuilder {
        ThreePaneLayoutBuilder::new(Direction::Vertical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_two_pane_horizontal() {
        let area = Rect::new(0, 0, 100, 50);
        let layout = TwoPaneLayout::horizontal()
            .left(PaneConfig::new("Left", 40).focused())
            .right(PaneConfig::new("Right", 60))
            .build(area);

        assert!(layout.left.focused);
        assert!(!layout.right.focused);
        assert_eq!(layout.left.area.width, 40);
        assert_eq!(layout.right.area.width, 60);
    }

    #[test]
    fn test_three_pane_horizontal() {
        let area = Rect::new(0, 0, 100, 50);
        let layout = ThreePaneLayout::horizontal()
            .left(PaneConfig::new("A", 25))
            .middle(PaneConfig::new("B", 50).focused())
            .right(PaneConfig::new("C", 25))
            .build(area);

        assert!(!layout.left.focused);
        assert!(layout.middle.focused);
        assert!(!layout.right.focused);
    }
}
