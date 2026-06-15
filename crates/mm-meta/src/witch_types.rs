//! Witch state machine types — published snapshots for protocol responses.
//!
//! These types represent the Witch's observable state. They cross the protocol
//! boundary (Serialize/Deserialize) and carry no server-side logic.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::decisions::{DecisionKey, DecisionKeyKind};

// ============================================================================
// Startup State
// ============================================================================

/// Witch startup state — lifecycle from boot to fully operational.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum WitchStartupState {
    /// No database — waiting for client to provide setup payload.
    AwaitingSetup,
    /// Schema reconciliation auto-running.
    Reconciling,
    /// Database vacuum auto-running.
    Vacuuming,
    /// Fully operational.
    #[default]
    Ready,
}

// ============================================================================
// Reasoning Level
// ============================================================================

/// Reasoning level - how much signal derivation the Witch has completed.
///
/// Gates the overall UI mode:
/// - None/Inodes → Splash screen
/// - Full → Normal UI with blinking eye
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ReasoningLevel {
    /// Startup — no reasoning yet.
    #[default]
    None,
    /// Inode-level signal derivation in progress.
    Inodes,
    /// Full reasoning available, mutations accepted.
    Full,
}

// ============================================================================
// Watcher State
// ============================================================================

/// Watcher state — tracks filesystem watcher thread progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum WatcherState {
    /// Watcher spawned but not yet started.
    #[default]
    NotStarted,
    /// Initial directory walk in progress (watcher scanning zone roots).
    InitialScan,
    /// Watcher active and monitoring. Initial scan complete.
    Watching,
    /// inotify unavailable — watcher periodically re-walks zones.
    Polling,
}

// ============================================================================
// Work Status Types
// ============================================================================

/// Status information returned from tick() and status().
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkStatus {
    /// Current high-level state snapshot.
    pub state: WorkStateSnapshot,
    /// Tasks waiting to be processed (in queue or in-flight).
    pub pending: usize,
    /// Total tasks processed in current session.
    pub total_processed: usize,
    /// Total tasks queued in current session (for progress: processed/queued).
    pub session_queued: usize,
    /// Breakdown of pending tasks by type label.
    pub pending_by_label: HashMap<String, usize>,
}

/// Snapshot of Witch work state for status reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum WorkStateSnapshot {
    #[default]
    Idle,
    Working,
    Done,
}

// ============================================================================
// External Fetch Progress
// ============================================================================

/// Per-source progress snapshot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourceProgress {
    pub total: usize,
    pub processed: usize,
    pub matched: usize,
    pub no_match: usize,
    pub retries: usize,
}

/// Combined progress for all three external sources.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FetchProgress {
    pub acoustid: SourceProgress,
    pub mb: SourceProgress,
    pub discogs: SourceProgress,
    /// Current effective AcoustID requests/sec (from rate limiter).
    pub acoustid_rps: f32,
    /// Current effective MB requests/sec (from adaptive rate limiter).
    pub mb_rps: f32,
    /// Current effective Discogs requests/sec (from adaptive rate limiter).
    pub discogs_rps: f32,
    /// False when the Discogs queue is intentionally inert (no token configured).
    /// The TUI uses this to render a clear "not configured" hint instead of a
    /// stuck-at-zero progress line.
    pub discogs_enabled: bool,
}

/// Progress snapshot for Cover Art Archive fetching.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CoverArtProgress {
    /// Total releases queued for art fetching.
    pub total_releases: usize,
    /// Releases processed so far.
    pub processed: usize,
    /// New sidecar images written to corpus.
    pub images_written: usize,
    /// Images skipped (already present, or no art available).
    pub images_skipped: usize,
    /// Existing images replaced with higher-resolution versions.
    pub images_upgraded: usize,
}

/// Progress snapshot for Deezer ISRC-keyed cover art fetching.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DeezerProgress {
    /// Total album dirs queued for Deezer lookup.
    pub total_dirs: usize,
    /// Dirs processed so far.
    pub processed: usize,
    /// New cover sidecars written to corpus.
    pub images_written: usize,
    /// Dirs where Deezer had no track for the ISRC (sticky-cached).
    pub isrc_not_found: usize,
    /// Dirs that errored out during the lookup or download.
    pub errors: usize,
}

// ============================================================================
// Transaction Snapshot
// ============================================================================

/// Lightweight snapshot of the active transaction for status reads.
///
/// Contains enough data for render code (titlebar, insights view) without
/// the heavy mutation/diff data. For full decision details (transaction
/// review view), use a dedicated command query.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TransactionSnapshot {
    /// Human-readable label for the transaction.
    pub label: String,
    /// Number of decisions staged.
    pub decision_count: usize,
    /// Total mutations across all staged decisions.
    pub mutation_count: usize,
    /// All decision keys in the transaction.
    pub decision_keys: Vec<DecisionKey>,
    /// Per-decision labels, keyed by decision key.
    pub decision_labels: Vec<(DecisionKey, String)>,
}

// ============================================================================
// WitchStatus — Comprehensive State Machine Snapshot
// ============================================================================

/// The Witch's complete observable state, published as a protocol response.
///
/// This is the single source of truth for clients reading the Witch's state.
/// Returned synchronously by the Witch when a client sends `StatusQuery`.
///
/// Generation counters allow event detection by diffing between frames:
/// - `mutations_generation`: increments when a mutation batch completes
/// - `computations_generation`: increments when a computation batch completes
/// - `error_generation`: increments when a task error occurs
/// - `config_generation`: increments when config is mutated
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WitchStatus {
    // -- Startup state --
    /// Whether the Witch is operational or awaiting first-time setup.
    pub startup_state: WitchStartupState,

    // -- Work state --
    /// Task processing state (idle/working/done + counts).
    pub work: WorkStatus,
    /// Current reasoning level (None → Inodes → Full).
    pub reasoning_level: ReasoningLevel,
    /// Whether any tasks are in-flight or queued.
    pub has_pending: bool,
    /// Whether initial corpus scanning is still in progress.
    pub is_initial_scanning: bool,
    /// Number of tasks queued in the db_thread write queue.
    pub db_queue_depth: u64,

    // -- Transaction state --
    /// Active transaction snapshot, if any.
    pub transaction: Option<TransactionSnapshot>,
    /// Decision key kinds with staged decisions (for insight view filtering).
    pub handled_decision_kinds: HashSet<DecisionKeyKind>,

    // -- External fetch state --
    /// Whether the external fetch scheduler is currently running.
    pub is_external_fetch_active: bool,
    /// Progress snapshot from external fetch (AcoustID + MusicBrainz).
    pub external_fetch_progress: Option<FetchProgress>,
    /// Whether an AcoustID API key is configured.
    pub has_acoustid_api_key: bool,

    // -- Cover art fetch state --
    /// Whether a cover art fetch is currently running.
    pub is_cover_art_fetch_active: bool,
    /// Progress snapshot from cover art fetch.
    pub cover_art_progress: Option<CoverArtProgress>,

    // -- Deezer art fetch state --
    /// Whether a Deezer ISRC-keyed cover art fetch is currently running.
    pub is_deezer_fetch_active: bool,
    /// Progress snapshot from Deezer fetch.
    pub deezer_progress: Option<DeezerProgress>,

    // -- Generation counters (for event detection via frame diffing) --
    /// Increments each time a mutation batch completes.
    pub mutations_generation: u64,
    /// Increments each time a computation batch completes.
    pub computations_generation: u64,
    /// Most recent task error message, if any.
    pub last_error: Option<String>,
    /// Increments each time a new task error occurs.
    pub error_generation: u64,
    /// Increments each time config is mutated.
    pub config_generation: u64,
}

// ============================================================================
// WitchEvent — Server-Pushed State Changes
// ============================================================================

/// Event pushed from the Witch to all connected clients.
///
/// Replaces status polling: the Witch broadcasts after any meaningful
/// state change in its run loop. Clients diff generation counters in the
/// contained `WitchStatus` to detect what changed — same logic as before,
/// just push-driven instead of poll-driven.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum WitchEvent {
    /// Full status snapshot after a state change.
    StatusChanged(WitchStatus),
}
