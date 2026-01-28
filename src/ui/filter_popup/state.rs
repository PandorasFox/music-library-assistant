//! Filter Popup State
//!
//! State management for the filter popup overlay.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ui::tag_search::{ConditionType, FileTypeCategory};

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

/// State for a single filter condition.
#[derive(Debug, Clone, Default)]
pub struct FilterCondition {
    /// Type of filter (FileType, SampleRate, Bitrate, Duration).
    pub condition_type: ConditionType,
    /// File type category (for FileType conditions).
    pub file_type_category: FileTypeCategory,
    /// Range minimum (for range conditions).
    pub range_min: String,
    /// Range maximum (for range conditions).
    pub range_max: String,
}

impl FilterCondition {
    /// Create a new empty filter condition.
    pub fn new() -> Self {
        Self {
            condition_type: ConditionType::FileType, // Default to file type
            ..Default::default()
        }
    }

    /// Check if this filter has any active constraints.
    pub fn is_active(&self) -> bool {
        match self.condition_type {
            ConditionType::Tag => false, // Tag not used in filter popup
            ConditionType::FileType => self.file_type_category != FileTypeCategory::Any,
            ConditionType::SampleRate | ConditionType::Bitrate | ConditionType::Duration => {
                !self.range_min.is_empty() || !self.range_max.is_empty()
            }
        }
    }

    /// Check if a track matches this filter condition.
    pub fn matches(&self, file_type: &str, sample_rate: Option<i32>, bitrate_kbps: Option<i32>, duration_ms: Option<i64>) -> bool {
        match self.condition_type {
            ConditionType::Tag => true, // Tag not used in filter popup
            ConditionType::FileType => self.file_type_category.matches(file_type),
            ConditionType::SampleRate => {
                let value = sample_rate.unwrap_or(0) as i64;
                Self::in_range(value, &self.range_min, &self.range_max)
            }
            ConditionType::Bitrate => {
                let value = bitrate_kbps.unwrap_or(0) as i64;
                Self::in_range(value, &self.range_min, &self.range_max)
            }
            ConditionType::Duration => {
                let value = duration_ms.unwrap_or(0) / 1000; // Convert to seconds
                Self::in_range(value, &self.range_min, &self.range_max)
            }
        }
    }

    /// Check if a value is within the range.
    fn in_range(value: i64, min_str: &str, max_str: &str) -> bool {
        let min = min_str.parse::<i64>().unwrap_or(i64::MIN);
        let max = max_str.parse::<i64>().unwrap_or(i64::MAX);
        value >= min && value <= max
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

    /// Create with an existing filter condition.
    pub fn with_condition(condition: FilterCondition) -> Self {
        Self {
            condition,
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

            KeyCode::Left | KeyCode::Right => {
                self.handle_cycle_key();
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
                    ConditionType::FileType => FilterFieldFocus::FileTypeCategory,
                    _ => FilterFieldFocus::RangeMin,
                }
            }
            FilterFieldFocus::FileTypeCategory => FilterFieldFocus::ApplyButton,
            FilterFieldFocus::RangeMin => FilterFieldFocus::RangeMax,
            FilterFieldFocus::RangeMax => FilterFieldFocus::ApplyButton,
            FilterFieldFocus::ApplyButton => FilterFieldFocus::ClearButton,
            FilterFieldFocus::ClearButton => FilterFieldFocus::ConditionType,
        };
    }

    /// Move focus to the previous field.
    fn focus_prev(&mut self) {
        self.focus = match self.focus {
            FilterFieldFocus::ConditionType => FilterFieldFocus::ClearButton,
            FilterFieldFocus::FileTypeCategory => FilterFieldFocus::ConditionType,
            FilterFieldFocus::RangeMin => FilterFieldFocus::ConditionType,
            FilterFieldFocus::RangeMax => FilterFieldFocus::RangeMin,
            FilterFieldFocus::ApplyButton => {
                match self.condition.condition_type {
                    ConditionType::FileType => FilterFieldFocus::FileTypeCategory,
                    _ => FilterFieldFocus::RangeMax,
                }
            }
            FilterFieldFocus::ClearButton => FilterFieldFocus::ApplyButton,
        };
    }

    /// Handle left/right for cycling options.
    fn handle_cycle_key(&mut self) {
        match self.focus {
            FilterFieldFocus::ConditionType => {
                // Cycle through non-Tag condition types
                self.condition.condition_type = match self.condition.condition_type {
                    ConditionType::Tag => ConditionType::FileType,
                    ConditionType::FileType => ConditionType::SampleRate,
                    ConditionType::SampleRate => ConditionType::Bitrate,
                    ConditionType::Bitrate => ConditionType::Duration,
                    ConditionType::Duration => ConditionType::FileType,
                };
            }
            FilterFieldFocus::FileTypeCategory => {
                self.condition.file_type_category = self.condition.file_type_category.next();
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
            _ => {}
        }
    }
}
