//! SearchWidget — backend-agnostic search query builder and result navigator.
//!
//! Manages the condition list, field focus, mode switching, and result
//! list navigation. Produces typed actions that the app layer dispatches
//! as protocol queries.

use crate::domain_types::{
    ConditionType, SearchCondition, TagSearchMode, SEARCHABLE_TAGS,
};
use crate::input::InputAction;
use crate::standard_list::StandardListState;

// ============================================================================
// QueryFieldFocus
// ============================================================================

/// Focus within the query builder condition grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QueryFieldFocus {
    /// Logical operator (AND/OR/XOR) — only for non-first conditions.
    Operator,
    /// Condition type selector (TAG/TYPE/RATE/KBPS/TIME).
    ConditionType,
    /// Tag name text input.
    #[default]
    TagName,
    /// Comparison operator (IS/NOT/CONTAINS/LIKE).
    Comparison,
    /// Tag value text input.
    Value,
    /// Range minimum text input.
    RangeMin,
    /// Range maximum text input.
    RangeMax,
    /// File type category selector.
    FileTypeCategory,
    /// Add-condition button.
    AddCondition,
    /// Search button.
    SearchButton,
}

// ============================================================================
// SearchAction
// ============================================================================

/// Actions produced by the SearchWidget.
#[derive(Debug, Clone)]
pub enum SearchAction {
    /// Execute search with current conditions (app layer fetches data).
    ExecuteSearch,
    /// User selected a result at index.
    SelectResult(usize),
    /// Bulk edit all results.
    BulkEdit,
    /// Switch between query builder and results mode.
    SwitchMode,
    /// Cancel / exit search.
    Cancel,
}

// ============================================================================
// SearchWidget
// ============================================================================

/// Backend-agnostic search interaction state.
///
/// Manages the query builder conditions and result list navigation.
/// The actual result data is stored externally; this widget only tracks
/// cursor state and count for display purposes.
#[derive(Debug)]
pub struct SearchWidget {
    /// Current mode (query builder or results).
    pub mode: TagSearchMode,
    /// Search conditions (at least one).
    pub conditions: Vec<SearchCondition>,
    /// Currently focused condition index.
    pub focused_condition: usize,
    /// Currently focused field within condition.
    pub field_focus: QueryFieldFocus,
    /// StandardList state for results navigation.
    pub results_list: StandardListState,
    /// Number of results (for display, actual data stored externally).
    pub result_count: usize,
}

impl Default for SearchWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchWidget {
    /// Create a new search widget.
    pub fn new() -> Self {
        Self {
            mode: TagSearchMode::QueryBuilder,
            conditions: vec![SearchCondition::new()],
            focused_condition: 0,
            field_focus: QueryFieldFocus::TagName,
            results_list: StandardListState::new(crate::standard_list::StandardListConfig::default()),
            result_count: 0,
        }
    }

    /// Handle a semantic input action.
    pub fn handle_input(&mut self, action: &InputAction) -> Option<SearchAction> {
        match self.mode {
            TagSearchMode::QueryBuilder => self.handle_query_builder_input(action),
            TagSearchMode::Results => self.handle_results_input(action),
        }
    }

    fn handle_query_builder_input(&mut self, action: &InputAction) -> Option<SearchAction> {
        match action {
            // Tab: tag-name completion when on TagName field with text
            InputAction::CycleNext => {
                if self.field_focus == QueryFieldFocus::TagName
                    && !self
                        .conditions
                        .get(self.focused_condition)
                        .map(|c| c.tag_name.is_empty())
                        .unwrap_or(true)
                {
                    self.apply_tag_name_suggestion();
                    None
                } else {
                    None // Let caller handle lateral cycling
                }
            }

            InputAction::Cancel => Some(SearchAction::Cancel),

            InputAction::NavUp => {
                self.move_focus_up();
                None
            }
            InputAction::NavDown => {
                self.move_focus_down();
                None
            }
            InputAction::NavLeft => {
                self.move_focus_left();
                None
            }
            InputAction::NavRight => {
                self.move_focus_right();
                None
            }

            InputAction::Confirm => {
                if self.field_focus == QueryFieldFocus::SearchButton {
                    Some(SearchAction::ExecuteSearch)
                } else if self.field_focus == QueryFieldFocus::AddCondition {
                    self.add_condition();
                    None
                } else if self.field_focus == QueryFieldFocus::Operator
                    && self.focused_condition > 0
                {
                    self.cycle_operator();
                    None
                } else if self.field_focus == QueryFieldFocus::Comparison {
                    self.cycle_comparison();
                    None
                } else if self.field_focus == QueryFieldFocus::ConditionType {
                    self.cycle_condition_type();
                    None
                } else if self.field_focus == QueryFieldFocus::FileTypeCategory {
                    self.cycle_file_type_category();
                    None
                } else {
                    self.move_focus_right();
                    None
                }
            }

            // Text editing — delegate to focused TextInputState
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
                None
            }

            _ => None,
        }
    }

    fn handle_results_input(&mut self, action: &InputAction) -> Option<SearchAction> {
        // B for bulk edit
        if matches!(action, InputAction::Char('b' | 'B')) {
            if self.result_count > 0 {
                return Some(SearchAction::BulkEdit);
            }
            return None;
        }

        match action {
            InputAction::Cancel => {
                self.mode = TagSearchMode::QueryBuilder;
                None
            }
            InputAction::NavUp => {
                if self.results_list.cursor > 0 {
                    self.results_list.cursor -= 1;
                    self.results_list.recompute_scroll(self.result_count);
                }
                None
            }
            InputAction::NavDown => {
                if self.results_list.cursor + 1 < self.result_count {
                    self.results_list.cursor += 1;
                    self.results_list.recompute_scroll(self.result_count);
                }
                None
            }
            InputAction::Confirm => {
                if self.results_list.cursor < self.result_count {
                    Some(SearchAction::SelectResult(self.results_list.cursor))
                } else {
                    None
                }
            }
            InputAction::Home => {
                self.results_list.cursor = 0;
                self.results_list.recompute_scroll(self.result_count);
                None
            }
            InputAction::End => {
                if self.result_count > 0 {
                    self.results_list.cursor = self.result_count - 1;
                    self.results_list.recompute_scroll(self.result_count);
                }
                None
            }
            _ => None,
        }
    }

    // =========================================================================
    // Focus navigation
    // =========================================================================

    fn move_focus_up(&mut self) {
        match self.field_focus {
            QueryFieldFocus::SearchButton => {
                self.field_focus = QueryFieldFocus::AddCondition;
            }
            QueryFieldFocus::AddCondition => {
                if !self.conditions.is_empty() {
                    self.focused_condition = self.conditions.len() - 1;
                    self.field_focus = QueryFieldFocus::ConditionType;
                }
            }
            QueryFieldFocus::Operator
            | QueryFieldFocus::ConditionType
            | QueryFieldFocus::TagName
            | QueryFieldFocus::Comparison
            | QueryFieldFocus::Value
            | QueryFieldFocus::RangeMin
            | QueryFieldFocus::RangeMax
            | QueryFieldFocus::FileTypeCategory => {
                if self.focused_condition > 0 {
                    self.focused_condition -= 1;
                }
            }
        }
    }

    fn move_focus_down(&mut self) {
        match self.field_focus {
            QueryFieldFocus::Operator
            | QueryFieldFocus::ConditionType
            | QueryFieldFocus::TagName
            | QueryFieldFocus::Comparison
            | QueryFieldFocus::Value
            | QueryFieldFocus::RangeMin
            | QueryFieldFocus::RangeMax
            | QueryFieldFocus::FileTypeCategory => {
                if self.focused_condition + 1 < self.conditions.len() {
                    self.focused_condition += 1;
                } else {
                    self.field_focus = QueryFieldFocus::AddCondition;
                }
            }
            QueryFieldFocus::AddCondition => {
                self.field_focus = QueryFieldFocus::SearchButton;
            }
            QueryFieldFocus::SearchButton => {}
        }
    }

    fn move_focus_left(&mut self) {
        match self.field_focus {
            QueryFieldFocus::Value => {
                self.field_focus = QueryFieldFocus::Comparison;
            }
            QueryFieldFocus::Comparison => {
                self.field_focus = QueryFieldFocus::TagName;
            }
            QueryFieldFocus::TagName => {
                self.field_focus = QueryFieldFocus::ConditionType;
            }
            QueryFieldFocus::ConditionType => {
                if self.focused_condition > 0 {
                    self.field_focus = QueryFieldFocus::Operator;
                }
            }
            QueryFieldFocus::RangeMax => {
                self.field_focus = QueryFieldFocus::RangeMin;
            }
            QueryFieldFocus::RangeMin => {
                self.field_focus = QueryFieldFocus::ConditionType;
            }
            QueryFieldFocus::FileTypeCategory => {
                self.field_focus = QueryFieldFocus::ConditionType;
            }
            _ => {}
        }
    }

    fn move_focus_right(&mut self) {
        let condition_type = self
            .conditions
            .get(self.focused_condition)
            .map(|c| c.condition_type)
            .unwrap_or_default();

        match self.field_focus {
            QueryFieldFocus::Operator => {
                self.field_focus = QueryFieldFocus::ConditionType;
            }
            QueryFieldFocus::ConditionType => {
                match condition_type {
                    ConditionType::Tag => {
                        self.field_focus = QueryFieldFocus::TagName;
                    }
                    ConditionType::FileType => {
                        self.field_focus = QueryFieldFocus::FileTypeCategory;
                    }
                    ConditionType::SampleRate
                    | ConditionType::Bitrate
                    | ConditionType::Duration => {
                        self.field_focus = QueryFieldFocus::RangeMin;
                    }
                }
            }
            QueryFieldFocus::TagName => {
                self.field_focus = QueryFieldFocus::Comparison;
            }
            QueryFieldFocus::Comparison => {
                self.field_focus = QueryFieldFocus::Value;
            }
            QueryFieldFocus::RangeMin => {
                self.field_focus = QueryFieldFocus::RangeMax;
            }
            _ => {}
        }
    }

    // =========================================================================
    // Condition management
    // =========================================================================

    /// Add a new condition.
    pub fn add_condition(&mut self) {
        self.conditions.push(SearchCondition::new());
        self.focused_condition = self.conditions.len() - 1;
        self.field_focus = QueryFieldFocus::TagName;
    }

    /// Remove the focused condition (if more than one).
    pub fn remove_condition(&mut self) {
        if self.conditions.len() > 1 {
            self.conditions.remove(self.focused_condition);
            if self.focused_condition >= self.conditions.len() {
                self.focused_condition = self.conditions.len() - 1;
            }
        }
    }

    fn cycle_operator(&mut self) {
        if let Some(condition) = self.conditions.get_mut(self.focused_condition) {
            condition.operator = condition.operator.next();
        }
    }

    fn cycle_comparison(&mut self) {
        if let Some(condition) = self.conditions.get_mut(self.focused_condition) {
            condition.comparison = condition.comparison.next();
        }
    }

    fn cycle_condition_type(&mut self) {
        if let Some(condition) = self.conditions.get_mut(self.focused_condition) {
            condition.condition_type = condition.condition_type.next();
        }
    }

    fn cycle_file_type_category(&mut self) {
        if let Some(condition) = self.conditions.get_mut(self.focused_condition) {
            condition.file_type_category = condition.file_type_category.next();
        }
    }

    fn handle_text_input(&mut self, action: &InputAction) -> bool {
        // Range fields: only allow digits
        if matches!(
            self.field_focus,
            QueryFieldFocus::RangeMin | QueryFieldFocus::RangeMax
        ) {
            if let InputAction::Char(c) = action {
                if !c.is_ascii_digit() {
                    return true;
                }
            }
        }

        if let Some(input) = self.focused_input_mut() {
            input.handle_input(action)
        } else {
            false
        }
    }

    fn focused_input_mut(&mut self) -> Option<&mut crate::text_input::TextInputState> {
        let condition = self.conditions.get_mut(self.focused_condition)?;
        match self.field_focus {
            QueryFieldFocus::TagName => Some(&mut condition.tag_name),
            QueryFieldFocus::Value => Some(&mut condition.search_value),
            QueryFieldFocus::RangeMin => Some(&mut condition.range_min),
            QueryFieldFocus::RangeMax => Some(&mut condition.range_max),
            _ => None,
        }
    }

    fn apply_tag_name_suggestion(&mut self) {
        if let Some(condition) = self.conditions.get_mut(self.focused_condition) {
            let query = condition.tag_name.value().to_lowercase();
            if !query.is_empty() {
                if let Some(match_name) = SEARCHABLE_TAGS.iter().find(|t| t.starts_with(&query)) {
                    condition.tag_name.set_value(*match_name);
                }
            }
        }
    }

    /// Convert all conditions to wire format for protocol queries.
    pub fn conditions_to_wire(&self) -> Vec<mm_meta::domain_query_types::SearchConditionWire> {
        self.conditions.iter().map(|c| c.to_wire()).collect()
    }

    /// Set result count and switch to results mode.
    pub fn set_results(&mut self, count: usize) {
        self.result_count = count;
        self.results_list.reset();
        if count > 0 {
            self.mode = TagSearchMode::Results;
        }
    }

    // =========================================================================
    // Accessors for render layer
    // =========================================================================

    /// Check if focus is on the search button.
    pub fn is_on_search_button(&self) -> bool {
        self.field_focus == QueryFieldFocus::SearchButton
    }

    /// Check if focus is on the add condition button.
    pub fn is_on_add_condition(&self) -> bool {
        self.field_focus == QueryFieldFocus::AddCondition
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_widget_defaults() {
        let w = SearchWidget::new();
        assert_eq!(w.mode, TagSearchMode::QueryBuilder);
        assert_eq!(w.conditions.len(), 1);
        assert_eq!(w.field_focus, QueryFieldFocus::TagName);
    }

    #[test]
    fn add_and_remove_conditions() {
        let mut w = SearchWidget::new();
        w.add_condition();
        assert_eq!(w.conditions.len(), 2);
        assert_eq!(w.focused_condition, 1);

        w.remove_condition();
        assert_eq!(w.conditions.len(), 1);
        assert_eq!(w.focused_condition, 0);
    }

    #[test]
    fn conditions_to_wire() {
        let mut w = SearchWidget::new();
        w.conditions[0].tag_name.set_value("artist");
        w.conditions[0].search_value.set_value("Bach");
        let wire = w.conditions_to_wire();
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0].tag_name, "artist");
        assert_eq!(wire[0].search_value, "Bach");
    }

    #[test]
    fn execute_search_action() {
        let mut w = SearchWidget::new();
        w.field_focus = QueryFieldFocus::SearchButton;
        let action = w.handle_input(&InputAction::Confirm);
        assert!(matches!(action, Some(SearchAction::ExecuteSearch)));
    }

    #[test]
    fn results_mode_navigation() {
        let mut w = SearchWidget::new();
        w.set_results(5);
        assert_eq!(w.mode, TagSearchMode::Results);
        assert_eq!(w.result_count, 5);

        // Navigate down
        w.handle_input(&InputAction::NavDown);
        assert_eq!(w.results_list.cursor, 1);

        // Select
        let action = w.handle_input(&InputAction::Confirm);
        assert!(matches!(action, Some(SearchAction::SelectResult(1))));

        // Escape returns to query builder
        w.handle_input(&InputAction::Cancel);
        assert_eq!(w.mode, TagSearchMode::QueryBuilder);
    }
}
