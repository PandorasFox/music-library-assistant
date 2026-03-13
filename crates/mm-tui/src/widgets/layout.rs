//! Layout Widgets
//!
//! Multi-pane layout primitives with focus management.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::Color,
};

/// Configuration for a single pane in a multi-pane layout.
#[derive(Clone)]
pub struct PaneConfig {
    /// Title displayed in the pane's border
    pub _title: String,
    /// Percentage width (for horizontal) or height (for vertical) of this pane
    pub size_percent: u16,
}

impl PaneConfig {
    pub fn new(title: impl Into<String>, size_percent: u16) -> Self {
        Self {
            _title: title.into(),
            size_percent,
        }
    }
}

/// A pane with its computed area.
pub struct FocusablePane {
    /// The computed area for this pane
    pub area: Rect,
}

/// Style configuration for pane borders
#[derive(Clone, Copy)]
pub struct PaneStyle {
    /// Border color when focused
    pub _focused_border: Color,
    /// Border color when not focused
    pub _unfocused_border: Color,
    /// Background color (optional)
    pub _background: Option<Color>,
}

impl Default for PaneStyle {
    fn default() -> Self {
        Self {
            _focused_border: Color::Yellow,
            _unfocused_border: Color::White,
            _background: None,
        }
    }
}

/// Helper to create a FocusablePane from config
fn make_pane(area: Rect, _config: &PaneConfig, _style: &PaneStyle) -> FocusablePane {
    FocusablePane { area }
}

/// Three-pane horizontal layout with focus management.
///
/// # Example
/// ```ignore
/// let layout = ThreePaneLayout::horizontal()
///     .left(PaneConfig::new("Tracks", 25))
///     .middle(PaneConfig::new("Tags", 45))
///     .right(PaneConfig::new("Actions", 30))
///     .build(area);
///
/// // Access computed areas
/// render_tracks(f, layout.left.area);
/// render_tags(f, layout.middle.area);
/// render_actions(f, layout.right.area);
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_three_pane_horizontal() {
        let area = Rect::new(0, 0, 100, 50);
        let layout = ThreePaneLayout::horizontal()
            .left(PaneConfig::new("A", 25))
            .middle(PaneConfig::new("B", 50))
            .right(PaneConfig::new("C", 25))
            .build(area);

        // Verify areas are computed
        assert!(layout.left.area.width > 0);
        assert!(layout.middle.area.width > 0);
        assert!(layout.right.area.width > 0);
    }
}
