//! UI Types and Traits
//!
//! Core types for UI state management including mode enum, modal states,
//! and traits for progress statistics updates.

use crate::db_thread::DbThreadStats;
use crate::witch::WorkerStats;

// ============================================================================
// Progress Stats Trait
// ============================================================================

/// Trait for progress states that display daemon statistics.
///
/// Allows generic stats update logic in tick functions.
pub(crate) trait ProgressStatsUpdater {
    fn set_db_queue_depth(&mut self, depth: u64);
    fn set_db_stats(&mut self, stats: Option<DbThreadStats>);
    fn set_worker_stats(&mut self, stats: Option<WorkerStats>);
}

impl ProgressStatsUpdater for crate::ui::progress_screen::ProgressScreen {
    fn set_db_queue_depth(&mut self, depth: u64) {
        crate::ui::progress_screen::ProgressScreen::set_db_queue_depth(self, depth);
    }
    fn set_db_stats(&mut self, stats: Option<DbThreadStats>) {
        crate::ui::progress_screen::ProgressScreen::set_db_stats(self, stats);
    }
    fn set_worker_stats(&mut self, stats: Option<WorkerStats>) {
        crate::ui::progress_screen::ProgressScreen::set_worker_stats(self, stats);
    }
}


// ============================================================================
// UI Mode Enum
// ============================================================================

/// Current UI mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiMode {
    /// Unified progress screen - startup eyeballing, content analysis, signal refresh
    Progress,
    DirBrowser,
    DeploymentPreview,
    /// Exit confirmation modal (when operations are in progress)
    ExitConfirmModal,
    /// Corpus browser with directory tree and metadata preview
    CorpusBrowser,
    /// Full-screen insights view (part of lateral view ring)
    Insights,
    /// Intake confirmation - prompt to index unindexed files
    IntakeConfirmation,
    /// Tag search with query builder and results (part of lateral view ring)
    TagSearch,
    /// Unified tag editor with transaction support (replaces TagEditor and DirectoryTagEditor)
    UnifiedTagEditor,
    /// Missing file resolution modal
    MissingFileResolution,
    /// Tag canonicity resolution modal (artist/genre/album canonization, album_artist setting)
    TagCanonicityResolution,
    /// Compound tag split resolution modal (split "Rock; Metal" into separate values)
    CompoundTagSplit,
    /// OOB tag sync resolution modal
    OobSyncResolution,
    /// OOB tag conflict inspection modal
    OobConflictInspection,
    /// Inode changed acknowledgement modal
    InodeChangedAcknowledge,
    /// Standardized transaction review modal - all mutation flows pass through here
    TransactionReview,
    /// Format standardization view (part of lateral view ring)
    FormatStandardization,
}

// ============================================================================
// Modal States
// ============================================================================

/// State for the exit confirmation modal.
/// Default selection is "No" (stay in application).
/// Pressing Esc/Enter/Space when selected_no=true returns to main menu.
#[derive(Debug, Clone, Default)]
pub(crate) struct ExitConfirmModalState {
    /// True = "No" selected (default), False = "Yes" selected
    pub selected_no: bool,
    /// True if operations are in progress (shows warning), False for simple exit prompt
    pub has_operations: bool,
}

impl ExitConfirmModalState {
    pub fn new(has_operations: bool) -> Self {
        Self {
            selected_no: true,
            has_operations,
        }
    }
}
