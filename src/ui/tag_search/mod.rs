//! Tag Search Module
//!
//! Provides tag-based searching of the corpus with a query builder UI.
//!
//! ## Features
//!
//! - Query builder with tag name/value conditions
//! - Logical operators (AND/OR/XOR/NOT) for combining conditions
//! - Tab-completion for tag names
//! - Search results view with track list and info pane
//! - Integration with tag editor (Enter = single track, Shift+Enter = all results)

mod state;
mod types;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

// TODO: Re-enable when corpus::deploy is available
// use crate::corpus::deploy::compute_deployment_path_with_tags;
use crate::ui::widgets::{LateralView, UnifiedTitleBar};

pub use state::TagSearchState;
pub use types::{
    ComparisonOperator, FileTypeCategory, SearchCondition, TagSearchAction, TagSearchMode,
};

impl TagSearchState {
    /// Handle a key event. Returns an action that may require db access.
    pub fn handle_key(&mut self, key: KeyEvent) -> TagSearchAction {
        // Handle modal first if active
        if self.modal.is_some() {
            return self.handle_modal_key(key);
        }

        match self.mode {
            TagSearchMode::QueryBuilder => self.handle_query_builder_key(key),
            TagSearchMode::Results => self.handle_results_mode_key(key),
        }
    }

    fn handle_modal_key(&mut self, key: KeyEvent) -> TagSearchAction {
        // GatheringTags modal is non-interactive - handled by tick
        if matches!(self.modal, Some(types::TagSearchModal::GatheringTags)) {
            return TagSearchAction::None;
        }

        match key.code {
            // Enter or Escape dismisses the modal
            KeyCode::Enter | KeyCode::Esc => {
                self.modal = None;
                TagSearchAction::None
            }
            _ => TagSearchAction::None,
        }
    }

    fn handle_query_builder_key(&mut self, key: KeyEvent) -> TagSearchAction {
        match key.code {
            // Tab/Shift-Tab for lateral view cycling (no tab-completion to avoid conflict)
            KeyCode::Tab if !key.modifiers.contains(KeyModifiers::SHIFT) => TagSearchAction::CycleNext,
            KeyCode::Tab | KeyCode::BackTab => TagSearchAction::CyclePrev,

            // Escape
            KeyCode::Esc => TagSearchAction::Cancel,

            // Navigate between conditions and fields
            KeyCode::Up => {
                self.move_focus_up();
                TagSearchAction::None
            }
            KeyCode::Down => {
                self.move_focus_down();
                TagSearchAction::None
            }
            KeyCode::Left => {
                self.move_focus_left();
                TagSearchAction::None
            }
            KeyCode::Right => {
                self.move_focus_right();
                TagSearchAction::None
            }

            // Enter to execute search or add condition
            KeyCode::Enter => {
                if self.is_on_search_button() {
                    // Return action to execute search (db access happens in action handler)
                    TagSearchAction::ExecuteSearch
                } else if self.is_on_add_condition() {
                    self.add_condition();
                    TagSearchAction::None
                } else if self.is_on_operator_field() {
                    self.cycle_operator();
                    TagSearchAction::None
                } else if self.is_on_comparison_field() {
                    self.cycle_comparison();
                    TagSearchAction::None
                } else {
                    // Move to next field
                    self.move_focus_right();
                    TagSearchAction::None
                }
            }

            // Character input
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert_char(c);
                TagSearchAction::None
            }
            KeyCode::Backspace => {
                self.backspace();
                TagSearchAction::None
            }
            KeyCode::Delete => {
                self.delete();
                TagSearchAction::None
            }

            _ => TagSearchAction::None,
        }
    }

    fn handle_results_mode_key(&mut self, key: KeyEvent) -> TagSearchAction {
        match key.code {
            // Tab/Shift-Tab for lateral view cycling
            KeyCode::Tab if !key.modifiers.contains(KeyModifiers::SHIFT) => TagSearchAction::CycleNext,
            KeyCode::Tab | KeyCode::BackTab => TagSearchAction::CyclePrev,

            // Escape returns to query builder
            KeyCode::Esc => {
                self.mode = TagSearchMode::QueryBuilder;
                TagSearchAction::None
            }

            // Navigate results
            KeyCode::Up | KeyCode::Char('k') => {
                self.results_select_prev();
                TagSearchAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.results_select_next();
                TagSearchAction::None
            }

            // B = bulk edit all results (show gathering modal first)
            KeyCode::Char('b') | KeyCode::Char('B') => {
                let tracks = self.all_result_tracks();
                if !tracks.is_empty() {
                    // Show gathering modal and store pending tracks
                    self.modal = Some(types::TagSearchModal::GatheringTags);
                    self.pending_bulk_edit = Some(tracks);
                    TagSearchAction::None
                } else {
                    TagSearchAction::None
                }
            }

            // Enter = edit single track
            KeyCode::Enter => {
                if let Some(twt) = self.selected_result() {
                    TagSearchAction::EditTrack(twt.track.clone())
                } else {
                    TagSearchAction::None
                }
            }

            _ => TagSearchAction::None,
        }
    }

    /// Render the tag search view.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        // Layout: Title bar | Content
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(UnifiedTitleBar::height()),
                Constraint::Min(5),
            ])
            .split(area);

        // Title bar
        let titlebar = UnifiedTitleBar::new(LateralView::TagSearch);
        titlebar.render(f, chunks[0]);

        // Content based on mode
        match self.mode {
            TagSearchMode::QueryBuilder => self.render_query_builder(f, chunks[1]),
            TagSearchMode::Results => self.render_results(f, chunks[1]),
        }

        // Render modal overlay if active
        if let Some(ref modal) = self.modal {
            self.render_modal(f, area, modal);
        }
    }

    fn render_modal(&self, f: &mut Frame, area: Rect, modal: &types::TagSearchModal) {
        use crate::ui::widgets::Modal;

        match modal {
            types::TagSearchModal::NoResults => {
                Modal::new()
                    .title("Search Results")
                    .fixed_size(32, 8)
                    .content(vec![
                        Line::raw(""),
                        Line::styled("No results found", Style::default().fg(Color::White)),
                        Line::raw(""),
                        Line::styled("[ OK ]", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    ])
                    .centered()
                    .render(f, area);
            }
            types::TagSearchModal::GatheringTags => {
                let track_count = self.pending_bulk_edit.as_ref().map(|t| t.len()).unwrap_or(0);
                Modal::new()
                    .title("Bulk Edit")
                    .fixed_size(42, 7)
                    .content(vec![
                        Line::raw(""),
                        Line::styled(
                            format!("Gathering tags for {} files...", track_count),
                            Style::default().fg(Color::Yellow),
                        ),
                        Line::raw(""),
                    ])
                    .centered()
                    .render(f, area);
            }
        }
    }

    fn render_query_builder(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title("Tag Query Builder");

        let inner = block.inner(area);
        f.render_widget(block, area);

        // Layout conditions vertically
        let mut y = inner.y;

        for (idx, condition) in self.conditions.iter().enumerate() {
            if y >= inner.y + inner.height - 3 {
                break; // Not enough space
            }

            let is_focused = self.focused_condition == idx;
            let line_height = 1;

            // Render condition row
            let row_area = Rect::new(inner.x, y, inner.width, line_height);
            self.render_condition_row(f, row_area, idx, condition, is_focused);

            y += line_height + 1;
        }

        // Add condition button
        if y < inner.y + inner.height - 2 {
            let add_style = if self.is_on_add_condition() {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let add_line = Line::from(Span::styled("[+ Add another condition]", add_style));
            f.render_widget(Paragraph::new(add_line), Rect::new(inner.x + 2, y, inner.width - 2, 1));
            y += 2;
        }

        // Search button
        if y < inner.y + inner.height {
            let search_style = if self.is_on_search_button() {
                Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let search_line = Line::from(Span::styled("[ Search ]", search_style));
            let search_x = inner.x + (inner.width.saturating_sub(12)) / 2;
            f.render_widget(Paragraph::new(search_line), Rect::new(search_x, y, 12, 1));
        }
    }

    fn render_condition_row(&self, f: &mut Frame, area: Rect, idx: usize, condition: &SearchCondition, is_focused: bool) {
        let mut spans = Vec::new();

        // Operator (for non-first conditions)
        if idx > 0 {
            let op_focused = is_focused && self.field_focus == QueryFieldFocus::Operator;
            let op_style = if op_focused {
                Style::default().bg(Color::Yellow).fg(Color::Black)
            } else {
                Style::default().fg(Color::Yellow)
            };
            spans.push(Span::styled(format!("{:<4}", condition.operator.label()), op_style));
        } else {
            spans.push(Span::raw("    ")); // Align with operators (4 chars: "AND ")
        }

        // Tag name field
        let name_focused = is_focused && self.field_focus == QueryFieldFocus::TagName;
        let name_style = if name_focused {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        } else {
            Style::default().fg(Color::White)
        };
        let name_display = if condition.tag_name.is_empty() {
            "<tag name>".to_string()
        } else {
            condition.tag_name.clone()
        };
        // Show cursor for focused text input
        let name_with_cursor = if name_focused {
            format!("{}_", name_display)
        } else {
            name_display
        };
        spans.push(Span::styled(format!("{:<16}", name_with_cursor), name_style));
        spans.push(Span::raw(" "));

        // Comparison operator
        let comp_focused = is_focused && self.field_focus == QueryFieldFocus::Comparison;
        let comp_style = if comp_focused {
            Style::default().bg(Color::Magenta).fg(Color::Black)
        } else {
            Style::default().fg(Color::Magenta)
        };
        spans.push(Span::styled(format!("{:<8}", condition.comparison.label()), comp_style));
        spans.push(Span::raw(" "));

        // Value field
        let value_focused = is_focused && self.field_focus == QueryFieldFocus::Value;
        let value_style = if value_focused {
            Style::default().bg(Color::DarkGray).fg(Color::White)
        } else {
            Style::default().fg(Color::White)
        };
        let value_display = if condition.value.is_empty() {
            "<value>".to_string()
        } else {
            condition.value.clone()
        };
        // Show cursor for focused text input
        let value_with_cursor = if value_focused {
            format!("{}_", value_display)
        } else {
            value_display
        };
        spans.push(Span::styled(value_with_cursor, value_style));

        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_results(&self, f: &mut Frame, area: Rect) {
        // Two-pane layout: Results (60%) | Info (40%)
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);

        self.render_results_list(f, chunks[0]);
        self.render_results_info(f, chunks[1]);
    }

    fn render_results_list(&self, f: &mut Frame, area: Rect) {
        let title = format!("Results ({} tracks)", self.results.len());
        let block = Block::default().borders(Borders::ALL).title(title);
        let inner = block.inner(area);
        f.render_widget(block, area);

        // Render track list sorted by path
        // TODO: Re-enable deployment path display when corpus::deploy is available
        let visible_height = inner.height as usize;
        let scroll = self.results_scroll;

        let lines: Vec<Line> = self.results
            .iter()
            .enumerate()
            .skip(scroll)
            .take(visible_height)
            .map(|(idx, twt)| {
                // Use corpus path directly (deployment path display disabled)
                let path_str = &twt.track.path;
                let is_selected = idx == self.results_selected;

                let indicator = if is_selected { "▶ " } else { "  " };
                let style = if is_selected {
                    Style::default().bg(Color::DarkGray).fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                Line::styled(format!("{}{}", indicator, path_str), style)
            })
            .collect();

        f.render_widget(Paragraph::new(lines), inner);
    }

    fn render_results_info(&self, f: &mut Frame, area: Rect) {
        let block = Block::default().borders(Borders::ALL).title("Track Info");
        let inner = block.inner(area);
        f.render_widget(block, area);

        let lines: Vec<Line> = if let Some(twt) = self.selected_result() {
            vec![
                Line::from(vec![
                    Span::styled("Title: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(twt.get_tag("title").unwrap_or("-")),
                ]),
                Line::from(vec![
                    Span::styled("Artist: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(twt.get_tag("artist").unwrap_or("-")),
                ]),
                Line::from(vec![
                    Span::styled("Album: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(twt.get_tag("album").unwrap_or("-")),
                ]),
                Line::from(vec![
                    Span::styled("Album Artist: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(twt.get_tag("album_artist").unwrap_or("-")),
                ]),
                Line::from(vec![
                    Span::styled("Genre: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(twt.get_tag("genre").unwrap_or("-")),
                ]),
                Line::raw(""),
                Line::from(vec![
                    Span::styled("Path: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(&twt.track.path),
                ]),
                Line::raw(""),
                Line::styled(
                    "Enter: Edit track | B: Bulk edit all",
                    Style::default().fg(Color::DarkGray),
                ),
            ]
        } else {
            vec![Line::styled("No track selected", Style::default().fg(Color::DarkGray))]
        };

        f.render_widget(Paragraph::new(lines), inner);
    }
}

/// Focus within query builder
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueryFieldFocus {
    Operator,
    #[default]
    TagName,
    Comparison,
    Value,
    AddCondition,
    SearchButton,
}
