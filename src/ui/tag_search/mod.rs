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

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::ui::helpers::render_pane;
use crate::ui::input::InputAction;
use crate::ui::widgets::standard_list::ListInputResult;

pub use state::TagSearchState;
pub use types::{
    SearchCondition, TagSearchAction, TagSearchMode,
};

impl TagSearchState {
    /// Handle a semantic input action. Returns an action that may require db access.
    pub fn handle_input(&mut self, action: &InputAction) -> TagSearchAction {
        // Handle modal first if active
        if self.modal.is_some() {
            return self.handle_modal_input(action);
        }

        match self.mode {
            TagSearchMode::QueryBuilder => self.handle_query_builder_input(action),
            TagSearchMode::Results => self.handle_results_mode_input(action),
        }
    }

    fn handle_modal_input(&mut self, action: &InputAction) -> TagSearchAction {
        // GatheringTags modal is non-interactive - handled by tick
        if matches!(self.modal, Some(types::TagSearchModal::GatheringTags)) {
            return TagSearchAction::None;
        }

        match action {
            // Enter or Escape dismisses the modal
            InputAction::Confirm | InputAction::Cancel => {
                self.modal = None;
                TagSearchAction::None
            }
            _ => TagSearchAction::None,
        }
    }

    fn handle_query_builder_input(&mut self, action: &InputAction) -> TagSearchAction {
        match action {
            // Tab: if on TagName field with partial text, apply tab-completion; otherwise cycle views
            InputAction::CycleNext => {
                if self.field_focus == QueryFieldFocus::TagName
                    && !self
                        .conditions
                        .get(self.focused_condition)
                        .map(|c| c.tag_name.is_empty())
                        .unwrap_or(true)
                {
                    self.apply_tag_name_suggestion();
                    TagSearchAction::None
                } else {
                    TagSearchAction::CycleNext
                }
            }
            InputAction::CyclePrev => TagSearchAction::CyclePrev,

            // Escape
            InputAction::Cancel => TagSearchAction::Cancel,

            // Navigate between conditions and fields
            InputAction::NavUp => {
                self.move_focus_up();
                TagSearchAction::None
            }
            InputAction::NavDown => {
                self.move_focus_down();
                TagSearchAction::None
            }
            InputAction::NavLeft => {
                self.move_focus_left();
                TagSearchAction::None
            }
            InputAction::NavRight => {
                self.move_focus_right();
                TagSearchAction::None
            }

            // Enter to execute search or add condition
            InputAction::Confirm => {
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
                } else if self.field_focus == QueryFieldFocus::ConditionType {
                    self.cycle_condition_type();
                    TagSearchAction::None
                } else if self.field_focus == QueryFieldFocus::FileTypeCategory {
                    self.cycle_file_type_category();
                    TagSearchAction::None
                } else {
                    // Move to next field
                    self.move_focus_right();
                    TagSearchAction::None
                }
            }

            // Text editing actions — delegate to focused TextInputState
            InputAction::Char(_)
            | InputAction::Paste(_)
            | InputAction::Backspace
            | InputAction::Delete
            | InputAction::TextHome
            | InputAction::TextEnd
            | InputAction::WordLeft
            | InputAction::WordRight
            | InputAction::KillToStart
            | InputAction::KillToEnd => {
                self.handle_text_input(action);
                TagSearchAction::None
            }

            _ => TagSearchAction::None,
        }
    }

    fn handle_results_mode_input(&mut self, action: &InputAction) -> TagSearchAction {
        // Handle 'b'/'B' before StandardList (it would treat Char as Unhandled anyway)
        if matches!(action, InputAction::Char('b' | 'B')) {
            let audio_files = self.all_result_audio_files();
            if !audio_files.is_empty() {
                self.modal = Some(types::TagSearchModal::GatheringTags);
                self.pending_bulk_edit = Some(audio_files);
            }
            return TagSearchAction::None;
        }

        let result = self.results_list.handle_input(action, &self.results);

        match result {
            ListInputResult::Consumed | ListInputResult::CursorMoved | ListInputResult::Toggled => {
                TagSearchAction::None
            }
            ListInputResult::Confirm(audio_file) => {
                TagSearchAction::EditAudioFile(audio_file)
            }
            ListInputResult::Unhandled => match action {
                InputAction::Cancel => {
                    self.mode = TagSearchMode::QueryBuilder;
                    TagSearchAction::None
                }
                InputAction::CycleNext => TagSearchAction::CycleNext,
                InputAction::CyclePrev => TagSearchAction::CyclePrev,
                _ => TagSearchAction::None,
            },
        }
    }

    /// Render the tag search view (titlebar is rendered by render_app).
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Content based on mode
        match self.mode {
            TagSearchMode::QueryBuilder => self.render_query_builder(f, area),
            TagSearchMode::Results => self.render_results(f, area),
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
            let search_style = if self.is_on_search_button() {
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
            let op_focused = is_focused && self.field_focus == QueryFieldFocus::Operator;
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
            spans.push(Span::raw("    ")); // Align with operators (4 chars: "AND ")
        }

        // Condition type selector [TAG/TYPE/RATE/KBPS/TIME]
        let type_focused = is_focused && self.field_focus == QueryFieldFocus::ConditionType;
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

        // Render fields based on condition type
        match condition.condition_type {
            ConditionType::Tag => {
                // Tag name field
                let name_focused = is_focused && self.field_focus == QueryFieldFocus::TagName;
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

                // Comparison operator
                let comp_focused = is_focused && self.field_focus == QueryFieldFocus::Comparison;
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

                // Value field
                let value_focused = is_focused && self.field_focus == QueryFieldFocus::Value;
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
                // File type category selector
                let cat_focused =
                    is_focused && self.field_focus == QueryFieldFocus::FileTypeCategory;
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
                // Range min field
                let min_focused = is_focused && self.field_focus == QueryFieldFocus::RangeMin;
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

                // Range max field
                let max_focused = is_focused && self.field_focus == QueryFieldFocus::RangeMax;
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

                // Show unit hint
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

        self.results_list.render(
            f,
            area,
            &self.results,
            |idx, is_cursor, _is_selected, _width| {
                let full_path = self.results[idx].audio_file.path();
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

/// Focus within query builder
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueryFieldFocus {
    Operator,
    /// Condition type selector (TAG/TYPE/RATE/KBPS/TIME)
    ConditionType,
    #[default]
    TagName,
    Comparison,
    Value,
    /// Range minimum for range conditions
    RangeMin,
    /// Range maximum for range conditions
    RangeMax,
    /// File type category selector (for FileType conditions)
    FileTypeCategory,
    AddCondition,
    SearchButton,
}
