//! UI Types and Traits
//!
//! Core types for UI state management including mode enum, modal states,
//! and traits for progress statistics updates.

use crate::db_thread::DbThreadStats;
use crate::daemon::WorkerStats;

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

impl ProgressStatsUpdater for crate::ui::splash_screen::SplashScreen {
    fn set_db_queue_depth(&mut self, depth: u64) {
        crate::ui::splash_screen::SplashScreen::set_db_queue_depth(self, depth);
    }
    fn set_db_stats(&mut self, stats: Option<DbThreadStats>) {
        crate::ui::splash_screen::SplashScreen::set_db_stats(self, stats);
    }
    fn set_worker_stats(&mut self, stats: Option<WorkerStats>) {
        crate::ui::splash_screen::SplashScreen::set_worker_stats(self, stats);
    }
}

impl ProgressStatsUpdater for crate::ui::startup::ContentAnalysisProgress {
    fn set_db_queue_depth(&mut self, depth: u64) {
        crate::ui::startup::ContentAnalysisProgress::set_db_queue_depth(self, depth);
    }
    fn set_db_stats(&mut self, stats: Option<DbThreadStats>) {
        crate::ui::startup::ContentAnalysisProgress::set_db_stats(self, stats);
    }
    fn set_worker_stats(&mut self, stats: Option<WorkerStats>) {
        crate::ui::startup::ContentAnalysisProgress::set_worker_stats(self, stats);
    }
}

impl ProgressStatsUpdater for crate::ui::startup::IntakeConfirmationState {
    fn set_db_queue_depth(&mut self, depth: u64) {
        crate::ui::startup::IntakeConfirmationState::set_db_queue_depth(self, depth);
    }
    fn set_db_stats(&mut self, stats: Option<DbThreadStats>) {
        crate::ui::startup::IntakeConfirmationState::set_db_stats(self, stats);
    }
    fn set_worker_stats(&mut self, stats: Option<WorkerStats>) {
        crate::ui::startup::IntakeConfirmationState::set_worker_stats(self, stats);
    }
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
#[allow(dead_code)]
pub(crate) enum UiMode {
    /// Content analysis progress screen - post-intake health signal computation
    ContentAnalysis,
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
    /// Loading splash screen - centered eye with status message
    LoadingSplash,
    /// Tag search with query builder and results (part of lateral view ring)
    TagSearch,
    /// Unified tag editor with transaction support (replaces TagEditor and DirectoryTagEditor)
    UnifiedTagEditor,
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
