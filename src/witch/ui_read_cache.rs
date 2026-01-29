//! Background cache for UI read queries.
//!
//! This module provides demand-driven, throttled caching for expensive DB queries
//! that the UI needs. Instead of blocking the UI thread with synchronous queries,
//! UI components flag "I want this data" and the Witch spawns background refreshes
//! on the rayon pool.
//!
//! ## Usage Pattern
//!
//! ```ignore
//! // UI code (every frame when data is needed):
//! witch.ui_read_cache().want_corpus_summary();
//!
//! // Render code (reads latest cached value, never blocks):
//! let summary = witch.ui_read_cache().corpus_summary();
//! ```
//!
//! ## Design
//!
//! - **Demand-driven**: No refresh unless UI explicitly wants the data
//! - **Throttled**: Configurable minimum interval between refreshes (default 15s)
//! - **Non-blocking**: UI never waits; reads whatever is cached (may be stale)
//! - **Generic**: `CacheEntry<T>` template makes adding new cached values mechanical

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::config;
use crate::corpus::db::types::{CorpusSummary, InsightsData};
use crate::corpus::db::Database;
use crate::ui::deploy_flow::DeployModalData;

// ============================================================================
// CacheEntry<T> - Generic cache slot
// ============================================================================

/// A single cached value with demand-driven background refresh.
///
/// Thread-safe: UI thread reads, worker threads write.
pub struct CacheEntry<T: Clone + Send + Sync + 'static> {
    shared: Arc<CacheShared<T>>,
    throttle: Duration,
}

struct CacheShared<T> {
    /// UI has flagged that it wants this data refreshed.
    wanted: AtomicBool,
    /// A refresh task is currently in-flight.
    refreshing: AtomicBool,
    /// The cached data and timestamp.
    inner: RwLock<CacheInner<T>>,
}

struct CacheInner<T> {
    /// The cached data, if ever computed.
    data: Option<T>,
    /// When the data was last refreshed. None if never refreshed.
    /// Used for throttle checks in want().
    updated_at: Option<Instant>,
}

impl<T: Clone + Send + Sync + 'static> CacheEntry<T> {
    /// Create a new cache entry with the given throttle duration.
    ///
    /// The throttle prevents refresh spam - want() calls are ignored if
    /// a refresh completed less than `throttle` ago.
    pub fn new(throttle: Duration) -> Self {
        Self {
            shared: Arc::new(CacheShared {
                wanted: AtomicBool::new(false),
                refreshing: AtomicBool::new(false),
                inner: RwLock::new(CacheInner {
                    data: None,
                    updated_at: None,
                }),
            }),
            throttle,
        }
    }

    /// UI calls this to express demand for this data.
    ///
    /// Idempotent and cheap. Sets the `wanted` flag if:
    /// 1. Not already refreshing (in-flight check)
    /// 2. Either never refreshed, or last refresh was >= throttle ago
    ///
    /// The Witch's tick() will notice the flag and spawn a refresh task.
    pub fn want(&self) {
        // Fast path: already refreshing, no need to check timestamp
        if self.shared.refreshing.load(Ordering::Relaxed) {
            return;
        }

        // Check throttle (requires lock, but only briefly)
        if let Ok(inner) = self.shared.inner.read() {
            if let Some(updated_at) = inner.updated_at {
                if updated_at.elapsed() < self.throttle {
                    return; // Too fresh, ignore
                }
            }
        }

        // Set wanted flag for daemon to pick up
        self.shared.wanted.store(true, Ordering::Relaxed);
    }

    /// Invalidate the cache, bypassing the throttle.
    ///
    /// Clears `updated_at` so the next `want()` call will trigger a refresh
    /// regardless of how recently the data was updated. Use this after
    /// mutations complete to ensure fresh data on next view.
    pub fn invalidate(&self) {
        if let Ok(mut inner) = self.shared.inner.write() {
            inner.updated_at = None;
        }
        // Also set wanted so refresh happens immediately
        self.shared.wanted.store(true, Ordering::Relaxed);
    }

    /// Read the latest cached value. Never blocks (briefly for RwLock).
    ///
    /// Returns None if the data has never been computed.
    pub fn get(&self) -> Option<T> {
        self.shared
            .inner
            .read()
            .ok()
            .and_then(|inner| inner.data.clone())
    }

    /// The Witch calls this to check if a refresh is needed.
    ///
    /// Returns a `CacheWriter` handle if:
    /// - wanted flag is set
    /// - not already refreshing
    ///
    /// The caller should spawn a worker task that computes the value
    /// and calls `writer.complete(data)` or `writer.abort()`.
    pub(crate) fn take_refresh(&self) -> Option<CacheWriter<T>> {
        // Atomically clear wanted and check if we should proceed
        if !self.shared.wanted.swap(false, Ordering::Relaxed) {
            return None; // Not wanted
        }

        // Try to claim the refreshing slot
        if self
            .shared
            .refreshing
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return None; // Already refreshing
        }

        // Return writer handle
        Some(CacheWriter {
            shared: Arc::clone(&self.shared),
        })
    }
}

// ============================================================================
// CacheWriter<T> - Worker completion handle
// ============================================================================

/// Handle for workers to complete or abort a cache refresh.
///
/// Must be consumed by calling either `complete()` or `abort()`.
/// Dropping without calling either will clear the refreshing flag
/// (equivalent to abort).
pub struct CacheWriter<T> {
    shared: Arc<CacheShared<T>>,
}

impl<T> CacheWriter<T> {
    /// Complete the refresh with computed data.
    ///
    /// - Sets data to Some(value)
    /// - Sets updated_at to now
    /// - Clears refreshing flag
    pub fn complete(self, data: T) {
        if let Ok(mut inner) = self.shared.inner.write() {
            inner.data = Some(data);
            inner.updated_at = Some(Instant::now());
        }
        self.shared.refreshing.store(false, Ordering::Relaxed);
    }

    /// Abort the refresh (e.g., on error).
    ///
    /// - Does NOT update data or timestamp
    /// - Clears refreshing flag (allowing retry on next want)
    pub fn abort(self) {
        self.shared.refreshing.store(false, Ordering::Relaxed);
    }
}

impl<T> Drop for CacheWriter<T> {
    fn drop(&mut self) {
        // Safety net: if dropped without complete/abort, clear refreshing
        // This prevents permanent "stuck refreshing" state on panic
        self.shared.refreshing.store(false, Ordering::Relaxed);
    }
}

// ============================================================================
// UiReadCache - Container for all cached UI data
// ============================================================================

/// Background cache for UI read queries.
///
/// Owned by the Witch. UI components call `want_*()` methods to flag demand,
/// and `*()` methods to read cached values.
pub struct UiReadCache {
    corpus_summary: CacheEntry<CorpusSummary>,
    insights_data: CacheEntry<InsightsData>,
    deploy_modal_data: CacheEntry<DeployModalData>,
}

impl UiReadCache {
    /// Default throttle duration for cache entries (15 seconds).
    const DEFAULT_THROTTLE: Duration = Duration::from_secs(15);
    /// Insights data throttle (30 seconds - heavier computation).
    const INSIGHTS_THROTTLE: Duration = Duration::from_secs(30);
    /// Deploy modal data throttle (30 seconds - heavier computation, multiple queries).
    const DEPLOY_MODAL_THROTTLE: Duration = Duration::from_secs(30);

    /// Create a new UI read cache with default throttle settings.
    pub fn new() -> Self {
        Self {
            corpus_summary: CacheEntry::new(Self::DEFAULT_THROTTLE),
            insights_data: CacheEntry::new(Self::INSIGHTS_THROTTLE),
            deploy_modal_data: CacheEntry::new(Self::DEPLOY_MODAL_THROTTLE),
        }
    }

    // -------------------------------------------------------------------------
    // Corpus Summary
    // -------------------------------------------------------------------------

    /// UI calls this when it wants corpus summary data.
    ///
    /// Idempotent - safe to call every frame. Respects throttle.
    pub fn want_corpus_summary(&self) {
        self.corpus_summary.want();
    }

    /// Read the latest cached corpus summary.
    ///
    /// Returns None if never computed. Never blocks.
    pub fn corpus_summary(&self) -> Option<CorpusSummary> {
        self.corpus_summary.get()
    }

    // -------------------------------------------------------------------------
    // Insights Data
    // -------------------------------------------------------------------------

    /// UI calls this when it wants insights data.
    ///
    /// Idempotent - safe to call every frame. Respects throttle.
    pub fn want_insights_data(&self) {
        self.insights_data.want();
    }

    /// Read the latest cached insights data.
    ///
    /// Returns None if never computed. Never blocks.
    pub fn insights_data(&self) -> Option<InsightsData> {
        self.insights_data.get()
    }

    /// Invalidate insights data cache, forcing refresh on next want().
    ///
    /// Call this after mutations complete to ensure fresh data when
    /// transitioning to insights view.
    pub fn invalidate_insights_data(&self) {
        self.insights_data.invalidate();
    }

    // -------------------------------------------------------------------------
    // Deploy Modal Data
    // -------------------------------------------------------------------------

    /// UI calls this when it wants deploy modal data.
    ///
    /// Idempotent - safe to call every frame. Respects throttle.
    pub fn want_deploy_modal_data(&self) {
        self.deploy_modal_data.want();
    }

    /// Read the latest cached deploy modal data.
    ///
    /// Returns None if never computed. Never blocks.
    pub fn deploy_modal_data(&self) -> Option<DeployModalData> {
        self.deploy_modal_data.get()
    }

    /// Force-trigger a refresh of deploy modal data.
    ///
    /// Called by the Witch after Content computations complete to pre-warm the cache.
    /// Bypasses the normal throttle since we know the data just changed.
    pub fn warm_deploy_modal_data(&self) {
        self.deploy_modal_data.want();
    }

    // -------------------------------------------------------------------------
    // Witch Integration
    // -------------------------------------------------------------------------

    /// The Witch calls this in tick() to spawn refresh tasks for flagged entries.
    ///
    /// Spawns rayon tasks for any entries that need refreshing.
    pub(crate) fn spawn_refreshes(&self) {
        // Corpus summary refresh
        if let Some(writer) = self.corpus_summary.take_refresh() {
            rayon::spawn(move || {
                // Open fresh read-only connection on worker thread
                if let Ok(db_path) = config::get_db_path() {
                    if let Ok(db) = Database::open_read_only(&db_path) {
                        if let Ok(summary) = db.get_corpus_summary() {
                            writer.complete(summary);
                            return;
                        }
                    }
                }
                // On error, abort (allows retry on next want)
                writer.abort();
            });
        }

        // Insights data refresh
        if let Some(writer) = self.insights_data.take_refresh() {
            rayon::spawn(move || {
                // Open fresh read-only connection on worker thread
                if let Ok(db_path) = config::get_db_path() {
                    if let Ok(db) = Database::open_read_only(&db_path) {
                        if let Ok(data) = db.get_insights_data() {
                            writer.complete(data);
                            return;
                        }
                    }
                }
                // On error, abort (allows retry on next want)
                writer.abort();
            });
        }

        // Deploy modal data refresh
        if let Some(writer) = self.deploy_modal_data.take_refresh() {
            rayon::spawn(move || {
                // Open fresh read-only connection on worker thread
                if let Ok(db_path) = config::get_db_path() {
                    if let Ok(db) = Database::open_read_only(&db_path) {
                        if let Ok(data) = DeployModalData::load(&db) {
                            writer.complete(data);
                            return;
                        }
                    }
                }
                // On error, abort (allows retry on next want)
                writer.abort();
            });
        }
    }
}

impl Default for UiReadCache {
    fn default() -> Self {
        Self::new()
    }
}
