//! UI Types and Traits
//!
//! Core types for UI state management.

/// Trait for progress states that display daemon statistics.
pub(crate) trait ProgressStatsUpdater {
    fn set_db_queue_depth(&mut self, depth: u64);
}

impl ProgressStatsUpdater for crate::progress_screen::ProgressScreen {
    fn set_db_queue_depth(&mut self, depth: u64) {
        crate::progress_screen::ProgressScreen::set_db_queue_depth(self, depth);
    }
}
