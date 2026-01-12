//! Rendering for the directory browser.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::state::DirBrowserState;

impl DirBrowserState {
    /// Render the directory browser.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Layout: header | tree | footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header with title
                Constraint::Min(5),    // Tree view
                Constraint::Length(3), // Footer with help
            ])
            .split(area);

        self.render_header(f, chunks[0]);
        self.render_tree(f, chunks[1]);
        self.render_footer(f, chunks[2]);
    }

    /// Render the header with title and warning if too many selections.
    fn render_header(&self, f: &mut Frame, area: Rect) {
        let selected_count = self.selected_paths().len();

        // Calculate warning level: ⚠ for each 5 dirs over threshold
        let warning_count = if selected_count > 5 {
            (selected_count - 1) / 5 // 6-10 = 1, 11-15 = 2, etc.
        } else {
            0
        };

        let warning_str = "⚠".repeat(warning_count);

        let title_text = if warning_count > 0 {
            format!(
                "{} {} ({} directories)",
                warning_str,
                self.config().title,
                selected_count
            )
        } else {
            self.config().title.clone()
        };

        let style = if warning_count > 0 {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };

        let title = Paragraph::new(title_text)
            .style(style)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(title, area);
    }

    /// Render the tree view.
    fn render_tree(&mut self, f: &mut Frame, area: Rect) {
        // Calculate visible height (accounting for borders)
        let inner_height = area.height.saturating_sub(2) as usize;
        self.set_visible_height(inner_height);

        // Build list items with tree formatting
        let items: Vec<ListItem> = self
            .entries()
            .iter()
            .enumerate()
            .skip(self.scroll_offset())
            .take(inner_height)
            .map(|(idx, entry)| {
                let is_cursor = idx == self.cursor_idx();
                let is_selected = self.is_selected(&entry.path);
                let is_greyed = self.is_ancestor_of_selected(&entry.path);

                // Build the line with proper indentation
                let mut spans = Vec::new();

                // Selection marker
                let select_marker = if is_greyed {
                    "[-] " // Indicate unselectable (ancestor of selected)
                } else if is_selected {
                    "[x] "
                } else {
                    "[ ] "
                };
                spans.push(Span::styled(
                    select_marker,
                    if is_greyed {
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)
                    } else if is_selected {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ));

                // Indentation
                for _ in 0..entry.depth {
                    spans.push(Span::raw("  "));
                }

                // Expand/collapse indicator
                let indicator = if entry.has_children {
                    if entry.is_expanded {
                        "▼ "
                    } else {
                        "▶ "
                    }
                } else {
                    "  "
                };
                spans.push(Span::styled(
                    indicator,
                    if is_greyed {
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)
                    } else {
                        Style::default().fg(Color::Yellow)
                    },
                ));

                // Directory name
                let name_style = if is_greyed {
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM)
                } else if is_cursor {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                spans.push(Span::styled(entry.name.clone(), name_style));

                // Item count (if enabled and > 0)
                if self.config().show_item_counts && entry.item_count > 0 {
                    spans.push(Span::styled(
                        format!("  ({} tracks)", entry.item_count),
                        Style::default().fg(Color::DarkGray),
                    ));
                }

                let line = Line::from(spans);
                let mut item = ListItem::new(line);

                if is_cursor {
                    item = item.style(Style::default().bg(Color::DarkGray));
                }

                item
            })
            .collect();

        let selected_count = self.selected_paths().len();
        let list_title = if selected_count > 0 {
            format!(" {} selected ", selected_count)
        } else {
            String::new()
        };

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(list_title.as_str()),
        );

        f.render_widget(list, area);
    }

    /// Render the footer with keyboard help.
    fn render_footer(&self, f: &mut Frame, area: Rect) {
        let help_text = vec![
            Span::styled("↑↓", Style::default().fg(Color::Yellow)),
            Span::raw(" navigate  "),
            Span::styled("←→", Style::default().fg(Color::Yellow)),
            Span::raw(" expand/collapse  "),
            Span::styled("Space", Style::default().fg(Color::Yellow)),
            Span::raw(" toggle  "),
            Span::styled("Enter", Style::default().fg(Color::Green)),
            Span::raw(" proceed  "),
            Span::styled("Esc", Style::default().fg(Color::Red)),
            Span::raw(" cancel"),
        ];

        let help = Paragraph::new(Line::from(help_text))
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(help, area);
    }
}
