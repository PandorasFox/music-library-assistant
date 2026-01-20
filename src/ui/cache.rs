//! UI Cache - Cached Database Results for "Lively" Data
//!
//! **IMPORTANT**: Never query the database directly from render code.
//! DB queries during render cause multi-second frame times when workers
//! are active, and create SQLite contention with write operations.
//!
//! ## When to Use UiCache
//!
//! Use this cache for **"lively" data that updates during normal operation**:
//! corpus summary, daemon status, health stats - data that changes while
//! the user watches and should be periodically refreshed.
//!
//! ## Alternative: Modal/View Init Caching
//!
//! For **data that stays fixed during interaction** (tag editor loading
//! track tags, search results), query once during modal/view initialization
//! and store in the view's state struct. That data doesn't need periodic
//! refresh - it stays constant until the modal closes.
//!
//! ```text
//! // Modal init caching (in view state):
//! impl TagEditorState {
//!     pub fn new(track_id: i64, db: &Database) -> Self {
//!         let tags = db.get_track_tags(track_id);  // Query once here
//!         Self { cached_tags: tags, ... }
//!     }
//! }
//! ```
//!
//! ## Usage
//!
//! ```text
//! // In event loop (mod.rs):
//! app.ui_cache.refresh(&mut daemon);
//!
//! // In render code:
//! let summary = app.ui_cache.corpus_summary();  // Returns cached value
//! ```
//!
//! ## Adding New Cached Values
//!
//! 1. Add field to UiCache struct with `_at: Instant` timestamp
//! 2. Add constant for refresh interval
//! 3. Add refresh logic in `refresh()` method
//! 4. Add getter method that returns cloned/copied value

use std::time::{Duration, Instant};

use crate::corpus::db::types::CorpusSummary;
use crate::daemon::TaskDaemon;

/// Cached database results for UI rendering.
///
/// All values are refreshed at controlled intervals from the event loop.
/// Render code should only read from this cache, never query the DB directly.
pub struct UiCache {
    // -------------------------------------------------------------------------
    // Corpus Summary
    // -------------------------------------------------------------------------

    /// Cached corpus summary (track counts, health stats, deployment stats)
    corpus_summary: Option<CorpusSummary>,
    corpus_summary_at: Instant,
}

impl UiCache {
    /// How often to refresh corpus summary (expensive: multiple DB queries)
    const CORPUS_SUMMARY_INTERVAL: Duration = Duration::from_secs(2);

    /// Create a new empty cache.
    pub fn new() -> Self {
        Self {
            corpus_summary: None,
            corpus_summary_at: Instant::now(),
        }
    }

    /// Refresh all stale cached values.
    ///
    /// **Call from event loop only**, not from render code.
    /// This may perform DB queries for any values past their refresh interval.
    pub fn refresh(&mut self, daemon: &mut TaskDaemon) {
        self.refresh_corpus_summary(daemon);
    }

    /// Force immediate refresh of all cached values.
    ///
    /// Use sparingly - typically after operations that change DB state significantly.
    pub fn force_refresh(&mut self, daemon: &mut TaskDaemon) {
        // Reset timestamps to force refresh
        self.corpus_summary_at = Instant::now() - Self::CORPUS_SUMMARY_INTERVAL;
        self.refresh(daemon);
    }

    // -------------------------------------------------------------------------
    // Corpus Summary
    // -------------------------------------------------------------------------

    fn refresh_corpus_summary(&mut self, daemon: &mut TaskDaemon) {
        if self.corpus_summary_at.elapsed() >= Self::CORPUS_SUMMARY_INTERVAL {
            self.corpus_summary = daemon.read_only_db().get_corpus_summary().ok();
            self.corpus_summary_at = Instant::now();
        }
    }

    /// Get cached corpus summary for rendering.
    ///
    /// Returns None if never refreshed or if DB query failed.
    pub fn corpus_summary(&self) -> Option<CorpusSummary> {
        self.corpus_summary.clone()
    }
}

impl Default for UiCache {
    fn default() -> Self {
        Self::new()
    }
}
