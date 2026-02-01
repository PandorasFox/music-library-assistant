//! Tag Search State
//!
//! State management for the tag search query builder and results.

use std::collections::HashMap;

use crate::corpus::db::queries::ReadOnlyDb;
use crate::corpus::db::types::{AudioFile, FileSource};
// TODO: Re-enable when corpus::deploy is available
// use crate::corpus::deploy::compute_deployment_path_with_tags;

use super::types::{ConditionType, LogicalOperator, SearchCondition, TagSearchModal, TagSearchMode, SEARCHABLE_TAGS};
use super::QueryFieldFocus;

/// An audio file with its associated tags (for display and filtering).
#[derive(Debug, Clone)]
pub struct AudioFileWithTags {
    pub audio_file: AudioFile,
    pub tags: HashMap<String, String>,
}

impl AudioFileWithTags {
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

    /// Search results (audio files with tags matching query).
    pub results: Vec<AudioFileWithTags>,

    /// Selected result index.
    pub results_selected: usize,

    /// Results scroll offset.
    pub results_scroll: usize,

    /// Active modal dialog (if any).
    pub modal: Option<TagSearchModal>,

    /// Pending bulk edit audio files (set when showing "gathering" modal).
    pub pending_bulk_edit: Option<Vec<AudioFile>>,
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
            modal: None,
            pending_bulk_edit: None,
        }
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

    /// Check if focus is on a comparison field.
    pub fn is_on_comparison_field(&self) -> bool {
        self.field_focus == QueryFieldFocus::Comparison
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

    /// Move focus down.
    pub fn move_focus_down(&mut self) {
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
            QueryFieldFocus::SearchButton => {
                // Stay on search button
            }
        }
    }

    /// Move focus left.
    pub fn move_focus_left(&mut self) {
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

    /// Move focus right.
    pub fn move_focus_right(&mut self) {
        let condition_type = self.conditions.get(self.focused_condition)
            .map(|c| c.condition_type)
            .unwrap_or_default();

        match self.field_focus {
            QueryFieldFocus::Operator => {
                self.field_focus = QueryFieldFocus::ConditionType;
            }
            QueryFieldFocus::ConditionType => {
                // Move to appropriate fields based on condition type
                match condition_type {
                    ConditionType::Tag => {
                        self.field_focus = QueryFieldFocus::TagName;
                    }
                    ConditionType::FileType => {
                        self.field_focus = QueryFieldFocus::FileTypeCategory;
                    }
                    ConditionType::SampleRate | ConditionType::Bitrate | ConditionType::Duration => {
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

    /// Cycle the comparison operator for the current condition.
    pub fn cycle_comparison(&mut self) {
        if let Some(condition) = self.conditions.get_mut(self.focused_condition) {
            condition.comparison = condition.comparison.next();
        }
    }

    /// Cycle the condition type for the current condition.
    pub fn cycle_condition_type(&mut self) {
        if let Some(condition) = self.conditions.get_mut(self.focused_condition) {
            condition.condition_type = condition.condition_type.next();
        }
    }

    /// Cycle the file type category for the current condition.
    pub fn cycle_file_type_category(&mut self) {
        if let Some(condition) = self.conditions.get_mut(self.focused_condition) {
            condition.file_type_category = condition.file_type_category.next();
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
                QueryFieldFocus::RangeMin => {
                    // Only allow digits for range fields
                    if c.is_ascii_digit() {
                        condition.range_min.push(c);
                    }
                }
                QueryFieldFocus::RangeMax => {
                    if c.is_ascii_digit() {
                        condition.range_max.push(c);
                    }
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
                QueryFieldFocus::RangeMin => {
                    condition.range_min.pop();
                }
                QueryFieldFocus::RangeMax => {
                    condition.range_max.pop();
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

    /// Check if there's a pending bulk edit and take it.
    /// Returns the audio files if pending, clearing the pending state.
    pub fn take_pending_bulk_edit(&mut self) -> Option<Vec<AudioFile>> {
        if self.pending_bulk_edit.is_some() {
            self.modal = None;
            self.pending_bulk_edit.take()
        } else {
            None
        }
    }

    /// Execute the search query.
    pub fn execute_search(&mut self, read_db: &ReadOnlyDb<'_>) {
        // Build and execute the query
        let mut results = self.query_database(read_db);

        // Sort by corpus path (deployment path sorting disabled)
        // TODO: Re-enable deployment path sorting when corpus::deploy is available
        results.sort_by(|a, b| a.audio_file.path().cmp(b.audio_file.path()));

        self.results = results;
        self.results_selected = 0;
        self.results_scroll = 0;

        if self.results.is_empty() {
            // Show "no results" modal
            self.modal = Some(TagSearchModal::NoResults);
        } else {
            self.mode = TagSearchMode::Results;
        }
    }

    /// Query the database based on conditions.
    fn query_database(&self, read_db: &ReadOnlyDb<'_>) -> Vec<AudioFileWithTags> {
        // Get all audio files with tags
        let all_files = read_db.get_all_audio_files_with_tags(FileSource::Corpus).unwrap_or_default();

        // Convert to AudioFileWithTags and filter by conditions
        all_files
            .into_iter()
            .map(|(audio_file, tags)| AudioFileWithTags { audio_file, tags })
            .filter(|aft| self.evaluate_conditions(aft))
            .collect()
    }

    /// Evaluate all conditions against a track with tags.
    fn evaluate_conditions(&self, aft: &AudioFileWithTags) -> bool {
        if self.conditions.is_empty() {
            return true;
        }

        // Start with first condition (operator ignored)
        let mut result = self.evaluate_single_condition(aft, &self.conditions[0]);

        // Apply subsequent conditions with their operators
        for condition in self.conditions.iter().skip(1) {
            let cond_result = self.evaluate_single_condition(aft, condition);

            result = match condition.operator {
                LogicalOperator::And => result && cond_result,
                LogicalOperator::Or => result || cond_result,
                LogicalOperator::Xor => result ^ cond_result,
            };
        }

        result
    }

    /// Evaluate a single condition against a track with tags.
    fn evaluate_single_condition(&self, aft: &AudioFileWithTags, condition: &SearchCondition) -> bool {
        match condition.condition_type {
            ConditionType::Tag => self.evaluate_tag_condition(aft, condition),
            ConditionType::FileType => self.evaluate_file_type_condition(aft, condition),
            ConditionType::SampleRate => self.evaluate_sample_rate_condition(aft, condition),
            ConditionType::Bitrate => self.evaluate_bitrate_condition(aft, condition),
            ConditionType::Duration => self.evaluate_duration_condition(aft, condition),
        }
    }

    /// Evaluate a tag-based condition.
    fn evaluate_tag_condition(&self, aft: &AudioFileWithTags, condition: &SearchCondition) -> bool {
        use super::types::ComparisonOperator;

        if condition.tag_name.is_empty() || condition.value.is_empty() {
            return true; // Empty conditions match everything
        }

        let query = condition.value.to_lowercase();
        let tag_name = condition.tag_name.to_lowercase();

        // Get the tag value from the track's tags
        let field_value = aft.tags.get(&tag_name).map(|s| s.as_str());

        match condition.comparison {
            ComparisonOperator::Is => {
                // Exact match (case-insensitive)
                field_value
                    .map(|v| v.to_lowercase() == query)
                    .unwrap_or(false)
            }
            ComparisonOperator::Not => {
                // Negated exact match (case-insensitive)
                field_value
                    .map(|v| v.to_lowercase() != query)
                    .unwrap_or(true) // Missing field != query
            }
            ComparisonOperator::Contains => {
                // Substring match (case-insensitive)
                field_value
                    .map(|v| v.to_lowercase().contains(&query))
                    .unwrap_or(false)
            }
            ComparisonOperator::Like => {
                // SQL LIKE pattern (% = any chars, _ = single char)
                field_value
                    .map(|v| {
                        let v_lower = v.to_lowercase();
                        Self::match_like_pattern(&v_lower, &query)
                    })
                    .unwrap_or(false)
            }
        }
    }

    /// Evaluate a file type condition.
    fn evaluate_file_type_condition(&self, aft: &AudioFileWithTags, condition: &SearchCondition) -> bool {
        condition.file_type_category.matches(&aft.audio_file.audio.file_type)
    }

    /// Evaluate a sample rate range condition.
    fn evaluate_sample_rate_condition(&self, aft: &AudioFileWithTags, condition: &SearchCondition) -> bool {
        let sample_rate = aft.audio_file.audio.sample_rate.unwrap_or(0);
        self.evaluate_range(sample_rate as i64, &condition.range_min, &condition.range_max)
    }

    /// Evaluate a bitrate range condition (kbps).
    fn evaluate_bitrate_condition(&self, aft: &AudioFileWithTags, condition: &SearchCondition) -> bool {
        let bitrate = aft.audio_file.audio.bitrate_kbps.unwrap_or(0);
        self.evaluate_range(bitrate as i64, &condition.range_min, &condition.range_max)
    }

    /// Evaluate a duration range condition (seconds).
    fn evaluate_duration_condition(&self, aft: &AudioFileWithTags, condition: &SearchCondition) -> bool {
        // duration_ms is in milliseconds, convert to seconds for user-friendly input
        let duration_secs = aft.audio_file.audio.duration_ms.unwrap_or(0) / 1000;
        self.evaluate_range(duration_secs, &condition.range_min, &condition.range_max)
    }

    /// Evaluate a range condition (min <= value <= max).
    fn evaluate_range(&self, value: i64, min_str: &str, max_str: &str) -> bool {
        let min = min_str.parse::<i64>().unwrap_or(i64::MIN);
        let max = max_str.parse::<i64>().unwrap_or(i64::MAX);
        value >= min && value <= max
    }

    /// Match a SQL LIKE pattern (% = any chars, _ = single char).
    fn match_like_pattern(value: &str, pattern: &str) -> bool {
        Self::simple_like_match(value, pattern)
    }

    /// Simple LIKE pattern matching without regex.
    fn simple_like_match(value: &str, pattern: &str) -> bool {
        let v_chars: Vec<char> = value.chars().collect();
        let p_chars: Vec<char> = pattern.chars().collect();

        Self::like_match_recursive(&v_chars, &p_chars)
    }

    fn like_match_recursive(value: &[char], pattern: &[char]) -> bool {
        if pattern.is_empty() {
            return value.is_empty();
        }

        match pattern[0] {
            '%' => {
                // % matches zero or more characters
                // Try matching zero chars, or skip one char in value and try again
                Self::like_match_recursive(value, &pattern[1..])
                    || (!value.is_empty() && Self::like_match_recursive(&value[1..], pattern))
            }
            '_' => {
                // _ matches exactly one character
                !value.is_empty() && Self::like_match_recursive(&value[1..], &pattern[1..])
            }
            c => {
                // Literal character match
                !value.is_empty()
                    && value[0] == c
                    && Self::like_match_recursive(&value[1..], &pattern[1..])
            }
        }
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
    pub fn selected_result(&self) -> Option<&AudioFileWithTags> {
        self.results.get(self.results_selected)
    }

    /// Get all result audio files (without tags, for passing to tag editor).
    pub fn all_result_audio_files(&self) -> Vec<AudioFile> {
        self.results.iter().map(|aft| aft.audio_file.clone()).collect()
    }
}
