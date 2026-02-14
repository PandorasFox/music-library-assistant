//! Filter Popup State
//!
//! State management for the filter popup overlay.

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::tag_search::{ComparisonOperator, FileTypeCategory};

/// Direction for cycling through options.
#[derive(Debug, Clone, Copy)]
enum CycleDirection {
    Forward,
    Backward,
}

/// Type of filter condition in the filter popup.
///
/// Separate from tag_search::ConditionType to allow filter-specific variants
/// like Path, and to default to Path (most common for corpus browser).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterConditionType {
    /// Path substring filter (case-insensitive) - default for corpus browser
    #[default]
    Path,
    /// File type filter (lossless/lossy, specific formats)
    FileType,
    /// Sample rate range filter (Hz)
    SampleRate,
    /// Bitrate range filter (kbps)
    Bitrate,
    /// Duration range filter (seconds)
    Duration,
    /// Tag-based filter with comparison operators
    Tag,
}

impl FilterConditionType {
    /// Cycle to the next condition type.
    pub fn next(&self) -> Self {
        match self {
            FilterConditionType::Path => FilterConditionType::FileType,
            FilterConditionType::FileType => FilterConditionType::SampleRate,
            FilterConditionType::SampleRate => FilterConditionType::Bitrate,
            FilterConditionType::Bitrate => FilterConditionType::Duration,
            FilterConditionType::Duration => FilterConditionType::Tag,
            FilterConditionType::Tag => FilterConditionType::Path,
        }
    }

    /// Cycle to the previous condition type.
    pub fn prev(&self) -> Self {
        match self {
            FilterConditionType::Path => FilterConditionType::Tag,
            FilterConditionType::FileType => FilterConditionType::Path,
            FilterConditionType::SampleRate => FilterConditionType::FileType,
            FilterConditionType::Bitrate => FilterConditionType::SampleRate,
            FilterConditionType::Duration => FilterConditionType::Bitrate,
            FilterConditionType::Tag => FilterConditionType::Duration,
        }
    }
}

/// Focus position within the filter popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FilterFieldFocus {
    /// Condition type selector
    #[default]
    ConditionType,
    /// File type category (for FileType conditions)
    FileTypeCategory,
    /// Range minimum (for range conditions)
    RangeMin,
    /// Range maximum (for range conditions)
    RangeMax,
    /// Path substring (for Path conditions)
    PathSubstring,
    /// Tag name (for Tag conditions)
    TagName,
    /// Tag comparison operator (for Tag conditions)
    TagComparison,
    /// Tag value (for Tag conditions)
    TagValue,
    /// Apply button
    ApplyButton,
    /// Clear button
    ClearButton,
}

/// Actions returned from filter popup key handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterPopupAction {
    /// No action needed.
    None,
    /// Apply the filter and close popup.
    Apply,
    /// Clear the filter and close popup.
    Clear,
    /// Close popup without changes (Escape).
    Cancel,
}

/// Searchable tag field names for filter popup.
pub const FILTER_SEARCHABLE_TAGS: &[&str] = &[
    "artist",
    "album",
    "album_artist",
    "title",
    "genre",
];

/// State for a single filter condition.
#[derive(Debug, Clone, Default)]
pub struct FilterCondition {
    /// Type of filter.
    pub condition_type: FilterConditionType,
    /// File type category (for FileType conditions).
    pub file_type_category: FileTypeCategory,
    /// Range minimum (for range conditions).
    pub range_min: String,
    /// Range maximum (for range conditions).
    pub range_max: String,
    /// Path substring (for Path conditions - case-insensitive).
    pub path_substring: String,
    /// Tag name (for Tag conditions).
    pub tag_name: String,
    /// Tag comparison operator (for Tag conditions).
    pub tag_comparison: ComparisonOperator,
    /// Tag value (for Tag conditions).
    pub tag_value: String,
    /// Index into FILTER_SEARCHABLE_TAGS for tag name cycling.
    tag_name_idx: usize,
}

impl FilterCondition {
    /// Create a new empty filter condition.
    pub fn new() -> Self {
        Self {
            condition_type: FilterConditionType::Path, // Default to path for corpus browser
            tag_name: "artist".to_string(), // Default tag name
            ..Default::default()
        }
    }

    /// Check if this filter has any active constraints.
    pub fn is_active(&self) -> bool {
        match self.condition_type {
            FilterConditionType::Path => !self.path_substring.is_empty(),
            FilterConditionType::FileType => self.file_type_category != FileTypeCategory::Any,
            FilterConditionType::SampleRate | FilterConditionType::Bitrate | FilterConditionType::Duration => {
                !self.range_min.is_empty() || !self.range_max.is_empty()
            }
            FilterConditionType::Tag => !self.tag_value.is_empty(),
        }
    }

    /// Check if a track matches this filter condition.
    pub fn matches(
        &self,
        path: &str,
        file_type: &str,
        sample_rate: Option<i32>,
        bitrate_kbps: Option<i32>,
        duration_ms: Option<i64>,
        tags: &HashMap<String, Vec<String>>,
    ) -> bool {
        match self.condition_type {
            FilterConditionType::Path => self.matches_path(path),
            FilterConditionType::FileType => self.file_type_category.matches(file_type),
            FilterConditionType::SampleRate => {
                let value = sample_rate.unwrap_or(0) as i64;
                Self::in_range(value, &self.range_min, &self.range_max)
            }
            FilterConditionType::Bitrate => {
                let value = bitrate_kbps.unwrap_or(0) as i64;
                Self::in_range(value, &self.range_min, &self.range_max)
            }
            FilterConditionType::Duration => {
                let value = duration_ms.unwrap_or(0) / 1000; // Convert to seconds
                Self::in_range(value, &self.range_min, &self.range_max)
            }
            FilterConditionType::Tag => self.matches_tag(tags),
        }
    }

    /// Check if a path matches the path substring filter.
    fn matches_path(&self, path: &str) -> bool {
        if self.path_substring.is_empty() {
            return true;
        }
        path.to_lowercase().contains(&self.path_substring.to_lowercase())
    }

    /// Check if tags match the tag condition (checks all values for multi-value tags).
    fn matches_tag(&self, tags: &HashMap<String, Vec<String>>) -> bool {
        if self.tag_value.is_empty() {
            return true;
        }

        let values = tags.get(&self.tag_name.to_lowercase());
        let query = self.tag_value.to_lowercase();

        match self.tag_comparison {
            ComparisonOperator::Is => {
                values
                    .map(|vals| vals.iter().any(|v| v.to_lowercase() == query))
                    .unwrap_or(false)
            }
            ComparisonOperator::Not => {
                values
                    .map(|vals| vals.iter().all(|v| v.to_lowercase() != query))
                    .unwrap_or(true) // Missing tag != query
            }
            ComparisonOperator::Contains => {
                values
                    .map(|vals| vals.iter().any(|v| v.to_lowercase().contains(&query)))
                    .unwrap_or(false)
            }
            ComparisonOperator::Like => {
                values
                    .map(|vals| vals.iter().any(|v| Self::match_like_pattern(&v.to_lowercase(), &query)))
                    .unwrap_or(false)
            }
        }
    }

    /// Match a LIKE pattern (% = any chars, _ = single char).
    fn match_like_pattern(text: &str, pattern: &str) -> bool {
        // Simple recursive LIKE pattern matching without regex
        Self::like_match_recursive(text.chars().collect::<Vec<_>>().as_slice(),
                                   pattern.chars().collect::<Vec<_>>().as_slice())
    }

    fn like_match_recursive(text: &[char], pattern: &[char]) -> bool {
        match (text.first(), pattern.first()) {
            // Both empty - match
            (None, None) => true,
            // Pattern has %, match zero or more chars
            (_, Some('%')) => {
                // Try matching zero chars (skip %)
                Self::like_match_recursive(text, &pattern[1..])
                // Or match one char and try again
                || (!text.is_empty() && Self::like_match_recursive(&text[1..], pattern))
            }
            // Pattern has _, match exactly one char
            (Some(_), Some('_')) => {
                Self::like_match_recursive(&text[1..], &pattern[1..])
            }
            // Literal match
            (Some(t), Some(p)) if *t == *p => {
                Self::like_match_recursive(&text[1..], &pattern[1..])
            }
            // Pattern empty but text not, or chars don't match
            _ => false,
        }
    }

    /// Check if a value is within the range.
    fn in_range(value: i64, min_str: &str, max_str: &str) -> bool {
        let min = min_str.parse::<i64>().unwrap_or(i64::MIN);
        let max = max_str.parse::<i64>().unwrap_or(i64::MAX);
        value >= min && value <= max
    }

    /// Cycle to next tag name.
    fn cycle_tag_name_next(&mut self) {
        self.tag_name_idx = (self.tag_name_idx + 1) % FILTER_SEARCHABLE_TAGS.len();
        self.tag_name = FILTER_SEARCHABLE_TAGS[self.tag_name_idx].to_string();
    }

    /// Cycle to previous tag name.
    fn cycle_tag_name_prev(&mut self) {
        if self.tag_name_idx == 0 {
            self.tag_name_idx = FILTER_SEARCHABLE_TAGS.len() - 1;
        } else {
            self.tag_name_idx -= 1;
        }
        self.tag_name = FILTER_SEARCHABLE_TAGS[self.tag_name_idx].to_string();
    }
}

/// State for the filter popup overlay.
#[derive(Debug, Clone)]
pub struct FilterPopupState {
    /// The filter condition being edited.
    pub condition: FilterCondition,
    /// Current field focus.
    pub focus: FilterFieldFocus,
}

impl Default for FilterPopupState {
    fn default() -> Self {
        Self::new()
    }
}

impl FilterPopupState {
    /// Create a new filter popup state.
    pub fn new() -> Self {
        Self {
            condition: FilterCondition::new(),
            focus: FilterFieldFocus::ConditionType,
        }
    }

    /// Handle a key event.
    pub fn handle_key(&mut self, key: KeyEvent) -> FilterPopupAction {
        match key.code {
            KeyCode::Esc => FilterPopupAction::Cancel,

            KeyCode::Enter => {
                match self.focus {
                    FilterFieldFocus::ApplyButton => FilterPopupAction::Apply,
                    FilterFieldFocus::ClearButton => FilterPopupAction::Clear,
                    _ => {
                        // Enter on other fields moves to Apply
                        self.focus = FilterFieldFocus::ApplyButton;
                        FilterPopupAction::None
                    }
                }
            }

            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    self.focus_prev();
                } else {
                    self.focus_next();
                }
                FilterPopupAction::None
            }

            KeyCode::Up => {
                self.focus_prev();
                FilterPopupAction::None
            }

            KeyCode::Down => {
                self.focus_next();
                FilterPopupAction::None
            }

            KeyCode::Left => {
                self.handle_cycle_key(CycleDirection::Backward);
                FilterPopupAction::None
            }
            KeyCode::Right => {
                self.handle_cycle_key(CycleDirection::Forward);
                FilterPopupAction::None
            }

            KeyCode::Char(c) => {
                self.handle_char(c);
                FilterPopupAction::None
            }

            KeyCode::Backspace => {
                self.handle_backspace();
                FilterPopupAction::None
            }

            _ => FilterPopupAction::None,
        }
    }

    /// Move focus to the next field.
    fn focus_next(&mut self) {
        self.focus = match self.focus {
            FilterFieldFocus::ConditionType => {
                match self.condition.condition_type {
                    FilterConditionType::Path => FilterFieldFocus::PathSubstring,
                    FilterConditionType::FileType => FilterFieldFocus::FileTypeCategory,
                    FilterConditionType::SampleRate | FilterConditionType::Bitrate | FilterConditionType::Duration => {
                        FilterFieldFocus::RangeMin
                    }
                    FilterConditionType::Tag => FilterFieldFocus::TagName,
                }
            }
            FilterFieldFocus::PathSubstring => FilterFieldFocus::ApplyButton,
            FilterFieldFocus::FileTypeCategory => FilterFieldFocus::ApplyButton,
            FilterFieldFocus::RangeMin => FilterFieldFocus::RangeMax,
            FilterFieldFocus::RangeMax => FilterFieldFocus::ApplyButton,
            FilterFieldFocus::TagName => FilterFieldFocus::TagComparison,
            FilterFieldFocus::TagComparison => FilterFieldFocus::TagValue,
            FilterFieldFocus::TagValue => FilterFieldFocus::ApplyButton,
            FilterFieldFocus::ApplyButton => FilterFieldFocus::ClearButton,
            FilterFieldFocus::ClearButton => FilterFieldFocus::ConditionType,
        };
    }

    /// Move focus to the previous field.
    fn focus_prev(&mut self) {
        self.focus = match self.focus {
            FilterFieldFocus::ConditionType => FilterFieldFocus::ClearButton,
            FilterFieldFocus::PathSubstring => FilterFieldFocus::ConditionType,
            FilterFieldFocus::FileTypeCategory => FilterFieldFocus::ConditionType,
            FilterFieldFocus::RangeMin => FilterFieldFocus::ConditionType,
            FilterFieldFocus::RangeMax => FilterFieldFocus::RangeMin,
            FilterFieldFocus::TagName => FilterFieldFocus::ConditionType,
            FilterFieldFocus::TagComparison => FilterFieldFocus::TagName,
            FilterFieldFocus::TagValue => FilterFieldFocus::TagComparison,
            FilterFieldFocus::ApplyButton => {
                match self.condition.condition_type {
                    FilterConditionType::Path => FilterFieldFocus::PathSubstring,
                    FilterConditionType::FileType => FilterFieldFocus::FileTypeCategory,
                    FilterConditionType::SampleRate | FilterConditionType::Bitrate | FilterConditionType::Duration => {
                        FilterFieldFocus::RangeMax
                    }
                    FilterConditionType::Tag => FilterFieldFocus::TagValue,
                }
            }
            FilterFieldFocus::ClearButton => FilterFieldFocus::ApplyButton,
        };
    }

    /// Handle left/right for cycling options.
    fn handle_cycle_key(&mut self, direction: CycleDirection) {
        match self.focus {
            FilterFieldFocus::ConditionType => {
                self.condition.condition_type = match direction {
                    CycleDirection::Forward => self.condition.condition_type.next(),
                    CycleDirection::Backward => self.condition.condition_type.prev(),
                };
            }
            FilterFieldFocus::FileTypeCategory => {
                self.condition.file_type_category = match direction {
                    CycleDirection::Forward => self.condition.file_type_category.next(),
                    CycleDirection::Backward => self.condition.file_type_category.prev(),
                };
            }
            FilterFieldFocus::TagName => {
                match direction {
                    CycleDirection::Forward => self.condition.cycle_tag_name_next(),
                    CycleDirection::Backward => self.condition.cycle_tag_name_prev(),
                };
            }
            FilterFieldFocus::TagComparison => {
                self.condition.tag_comparison = match direction {
                    CycleDirection::Forward => self.condition.tag_comparison.next(),
                    CycleDirection::Backward => self.condition.tag_comparison.prev(),
                };
            }
            _ => {}
        }
    }

    /// Handle character input.
    fn handle_char(&mut self, c: char) {
        match self.focus {
            FilterFieldFocus::RangeMin => {
                if c.is_ascii_digit() {
                    self.condition.range_min.push(c);
                }
            }
            FilterFieldFocus::RangeMax => {
                if c.is_ascii_digit() {
                    self.condition.range_max.push(c);
                }
            }
            FilterFieldFocus::PathSubstring => {
                self.condition.path_substring.push(c);
            }
            FilterFieldFocus::TagValue => {
                self.condition.tag_value.push(c);
            }
            _ => {}
        }
    }

    /// Handle backspace.
    fn handle_backspace(&mut self) {
        match self.focus {
            FilterFieldFocus::RangeMin => {
                self.condition.range_min.pop();
            }
            FilterFieldFocus::RangeMax => {
                self.condition.range_max.pop();
            }
            FilterFieldFocus::PathSubstring => {
                self.condition.path_substring.pop();
            }
            FilterFieldFocus::TagValue => {
                self.condition.tag_value.pop();
            }
            _ => {}
        }
    }

    // =========================================================================
}
