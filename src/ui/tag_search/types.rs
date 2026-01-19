//! Tag Search Types
//!
//! Types for the tag search query builder and results.

use crate::corpus::db::Track;

/// Logical operators for combining search conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogicalOperator {
    #[default]
    And,
    Or,
    Xor,
}

impl LogicalOperator {
    /// Display label for the operator.
    pub fn label(&self) -> &'static str {
        match self {
            LogicalOperator::And => "AND",
            LogicalOperator::Or => "OR",
            LogicalOperator::Xor => "XOR",
        }
    }

    /// Cycle to the next operator.
    pub fn next(&self) -> Self {
        match self {
            LogicalOperator::And => LogicalOperator::Or,
            LogicalOperator::Or => LogicalOperator::Xor,
            LogicalOperator::Xor => LogicalOperator::And,
        }
    }
}

/// Comparison operators for tag value matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComparisonOperator {
    /// Exact match (case-insensitive) - default
    #[default]
    Is,
    /// Negated exact match
    Not,
    /// Substring match (case-insensitive)
    Contains,
    /// SQL LIKE pattern (% and _ wildcards)
    Like,
}

impl ComparisonOperator {
    /// Display label for the operator.
    pub fn label(&self) -> &'static str {
        match self {
            ComparisonOperator::Is => "IS",
            ComparisonOperator::Not => "NOT",
            ComparisonOperator::Contains => "CONTAINS",
            ComparisonOperator::Like => "LIKE",
        }
    }

    /// Cycle to the next operator.
    pub fn next(&self) -> Self {
        match self {
            ComparisonOperator::Is => ComparisonOperator::Not,
            ComparisonOperator::Not => ComparisonOperator::Contains,
            ComparisonOperator::Contains => ComparisonOperator::Like,
            ComparisonOperator::Like => ComparisonOperator::Is,
        }
    }
}

/// A single search condition (tag name + comparison + value).
#[derive(Debug, Clone, Default)]
pub struct SearchCondition {
    /// Logical operator connecting to previous condition (ignored for first).
    pub operator: LogicalOperator,
    /// Tag name to search (e.g., "artist", "album", "genre").
    pub tag_name: String,
    /// How to compare the tag value.
    pub comparison: ComparisonOperator,
    /// Value to search for.
    pub value: String,
}

impl SearchCondition {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Current mode of the tag search view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TagSearchMode {
    #[default]
    QueryBuilder,
    Results,
}

/// Actions returned from tag search key handling.
#[derive(Debug, Clone)]
pub enum TagSearchAction {
    /// No action needed.
    None,
    /// Cancel tag search (Escape when nothing to cancel).
    Cancel,
    /// Cycle to next lateral view (Tab).
    CycleNext,
    /// Cycle to previous lateral view (Shift-Tab).
    CyclePrev,
    /// Execute the search query (requires db access).
    ExecuteSearch,
    /// Open tag editor for a single track.
    EditTrack(Track),
    /// Open tag editor for all results (track list navigation).
    EditAllTracks(Vec<Track>),
}

/// Modal dialogs for tag search.
#[derive(Debug, Clone)]
pub enum TagSearchModal {
    /// No results found after search
    NoResults,
    /// Gathering tags for bulk edit (shown briefly before editor opens)
    GatheringTags,
}

/// Searchable tag field names.
pub const SEARCHABLE_TAGS: &[&str] = &[
    "artist",
    "album",
    "album_artist",
    "title",
    "genre",
];
