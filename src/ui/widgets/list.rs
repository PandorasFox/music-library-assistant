//! Selectable List Widget
//!
//! A list widget with consistent selection indicators, highlighting, and scroll support.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, StatefulWidget, Widget},
    Frame,
};

/// Style configuration for selectable lists
#[derive(Clone)]
pub struct SelectableListStyle {
    /// Prefix shown for selected items (e.g., ">> ")
    pub selected_prefix: String,
    /// Prefix shown for unselected items (e.g., "   ")
    pub unselected_prefix: String,
    /// Style for selected items when pane is focused
    pub selected_focused_style: Style,
    /// Style for selected items when pane is not focused
    pub selected_unfocused_style: Style,
    /// Style for unselected items
    pub unselected_style: Style,
    /// Border color when focused
    pub focused_border: Color,
    /// Border color when not focused
    pub unfocused_border: Color,
}

impl Default for SelectableListStyle {
    fn default() -> Self {
        Self {
            selected_prefix: ">> ".to_string(),
            unselected_prefix: "   ".to_string(),
            selected_focused_style: Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
            selected_unfocused_style: Style::default().add_modifier(Modifier::BOLD),
            unselected_style: Style::default(),
            focused_border: Color::Yellow,
            unfocused_border: Color::White,
        }
    }
}

impl SelectableListStyle {
    /// Minimal style with just highlighting, no prefix
    pub fn minimal() -> Self {
        Self {
            selected_prefix: String::new(),
            unselected_prefix: String::new(),
            ..Default::default()
        }
    }

    /// Arrow-style selection indicator
    pub fn arrow() -> Self {
        Self {
            selected_prefix: "> ".to_string(),
            unselected_prefix: "  ".to_string(),
            ..Default::default()
        }
    }

    /// Bullet-style selection indicator
    pub fn bullet() -> Self {
        Self {
            selected_prefix: "* ".to_string(),
            unselected_prefix: "  ".to_string(),
            ..Default::default()
        }
    }
}

/// An item in a selectable list with optional extra styling
#[derive(Clone)]
pub struct SelectableItem {
    /// Main text of the item
    pub text: String,
    /// Additional spans to append (e.g., metadata, counts)
    pub suffix: Option<Vec<Span<'static>>>,
    /// Optional custom style override
    pub custom_style: Option<Style>,
}

impl SelectableItem {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            suffix: None,
            custom_style: None,
        }
    }

    pub fn with_suffix(mut self, spans: Vec<Span<'static>>) -> Self {
        self.suffix = Some(spans);
        self
    }

    pub fn with_style(mut self, style: Style) -> Self {
        self.custom_style = Some(style);
        self
    }
}

impl<T: Into<String>> From<T> for SelectableItem {
    fn from(s: T) -> Self {
        SelectableItem::new(s)
    }
}

/// State for a SelectableList, wrapping ListState with additional tracking
#[derive(Clone, Default)]
pub struct SelectableListState {
    /// The underlying ratatui list state
    pub list_state: ListState,
    /// Total number of items (for scroll indicator)
    pub total_items: usize,
    /// Whether this list is currently focused
    pub focused: bool,
}

impl SelectableListState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_selected(mut self, idx: Option<usize>) -> Self {
        self.list_state.select(idx);
        self
    }

    pub fn focused(mut self) -> Self {
        self.focused = true;
        self
    }

    pub fn selected(&self) -> Option<usize> {
        self.list_state.selected()
    }

    pub fn select(&mut self, idx: Option<usize>) {
        self.list_state.select(idx);
    }

    /// Move selection up, wrapping at top
    pub fn select_previous(&mut self) {
        if self.total_items == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let new = if current == 0 {
            self.total_items - 1
        } else {
            current - 1
        };
        self.list_state.select(Some(new));
    }

    /// Move selection down, wrapping at bottom
    pub fn select_next(&mut self) {
        if self.total_items == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let new = if current >= self.total_items - 1 {
            0
        } else {
            current + 1
        };
        self.list_state.select(Some(new));
    }

    /// Move selection up without wrapping
    pub fn select_previous_clamped(&mut self) {
        if self.total_items == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        if current > 0 {
            self.list_state.select(Some(current - 1));
        }
    }

    /// Move selection down without wrapping
    pub fn select_next_clamped(&mut self) -> bool {
        if self.total_items == 0 {
            return false;
        }
        let current = self.list_state.selected().unwrap_or(0);
        if current < self.total_items - 1 {
            self.list_state.select(Some(current + 1));
            false
        } else {
            true // At end
        }
    }
}

/// A selectable list widget with consistent styling.
///
/// # Example
/// ```ignore
/// let items = vec![
///     SelectableItem::new("First"),
///     SelectableItem::new("Second").with_suffix(vec![Span::raw(" (default)")]),
///     SelectableItem::new("Third"),
/// ];
///
/// let mut state = SelectableListState::new()
///     .with_selected(Some(0))
///     .focused();
/// state.total_items = items.len();
///
/// SelectableList::new(items)
///     .title("Options")
///     .render(f, area, &mut state);
/// ```
pub struct SelectableList {
    items: Vec<SelectableItem>,
    title: String,
    style: SelectableListStyle,
    show_scroll_indicator: bool,
}

impl SelectableList {
    pub fn new(items: Vec<SelectableItem>) -> Self {
        Self {
            items,
            title: String::new(),
            style: SelectableListStyle::default(),
            show_scroll_indicator: true,
        }
    }

    /// Create from simple strings
    pub fn from_strings(items: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::new(items.into_iter().map(SelectableItem::new).collect())
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn style(mut self, style: SelectableListStyle) -> Self {
        self.style = style;
        self
    }

    pub fn hide_scroll_indicator(mut self) -> Self {
        self.show_scroll_indicator = false;
        self
    }

    /// Render the list to the frame
    pub fn render(self, f: &mut Frame, area: Rect, state: &mut SelectableListState) {
        state.total_items = self.items.len();
        let selected_idx = state.list_state.selected();

        // Build list items with appropriate styling
        let list_items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let is_selected = selected_idx == Some(i);
                let prefix = if is_selected {
                    &self.style.selected_prefix
                } else {
                    &self.style.unselected_prefix
                };

                let base_style = if is_selected {
                    if state.focused {
                        self.style.selected_focused_style
                    } else {
                        self.style.selected_unfocused_style
                    }
                } else {
                    self.style.unselected_style
                };

                let final_style = item.custom_style.unwrap_or(base_style);

                let mut spans = vec![
                    Span::raw(prefix.clone()),
                    Span::styled(item.text.clone(), final_style),
                ];

                if let Some(ref suffix) = item.suffix {
                    spans.extend(suffix.clone());
                }

                ListItem::new(Line::from(spans))
            })
            .collect();

        // Build title with optional scroll indicator
        let title = if self.show_scroll_indicator && !self.items.is_empty() {
            let pos = selected_idx.map(|i| i + 1).unwrap_or(0);
            format!("{} [{}/{}]", self.title, pos, self.items.len())
        } else {
            self.title.clone()
        };

        let border_color = if state.focused {
            self.style.focused_border
        } else {
            self.style.unfocused_border
        };

        let list = List::new(list_items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(title),
        );

        f.render_stateful_widget(list, area, &mut state.list_state);
    }
}

/// A simplified list builder for common cases
pub struct SimpleList;

impl SimpleList {
    /// Render a simple string list with default styling
    pub fn render(
        f: &mut Frame,
        area: Rect,
        title: &str,
        items: &[String],
        selected: Option<usize>,
        focused: bool,
    ) {
        let mut state = SelectableListState::new().with_selected(selected);
        if focused {
            state = state.focused();
        }
        state.total_items = items.len();

        SelectableList::from_strings(items.iter().cloned())
            .title(title)
            .render(f, area, &mut state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_navigation() {
        let mut state = SelectableListState::new().with_selected(Some(0));
        state.total_items = 5;

        state.select_next();
        assert_eq!(state.selected(), Some(1));

        state.select_previous();
        assert_eq!(state.selected(), Some(0));

        // Test wrapping
        state.select_previous();
        assert_eq!(state.selected(), Some(4));

        state.select_next();
        assert_eq!(state.selected(), Some(0));
    }

    #[test]
    fn test_clamped_navigation() {
        let mut state = SelectableListState::new().with_selected(Some(0));
        state.total_items = 3;

        // Should not wrap
        state.select_previous_clamped();
        assert_eq!(state.selected(), Some(0));

        state.select_next_clamped();
        state.select_next_clamped();
        let at_end = state.select_next_clamped();
        assert!(at_end);
        assert_eq!(state.selected(), Some(2));
    }
}
