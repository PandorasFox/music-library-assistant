//! Tag Search Module
//!
//! Provides tag-based searching of the corpus with a query builder UI.
//!
//! The query builder conditions are managed by `mm_ui::search_widget::SearchWidget`.
//! Search execution fires `SearchWithConditions` server-side query instead of
//! loading all files and filtering locally.

mod state;
mod types;

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::helpers::render_pane;
use crate::input::InputAction;
use crate::widgets::standard_list::{render_standard_list, ListInputResult};

pub use state::TagSearchState;
pub use types::{
    SearchCondition, TagSearchAction, TagSearchMode,
};

use mm_ui::search_widget::{QueryFieldFocus, SearchAction};

impl TagSearchState {
    /// Handle a semantic input action. Returns a domain action for the
    /// action handler to dispatch, or None if consumed internally.
    pub fn handle_input(&mut self, action: &InputAction) -> Option<TagSearchAction> {
        // Handle modal first if active
        if self.modal.is_some() {
            return self.handle_modal_input(action);
        }

        // In results mode, handle StandardList for wizard/click support
        if self.widget.mode == TagSearchMode::Results {
            return self.handle_results_mode_input(action);
        }

        // Delegate to widget
        match self.widget.handle_input(action) {
            Some(SearchAction::ExecuteSearch) => Some(TagSearchAction::ExecuteSearch),
            Some(SearchAction::Cancel) => Some(TagSearchAction::Cancel),
            Some(SearchAction::BulkEdit) => {
                let inodes = self.all_result_inodes();
                if !inodes.is_empty() {
                    self.modal = Some(types::TagSearchModal::GatheringTags);
                    self.pending_bulk_edit = Some(inodes.clone());
                    Some(TagSearchAction::BulkEdit(inodes))
                } else {
                    None
                }
            }
            Some(SearchAction::SelectResult(idx)) => {
                self.results.get(idx).map(|r| TagSearchAction::EditAudioFile(r.result.inode))
            }
            Some(SearchAction::SwitchMode) | None => None,
        }
    }

    fn handle_modal_input(&mut self, action: &InputAction) -> Option<TagSearchAction> {
        // GatheringTags modal is non-interactive
        if matches!(self.modal, Some(types::TagSearchModal::GatheringTags)) {
            return None;
        }

        match action {
            InputAction::Confirm | InputAction::Cancel => {
                self.modal = None;
                None
            }
            _ => None,
        }
    }

    fn handle_results_mode_input(&mut self, action: &InputAction) -> Option<TagSearchAction> {
        // B for bulk edit before StandardList
        if matches!(action, InputAction::Char('b' | 'B')) {
            let inodes = self.all_result_inodes();
            if !inodes.is_empty() {
                self.modal = Some(types::TagSearchModal::GatheringTags);
                self.pending_bulk_edit = Some(inodes.clone());
                return Some(TagSearchAction::BulkEdit(inodes));
            }
            return None;
        }

        let result = self.widget.results_list.handle_input(action, &self.results);

        match result {
            ListInputResult::Consumed | ListInputResult::CursorMoved | ListInputResult::Toggled => {
                None
            }
            ListInputResult::Confirm(inode) => {
                Some(TagSearchAction::EditAudioFile(inode))
            }
            ListInputResult::Unhandled => match action {
                InputAction::Cancel => {
                    self.widget.mode = TagSearchMode::QueryBuilder;
                    None
                }
                _ => None,
            },
        }
    }

    /// Render the tag search view (titlebar is rendered by render_app).
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        match self.widget.mode {
            TagSearchMode::QueryBuilder => self.render_query_builder(f, area),
            TagSearchMode::Results => self.render_results(f, area),
        }

        if let Some(ref modal) = self.modal {
            self.render_modal(f, area, modal);
        }
    }

    fn render_modal(&self, f: &mut Frame, area: Rect, modal: &types::TagSearchModal) {
        use crate::widgets::Modal;

        match modal {
            types::TagSearchModal::NoResults => {
                Modal::new()
                    .title("Search Results")
                    .fixed_size(32, 8)
                    .content(vec![
                        Line::raw(""),
                        Line::styled("No results found", Style::default().fg(Color::White)),
                        Line::raw(""),
                        Line::styled(
                            "[ OK ]",
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ])
                    .centered()
                    .render(f, area);
            }
            types::TagSearchModal::GatheringTags => {
                let track_count = self
                    .pending_bulk_edit
                    .as_ref()
                    .map(|t| t.len())
                    .unwrap_or(0);
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

        let inner = render_pane(f, area, block);

        let mut y = inner.y;

        for (idx, condition) in self.widget.conditions.iter().enumerate() {
            if y >= inner.y + inner.height - 3 {
                break;
            }

            let is_focused = self.widget.focused_condition == idx;
            let line_height = 1;

            let row_area = Rect::new(inner.x, y, inner.width, line_height);
            self.render_condition_row(f, row_area, idx, condition, is_focused);

            y += line_height + 1;
        }

        // Add condition button
        if y < inner.y + inner.height - 2 {
            let add_style = if self.widget.is_on_add_condition() {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let add_line = Line::from(Span::styled("[+ Add another condition]", add_style));
            f.render_widget(
                Paragraph::new(add_line),
                Rect::new(inner.x + 2, y, inner.width - 2, 1),
            );
            y += 2;
        }

        // Search button
        if y < inner.y + inner.height {
            let search_style = if self.widget.is_on_search_button() {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let search_line = Line::from(Span::styled("[ Search ]", search_style));
            let search_x = inner.x + (inner.width.saturating_sub(12)) / 2;
            f.render_widget(Paragraph::new(search_line), Rect::new(search_x, y, 12, 1));
        }
    }

    fn render_condition_row(
        &self,
        f: &mut Frame,
        area: Rect,
        idx: usize,
        condition: &SearchCondition,
        is_focused: bool,
    ) {
        use types::ConditionType;

        let mut spans = Vec::new();

        // Operator (for non-first conditions)
        if idx > 0 {
            let op_focused = is_focused && self.widget.field_focus == QueryFieldFocus::Operator;
            let op_style = if op_focused {
                Style::default().bg(Color::Yellow).fg(Color::Black)
            } else {
                Style::default().fg(Color::Yellow)
            };
            spans.push(Span::styled(
                format!("{:<4}", condition.operator.label()),
                op_style,
            ));
        } else {
            spans.push(Span::raw("    "));
        }

        // Condition type selector
        let type_focused = is_focused && self.widget.field_focus == QueryFieldFocus::ConditionType;
        let type_style = if type_focused {
            Style::default().bg(Color::Blue).fg(Color::White)
        } else {
            Style::default().fg(Color::Blue)
        };
        spans.push(Span::styled(
            format!("[{:<4}]", condition.condition_type.label()),
            type_style,
        ));
        spans.push(Span::raw(" "));

        match condition.condition_type {
            ConditionType::Tag => {
                let name_focused = is_focused && self.widget.field_focus == QueryFieldFocus::TagName;
                let name_style = if name_focused {
                    Style::default().bg(Color::DarkGray).fg(Color::White)
                } else {
                    Style::default().fg(Color::White)
                };
                let name_display = if condition.tag_name.is_empty() {
                    "<tag>".to_string()
                } else {
                    condition.tag_name.value().to_string()
                };
                let name_with_cursor = if name_focused {
                    let (before, cursor_ch, after) = condition.tag_name.cursor_splits();
                    format!("{}{}{}", before, cursor_ch, after)
                } else {
                    name_display
                };
                spans.push(Span::styled(
                    format!("{:<12}", name_with_cursor),
                    name_style,
                ));
                spans.push(Span::raw(" "));

                let comp_focused = is_focused && self.widget.field_focus == QueryFieldFocus::Comparison;
                let comp_style = if comp_focused {
                    Style::default().bg(Color::Magenta).fg(Color::Black)
                } else {
                    Style::default().fg(Color::Magenta)
                };
                spans.push(Span::styled(
                    format!("{:<8}", condition.comparison.label()),
                    comp_style,
                ));
                spans.push(Span::raw(" "));

                let value_focused = is_focused && self.widget.field_focus == QueryFieldFocus::Value;
                let value_style = if value_focused {
                    Style::default().bg(Color::DarkGray).fg(Color::White)
                } else {
                    Style::default().fg(Color::White)
                };
                let value_display = if condition.search_value.is_empty() {
                    "<value>".to_string()
                } else {
                    condition.search_value.value().to_string()
                };
                let value_with_cursor = if value_focused {
                    let (before, cursor_ch, after) = condition.search_value.cursor_splits();
                    format!("{}{}{}", before, cursor_ch, after)
                } else {
                    value_display
                };
                spans.push(Span::styled(value_with_cursor, value_style));
            }
            ConditionType::FileType => {
                let cat_focused =
                    is_focused && self.widget.field_focus == QueryFieldFocus::FileTypeCategory;
                let cat_style = if cat_focused {
                    Style::default().bg(Color::Green).fg(Color::Black)
                } else {
                    Style::default().fg(Color::Green)
                };
                spans.push(Span::styled(
                    format!("[{}]", condition.file_type_category.label()),
                    cat_style,
                ));
            }
            ConditionType::SampleRate | ConditionType::Bitrate | ConditionType::Duration => {
                let min_focused = is_focused && self.widget.field_focus == QueryFieldFocus::RangeMin;
                let min_style = if min_focused {
                    Style::default().bg(Color::DarkGray).fg(Color::White)
                } else {
                    Style::default().fg(Color::White)
                };
                let min_display = if condition.range_min.is_empty() {
                    "<min>".to_string()
                } else {
                    condition.range_min.value().to_string()
                };
                let min_with_cursor = if min_focused {
                    let (before, cursor_ch, after) = condition.range_min.cursor_splits();
                    format!("{}{}{}", before, cursor_ch, after)
                } else {
                    min_display
                };
                spans.push(Span::styled(format!("{:<8}", min_with_cursor), min_style));
                spans.push(Span::styled(" to ", Style::default().fg(Color::DarkGray)));

                let max_focused = is_focused && self.widget.field_focus == QueryFieldFocus::RangeMax;
                let max_style = if max_focused {
                    Style::default().bg(Color::DarkGray).fg(Color::White)
                } else {
                    Style::default().fg(Color::White)
                };
                let max_display = if condition.range_max.is_empty() {
                    "<max>".to_string()
                } else {
                    condition.range_max.value().to_string()
                };
                let max_with_cursor = if max_focused {
                    let (before, cursor_ch, after) = condition.range_max.cursor_splits();
                    format!("{}{}{}", before, cursor_ch, after)
                } else {
                    max_display
                };
                spans.push(Span::styled(format!("{:<8}", max_with_cursor), max_style));

                let unit = match condition.condition_type {
                    ConditionType::SampleRate => "Hz",
                    ConditionType::Bitrate => "kbps",
                    ConditionType::Duration => "sec",
                    _ => "",
                };
                spans.push(Span::styled(
                    format!(" {}", unit),
                    Style::default().fg(Color::DarkGray),
                ));
            }
        }

        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    fn render_results(&mut self, f: &mut Frame, area: Rect) {
        let title = format!("Results ({} tracks)", self.results.len());
        let Self { ref mut widget, ref results, .. } = *self;

        render_standard_list(
            &mut widget.results_list,
            f,
            area,
            results,
            |idx, is_cursor, _is_selected, _width| {
                let full_path = &results[idx].result.path;
                let path_str = full_path.strip_prefix("corpus/").unwrap_or(full_path);
                let indicator = if is_cursor { "▶ " } else { "  " };
                let style = if is_cursor {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                Line::styled(format!("{}{}", indicator, path_str), style)
            },
            &title,
            true,
        );
    }
}
