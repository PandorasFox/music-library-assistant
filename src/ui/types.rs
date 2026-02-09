//! UI Types and Traits
//!
//! Core types for UI state management.

use crate::db_thread::DbThreadStats;
use crate::witch::WorkerStats;

/// Trait for progress states that display daemon statistics.
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
