//! Witch state machine types — published snapshots for protocol responses.
//!
//! These types represent the Witch's observable state. They cross the protocol
//! boundary (Serialize/Deserialize) and carry no server-side logic.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::decisions::{DecisionKey, DecisionKeyKind};

// ============================================================================
// Startup State
// ============================================================================

/// Witch startup state — lifecycle from boot to fully operational.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceProgress {
    pub total: usize,
    pub processed: usize,
    pub matched: usize,
    pub no_match: usize,
    pub retries: usize,
}

/// Combined progress for both external sources.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FetchProgress {
    pub acoustid: SourceProgress,
    pub mb: SourceProgress,
    /// Current effective AcoustID requests/sec (from rate limiter).
    pub acoustid_rps: f32,
    /// Current effective MB requests/sec (from adaptive rate limiter).
    pub mb_rps: f32,
}

// ============================================================================
// Transaction Snapshot
// ============================================================================

/// Lightweight snapshot of the active transaction for status reads.
///
/// Contains enough data for render code (titlebar, insights view) without
/// the heavy mutation/diff data. For full decision details (transaction
/// review view), use a dedicated command query.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
