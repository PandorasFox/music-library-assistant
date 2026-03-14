//! Tag Search Types
//!
//! Re-exports shared search types from mm-ui and adds TUI-specific types.

use mm_meta::db_types::AudioFile;

// Re-export shared types from mm-ui.
pub use mm_ui::domain_types::{
    ComparisonOperator, ConditionType, LogicalOperator, SearchCondition, TagSearchMode,
    SEARCHABLE_TAGS,
};

/// Domain actions returned from tag search key handling.
///
/// CycleNext/CyclePrev are handled centrally. Cancel is a domain action here
/// because it has multi-modal behavior (dismiss modal, exit results, exit search).
#[derive(Debug, Clone)]
pub enum TagSearchAction {
    /// Cancel tag search (Escape when nothing to cancel).
    Cancel,
    /// Execute the search query (requires db access).
    ExecuteSearch,
    /// Open tag editor for a single audio file.
    EditAudioFile(AudioFile),
}

/// Modal dialogs for tag search.
#[derive(Debug, Clone)]
pub enum TagSearchModal {
    /// No results found after search
    NoResults,
    /// Gathering tags for bulk edit (shown briefly before editor opens)
    GatheringTags,
}
