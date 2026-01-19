//! Tag Search State
//!
//! State management for the tag search query builder and results.

use std::collections::HashMap;

use crate::corpus::db::{Database, Track};
use crate::flows::deploy::compute_deployment_path_with_tags;

use super::types::{LogicalOperator, SearchCondition, TagSearchMode, SEARCHABLE_TAGS};
use super::QueryFieldFocus;

/// A track with its associated tags (for display and filtering).
#[derive(Debug, Clone)]
pub struct TrackWithTags {
    pub track: Track,
    pub tags: HashMap<String, String>,
}

impl TrackWithTags {
    /// Get a tag value by name (case-insensitive).
    pub fn get_tag(&self, name: &str) -> Option<&str> {
        self.tags.get(name).map(|s| s.as_str())
    }
}

/// State for the tag search view.
#[derive(Debug)]
pub struct TagSearchState {
    /// Current mode (query builder or results).
    pub mode: TagSearchMode,

    /// Search conditions (at least one).
    pub conditions: Vec<SearchCondition>,

    /// Currently focused condition index.
    pub focused_condition: usize,

    /// Currently focused field within condition.
    pub field_focus: QueryFieldFocus,

    /// Search results (tracks with tags matching query).
    pub results: Vec<TrackWithTags>,

    /// Selected result index.
    pub results_selected: usize,

    /// Results scroll offset.
    pub results_scroll: usize,
}

impl Default for TagSearchState {
    fn default() -> Self {
        Self::new()
    }
}

impl TagSearchState {
    /// Create a new tag search state.
    pub fn new() -> Self {
        Self {
            mode: TagSearchMode::QueryBuilder,
            conditions: vec![SearchCondition::new()],
            focused_condition: 0,
            field_focus: QueryFieldFocus::TagName,
            results: Vec::new(),
            results_selected: 0,
            results_scroll: 0,
        }
    }

    /// Get the currently active field focus.
    pub fn active_field_focus(&self) -> QueryFieldFocus {
        self.field_focus
    }

    /// Check if focus is on the search button.
    pub fn is_on_search_button(&self) -> bool {
        self.field_focus == QueryFieldFocus::SearchButton
    }

    /// Check if focus is on the add condition button.
    pub fn is_on_add_condition(&self) -> bool {
        self.field_focus == QueryFieldFocus::AddCondition
    }

    /// Check if focus is on an operator field.
    pub fn is_on_operator_field(&self) -> bool {
        self.field_focus == QueryFieldFocus::Operator && self.focused_condition > 0
    }

    /// Move focus up.
    pub fn move_focus_up(&mut self) {
        match self.field_focus {
            QueryFieldFocus::SearchButton => {
                self.field_focus = QueryFieldFocus::AddCondition;
            }
            QueryFieldFocus::AddCondition => {
                if !self.conditions.is_empty() {
                    self.focused_condition = self.conditions.len() - 1;
                    self.field_focus = QueryFieldFocus::TagName;
                }
            }
            QueryFieldFocus::Operator | QueryFieldFocus::TagName | QueryFieldFocus::Value => {
                if self.focused_condition > 0 {
                    self.focused_condition -= 1;
                }
            }
        }
    }

    /// Move focus down.
    pub fn move_focus_down(&mut self) {
        match self.field_focus {
            QueryFieldFocus::Operator | QueryFieldFocus::TagName | QueryFieldFocus::Value => {
                if self.focused_condition + 1 < self.conditions.len() {
                    self.focused_condition += 1;
                } else {
                    self.field_focus = QueryFieldFocus::AddCondition;
                }
            }
            QueryFieldFocus::AddCondition => {
                self.field_focus = QueryFieldFocus::SearchButton;
            }
            QueryFieldFocus::SearchButton => {
                // Stay on search button
            }
        }
    }

    /// Move focus left.
    pub fn move_focus_left(&mut self) {
        match self.field_focus {
            QueryFieldFocus::Value => {
                self.field_focus = QueryFieldFocus::TagName;
            }
            QueryFieldFocus::TagName => {
                if self.focused_condition > 0 {
                    self.field_focus = QueryFieldFocus::Operator;
                }
            }
            _ => {}
        }
    }

    /// Move focus right.
    pub fn move_focus_right(&mut self) {
        match self.field_focus {
            QueryFieldFocus::Operator => {
                self.field_focus = QueryFieldFocus::TagName;
            }
            QueryFieldFocus::TagName => {
                self.field_focus = QueryFieldFocus::Value;
            }
            _ => {}
        }
    }

    /// Insert a character at the current field.
    pub fn insert_char(&mut self, c: char) {
        if let Some(condition) = self.conditions.get_mut(self.focused_condition) {
            match self.field_focus {
                QueryFieldFocus::TagName => {
                    condition.tag_name.push(c);
                }
                QueryFieldFocus::Value => {
                    condition.value.push(c);
                }
                _ => {}
            }
        }
    }

    /// Backspace at the current field.
    pub fn backspace(&mut self) {
        if let Some(condition) = self.conditions.get_mut(self.focused_condition) {
            match self.field_focus {
                QueryFieldFocus::TagName => {
                    condition.tag_name.pop();
                }
                QueryFieldFocus::Value => {
                    condition.value.pop();
                }
                _ => {}
            }
        }
    }

    /// Delete at the current field.
    pub fn delete(&mut self) {
        // For simplicity, same as backspace for now
        self.backspace();
    }

    /// Add a new condition.
    pub fn add_condition(&mut self) {
        self.conditions.push(SearchCondition::new());
        self.focused_condition = self.conditions.len() - 1;
        self.field_focus = QueryFieldFocus::TagName;
    }

    /// Cycle the operator for the current condition.
    pub fn cycle_operator(&mut self) {
        if let Some(condition) = self.conditions.get_mut(self.focused_condition) {
            condition.operator = condition.operator.next();
        }
    }

    /// Apply tag name suggestion (tab-completion).
    pub fn apply_tag_name_suggestion(&mut self) {
        if let Some(condition) = self.conditions.get_mut(self.focused_condition) {
            let query = condition.tag_name.to_lowercase();
            if !query.is_empty() {
                // Find first matching tag name
                if let Some(match_name) = SEARCHABLE_TAGS.iter().find(|t| t.starts_with(&query)) {
                    condition.tag_name = (*match_name).to_string();
                }
            }
        }
    }

    /// Execute the search query.
    pub fn execute_search(&mut self, db: &Database) {
        // Build and execute the query
        let mut results = self.query_database(db);

        // Sort by deployment path
        results.sort_by(|a, b| {
            let path_a = compute_deployment_path_with_tags(&a.track, &a.tags);
            let path_b = compute_deployment_path_with_tags(&b.track, &b.tags);
            path_a.cmp(&path_b)
        });

        self.results = results;
        self.results_selected = 0;
        self.results_scroll = 0;

        if !self.results.is_empty() {
            self.mode = TagSearchMode::Results;
        }
    }

    /// Query the database based on conditions.
    fn query_database(&self, db: &Database) -> Vec<TrackWithTags> {
        // Get all tracks with tags
        let all_tracks = db.get_all_tracks_with_tags().unwrap_or_default();

        // Convert to TrackWithTags and filter by conditions
        all_tracks
            .into_iter()
            .map(|(track, tags)| TrackWithTags { track, tags })
            .filter(|twt| self.evaluate_conditions(twt))
            .collect()
    }

    /// Evaluate all conditions against a track with tags.
    fn evaluate_conditions(&self, twt: &TrackWithTags) -> bool {
        if self.conditions.is_empty() {
            return true;
        }

        // Start with first condition (operator ignored)
        let mut result = self.evaluate_single_condition(twt, &self.conditions[0]);

        // Apply subsequent conditions with their operators
        for condition in self.conditions.iter().skip(1) {
            let cond_result = self.evaluate_single_condition(twt, condition);

            result = match condition.operator {
                LogicalOperator::And => result && cond_result,
                LogicalOperator::Or => result || cond_result,
                LogicalOperator::Xor => result ^ cond_result,
                LogicalOperator::Not => result && !cond_result,
            };
        }

        result
    }

    /// Evaluate a single condition against a track with tags.
    fn evaluate_single_condition(&self, twt: &TrackWithTags, condition: &SearchCondition) -> bool {
        if condition.tag_name.is_empty() || condition.value.is_empty() {
            return true; // Empty conditions match everything
        }

        let query = condition.value.to_lowercase();
        let tag_name = condition.tag_name.to_lowercase();

        // Get the tag value from the track's tags
        let field_value = twt.tags.get(&tag_name).map(|s| s.as_str());

        field_value
            .map(|v| v.to_lowercase().contains(&query))
            .unwrap_or(false)
    }

    /// Select previous result.
    pub fn results_select_prev(&mut self) {
        if self.results_selected > 0 {
            self.results_selected -= 1;
            self.ensure_results_visible();
        }
    }

    /// Select next result.
    pub fn results_select_next(&mut self) {
        if self.results_selected + 1 < self.results.len() {
            self.results_selected += 1;
            self.ensure_results_visible();
        }
    }

    /// Ensure selected result is visible.
    fn ensure_results_visible(&mut self) {
        // Assume visible height of ~20 for now
        let visible_height = 20;
        if self.results_selected < self.results_scroll {
            self.results_scroll = self.results_selected;
        } else if self.results_selected >= self.results_scroll + visible_height {
            self.results_scroll = self.results_selected.saturating_sub(visible_height - 1);
        }
    }

    /// Get the currently selected result.
    pub fn selected_result(&self) -> Option<&TrackWithTags> {
        self.results.get(self.results_selected)
    }

    /// Get all result tracks (without tags, for passing to tag editor).
    pub fn all_result_tracks(&self) -> Vec<Track> {
        self.results.iter().map(|twt| twt.track.clone()).collect()
    }
}
