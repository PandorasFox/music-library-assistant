//! Tag Search Types
//!
//! Re-exports shared search types from mm-ui and adds TUI-specific types.

// Re-export shared types from mm-ui.
pub use mm_ui::domain_types::{ConditionType, SearchCondition, TagSearchMode};

/// Domain actions returned from tag search key handling.
#[derive(Debug, Clone)]
pub enum TagSearchAction {
    /// Cancel tag search (Escape when nothing to cancel).
    Cancel,
    /// Execute the search query (requires server-side query).
    ExecuteSearch,
    /// Open tag editor for a single audio file by inode.
    EditAudioFile(i64),
    /// Bulk edit all result inodes.
    BulkEdit(Vec<i64>),
}

/// Modal dialogs for tag search.
#[derive(Debug, Clone)]
pub enum TagSearchModal {
    /// No results found after search
    NoResults,
    /// Gathering tags for bulk edit (shown briefly before editor opens)
    GatheringTags,
}
