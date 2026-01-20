//! Task Daemon - Core execution subsystem for MLA.
//!
//! This module is the critical junction point for all corpus mutations and
//! computations. It enforces the DecisionWitness pattern for mutations.
//!
//! NOTE: Do not modify this module without explicit authorization.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use std::sync::mpsc::{self, Receiver, Sender};

use crate::config;
use crate::corpus::computations::Computation;
use crate::corpus::db::Database;
use crate::corpus::mutations::Mutation;
use crate::db_thread::{self, DbThreadHandle, DbThreadStats};

// ============================================================================
// State Machine
// ============================================================================

/// High-level daemon state for simple O(1) checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonState {
    /// No tasks, no lingering status
    Idle,
    /// Tasks are queued/executing
    Working,
    /// All tasks complete, lingering status available (30s timeout → Idle)
    Completed,
}

// ============================================================================
// Eye and Observation State
// ============================================================================

/// Eye lifecycle state - controlled by the daemon.
///
/// The Eye's visual state gates the overall UI mode:
/// - Closed/Awakening → Splash screen
/// - Awake → Normal UI with blinking eye
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EyeState {
    /// Eye is closed - startup mode, splash screen shown.
    #[default]
    Closed,
    /// Transitioning to Awake - second-level signals being computed.
    Awakening,
    /// Eye is awake - normal operation, can blink.
    Awake,
}


/// Corpus observation state - tracks whether the corpus has been seen.
///
/// Controls whether mutations are accepted:
/// - Unseen/Lazy/Paranoid → Mutations rejected
/// - Complete → Mutations accepted
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CorpusObservationState {
    /// Initial unknown state (launch only). Corpus has never been eyeballed.
    #[default]
    Unseen,
    /// Lazy eyeballing in progress (non-paranoid, runtime re-scan).
    Lazy,
    /// Paranoid eyeballing in progress (full verification).
    Paranoid,
    /// Eyeballing complete. Ready to accept mutations.
    Complete,
}

// ============================================================================
// Task Types
// ============================================================================

/// A task that can be queued for execution.
///
/// Tasks are either Mutations (require DecisionWitness), Computations (no witness),
/// or Migrations (require DecisionWitness but bypass accepting_mutations gate).
#[derive(Debug, Clone)]
pub enum Task {
    /// A state-altering mutation (requires DecisionWitness to queue).
    Mutation(Mutation),
    /// A read-only computation that emits signals (no witness required).
    Computation(Computation),
    /// A schema migration (requires DecisionWitness but bypasses accepting_mutations).
    Migration(Migration),
}

// ============================================================================
// Migration Types
// ============================================================================

/// A database schema migration.
///
/// Migrations require DecisionWitness (user approval) but bypass the `accepting_mutations`
/// gate. They must run before indexing can happen if schema changes are required.
#[derive(Debug, Clone)]
pub struct Migration {
    /// Version number this migration starts from.
    pub from_version: u32,
    /// Version number after migration completes.
    pub to_version: u32,
    /// Human-readable description of the migration.
    pub description: String,
}

// ============================================================================
// Decision Witness (Sealed Access Control)
// ============================================================================

/// Access control for mutation queueing.
///
/// # Design Pattern: Decision Witness
///
/// `DecisionWitness` is a zero-sized proof that mutations are being queued from
/// a user-led Decision context. TaskDaemon's mutation queueing methods require
/// this token, preventing code from queueing state-altering mutations without
/// explicit operator decisions.
///
/// The token can ONLY be obtained via [`confirm_decision()`], which should be
/// called at the moment the user confirms a decision (save, deploy, etc.).
///
/// Computations (health signals, verification) use separate queue methods
/// that do NOT require a witness, as they are decisionless by design.
pub mod sealed {
    /// A zero-sized token proving mutations come from a user-led Decision context.
    ///
    /// Cannot be constructed outside [`confirm_decision()`].
    #[derive(Clone, Copy)]
    pub struct DecisionWitness(());

    impl DecisionWitness {
        /// Internal constructor - only callable from confirm_decision()
        pub(super) fn new() -> Self {
            Self(())
        }
    }

    /// A zero-sized token proving code is executing inside TaskDaemon's mutation worker.
    ///
    /// All index-mutating database functions require this witness, ensuring they
    /// can only be called from within the daemon's execution context.
    ///
    /// Cannot be constructed outside the daemon's `execute_mutation()` function.
    #[derive(Clone, Copy)]
    pub struct MutationExecutionWitness(());

    impl MutationExecutionWitness {
        /// Internal constructor - only callable from execute_mutation()
        pub(super) fn new() -> Self {
            Self(())
        }
    }

    /// A zero-sized token proving code is executing inside TaskDaemon's migration worker.
    ///
    /// Migration apply functions require this witness, ensuring they can only be
    /// called from within the daemon's `execute_migration()` function.
    ///
    /// Cannot be constructed outside the daemon's `execute_migration()` function.
    #[derive(Clone, Copy)]
    pub struct MigrationWitness(());

    impl MigrationWitness {
        /// Internal constructor - only callable from execute_migration()
        pub(super) fn new() -> Self {
            Self(())
        }
    }
}

pub use sealed::DecisionWitness;
pub use sealed::MigrationWitness;
pub use sealed::MutationExecutionWitness;

/// Create a DecisionWitness, certifying that the user has confirmed a decision.
///
/// Call this at the moment of user confirmation (e.g., when user presses Enter
/// to save tag edits, or confirms deployment). The returned witness can then
/// be passed to [`TaskDaemon::add_decision`] or [`TaskDaemon::confirm_transaction`].
///
/// # Example
///
/// ```rust,ignore
/// // Start a transaction when entering a decision flow
/// daemon.start_transaction("Tag edits")?;
///
/// // User confirms edits for first item
/// let witness = confirm_decision();
/// daemon.add_decision(0, &witness, "Track A", mutations)?;
///
/// // ... user edits more items ...
///
/// // User commits the transaction
/// let commit_witness = confirm_decision();
/// daemon.confirm_transaction(&commit_witness)?;
/// ```
pub fn confirm_decision() -> DecisionWitness {
    DecisionWitness::new()
}

/// Create a MigrationWitness for startup migrations (pre-daemon context).
///
/// Call this when the user approves database migrations at startup.
/// The returned witness can then be passed to [`MigrationRegistry::apply_all_pending`].
///
/// This is separate from the daemon's `execute_migration()` context witness -
/// it's for migrations that run before the daemon exists.
pub fn confirm_startup_migration() -> MigrationWitness {
    MigrationWitness::new()
}

// ============================================================================
// Labels and Status Types
// ============================================================================

/// Human-readable task label for status display.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskLabel(pub String);

impl TaskLabel {
    pub fn new(label: impl Into<String>) -> Self {
        Self(label.into())
    }

    /// Create label from a mutation (fallback if no explicit label provided).
    pub fn from_mutation(mutation: &Mutation) -> Self {
        use crate::corpus::mutations::MutationCategory;
        let label = match mutation.category() {
            MutationCategory::TagEdit => "Tag edits",
            MutationCategory::Indexing => match mutation {
                Mutation::IndexTrack { .. } | Mutation::IndexFileFromPath { .. } => "Indexing tracks",
                Mutation::DropFromIndex { .. } => "Dropping from index",
                Mutation::UpdateTrack { .. } => "Updating tracks",
                Mutation::UpdateTrackPath { .. } => "Updating paths",
                _ => "Index operations",
            },
            MutationCategory::FileMove => "File moves",
            MutationCategory::FileCopy => "File copies",
            MutationCategory::FileDelete => "File deletions",
            MutationCategory::Deployment => "Deployment",
            MutationCategory::Migration => "Migrations",
        };
        Self(label.to_string())
    }

    /// Create label from a computation (fallback if no explicit label provided).
    pub fn from_computation(computation: &Computation) -> Self {
        Self(computation.label().to_string())
    }

    /// Create label from a migration.
    pub fn from_migration(migration: &Migration) -> Self {
        Self(format!("Migration v{} → v{}", migration.from_version, migration.to_version))
    }

    /// Create label from a task (mutation, computation, or migration).
    pub fn from_task(task: &Task) -> Self {
        match task {
            Task::Mutation(m) => Self::from_mutation(m),
            Task::Computation(c) => Self::from_computation(c),
            Task::Migration(m) => Self::from_migration(m),
        }
    }
}

/// Summary of a completed daemon session (for lingering display).
#[derive(Debug, Clone)]
pub struct CompletedSession {
    /// When the session completed
    pub completed_at: Instant,
    /// How long the session took (first queue to last complete)
    pub duration: Duration,
    /// Total tasks processed
    pub total_processed: usize,
    /// Tasks that failed
    pub failed: usize,
    /// Breakdown by task type
    pub task_counts: HashMap<String, usize>,
}

impl CompletedSession {
    /// Check if this session should still be displayed (within linger duration).
    pub fn should_display(&self, linger_duration: Duration) -> bool {
        self.completed_at.elapsed() < linger_duration
    }
}

/// Status information returned from tick().
#[derive(Debug, Clone, Default)]
pub struct DaemonStatus {
    /// Current high-level state
    pub state: DaemonStateSnapshot,
    /// Tasks waiting to be processed (in queue or in-flight)
    pub pending: usize,
    /// Tasks completed in this tick cycle
    pub completed: usize,
    /// Tasks failed in this tick cycle
    pub failed: usize,
    /// Total tasks processed in current session
    pub total_processed: usize,
    /// Total tasks queued in current session (for progress: processed/queued)
    pub session_queued: usize,
    /// Recent error messages
    pub recent_errors: Vec<String>,
    /// Breakdown of processed tasks by type label
    pub task_counts: HashMap<String, usize>,
    /// Elapsed time since first task was queued (if session active)
    pub elapsed: Option<Duration>,
    /// Completed session summary (for lingering display)
    pub completed_session: Option<CompletedSession>,
}

/// Snapshot of daemon state for status reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DaemonStateSnapshot {
    #[default]
    Idle,
    Working,
    Completed,
}

impl From<DaemonState> for DaemonStateSnapshot {
    fn from(state: DaemonState) -> Self {
        match state {
            DaemonState::Idle => DaemonStateSnapshot::Idle,
            DaemonState::Working => DaemonStateSnapshot::Working,
            DaemonState::Completed => DaemonStateSnapshot::Completed,
        }
    }
}

// ============================================================================
// Transaction Types
// ============================================================================

/// A witnessed decision with its associated pending mutations.
///
/// Decisions are accumulated in a transaction and committed together.
#[derive(Debug, Clone)]
pub struct WitnessedDecision {
    /// Human-readable label for this decision
    pub label: String,
    /// The mutations this decision will produce when committed
    pub mutations: Vec<Mutation>,
}

/// An active transaction accumulating decisions.
///
/// Transactions are ephemeral in-memory state. They are NOT persisted to disk.
/// Only one transaction may be active at a time.
#[derive(Debug)]
pub struct PendingTransaction {
    /// Human-readable label for this transaction
    pub label: String,
    /// When the transaction was started
    pub started_at: Instant,
    /// Accumulated decisions by index (UI-provided, may have gaps)
    decisions: HashMap<usize, WitnessedDecision>,
}

impl PendingTransaction {
    /// Create a new empty transaction.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            started_at: Instant::now(),
            decisions: HashMap::new(),
        }
    }

    /// Count of stored decisions.
    pub fn decision_count(&self) -> usize {
        self.decisions.len()
    }

    /// Total mutations across all decisions.
    pub fn mutation_count(&self) -> usize {
        self.decisions.values().map(|d| d.mutations.len()).sum()
    }

    /// Get all decision indices (sorted).
    pub fn indices(&self) -> Vec<usize> {
        let mut indices: Vec<_> = self.decisions.keys().copied().collect();
        indices.sort();
        indices
    }
}

/// Errors that can occur during transaction operations.
#[derive(Debug, Clone)]
pub enum TransactionError {
    /// Attempted to start a transaction when one is already active.
    AlreadyActive,
    /// Attempted an operation requiring an active transaction.
    NoActiveTransaction,
    /// Attempted to confirm transaction but daemon is not accepting mutations.
    /// This happens if eyeballing hasn't completed or read-only mode is enabled.
    NotAcceptingMutations,
}

impl std::fmt::Display for TransactionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionError::AlreadyActive => write!(f, "Transaction already active"),
            TransactionError::NoActiveTransaction => write!(f, "No active transaction"),
            TransactionError::NotAcceptingMutations => write!(f, "Not accepting mutations (eyeballing incomplete or read-only mode)"),
        }
    }
}

impl std::error::Error for TransactionError {}

/// Information about an active transaction for UI display.
#[derive(Debug, Clone)]
pub struct TransactionInfo {
    /// Human-readable label for this transaction
    pub label: String,
    /// Number of decisions accumulated
    pub decision_count: usize,
    /// Total mutations across all decisions
    pub mutation_count: usize,
    /// When the transaction was started
    pub started_at: Instant,
}

/// Summary returned when a transaction is committed.
#[derive(Debug, Clone)]
pub struct CommitSummary {
    /// Number of decisions that were committed
    pub decision_count: usize,
    /// Total mutations that were queued for execution
    pub mutation_count: usize,
}

/// Summary returned when a transaction is discarded.
#[derive(Debug, Clone)]
pub struct DiscardSummary {
    /// Number of decisions that were discarded
    pub decision_count: usize,
    /// Total mutations that were discarded
    pub mutation_count: usize,
}

// ============================================================================
// Internal Task Result
// ============================================================================

/// Result of executing a single task.
#[derive(Debug)]
struct TaskResult {
    success: bool,
    error: Option<String>,
    label: String,
    /// Follow-up computations to queue (from computation chaining)
    spawn: Vec<Computation>,
    /// Task execution duration in milliseconds
    duration_ms: u64,
    /// Time task waited in queue before execution (milliseconds)
    queue_wait_ms: u64,
    /// Snapshot of thread stats after execution (for computations)
    thread_stats: Option<crate::corpus::computations::ThreadStats>,
}

// ============================================================================
// Worker Performance Stats
// ============================================================================

/// Aggregated performance statistics from worker threads.
#[derive(Debug, Clone, Default)]
pub struct WorkerStats {
    /// Total tasks completed across all threads
    pub tasks_completed: u64,
    /// Average task execution time in milliseconds
    pub avg_task_ms: u64,
    /// Slowest single task execution time
    pub max_task_ms: u64,
    /// Label of the slowest task
    pub max_task_label: String,
    /// Average queue wait time in milliseconds
    pub queue_wait_avg_ms: u64,
    /// Maximum queue wait time
    pub queue_wait_max_ms: u64,
    /// Number of worker threads that have executed tasks
    pub active_threads: usize,
    /// Average DB read time in microseconds
    pub avg_db_read_us: u64,
    /// Median DB read time in microseconds (from recent samples)
    pub median_db_read_us: u64,
    /// Maximum DB read time in microseconds (slowest single read)
    pub max_db_read_us: u64,
    /// Total DB reads across all threads
    pub total_db_reads: u64,
}

// ============================================================================
// Shared Worker Stats (Thread-Safe)
// ============================================================================

/// Inner data for SharedWorkerStats protected by mutex.
struct WorkerStatsInner {
    max_task_label: String,
    thread_stats_map: HashMap<u64, crate::corpus::computations::ThreadStats>,
}

/// Thread-safe worker statistics, isolated from TaskDaemon struct.
///
/// This follows the proven pattern from DbThreadHandle - stats are stored in
/// a separate heap allocation via Arc, with atomic counters for numeric data
/// and a mutex for complex data (labels, thread_stats_map).
struct SharedWorkerStats {
    tasks_completed: AtomicU64,
    total_task_duration_ms: AtomicU64,
    total_queue_wait_ms: AtomicU64,
    max_task_ms: AtomicU64,
    max_queue_wait_ms: AtomicU64,
    /// Complex data behind mutex
    inner: Mutex<WorkerStatsInner>,
}

impl SharedWorkerStats {
    fn new() -> Self {
        Self {
            tasks_completed: AtomicU64::new(0),
            total_task_duration_ms: AtomicU64::new(0),
            total_queue_wait_ms: AtomicU64::new(0),
            max_task_ms: AtomicU64::new(0),
            max_queue_wait_ms: AtomicU64::new(0),
            inner: Mutex::new(WorkerStatsInner {
                max_task_label: String::new(),
                thread_stats_map: HashMap::new(),
            }),
        }
    }

    /// Atomically update max value if new value is larger.
    /// Returns true if max was updated.
    fn update_max(atomic: &AtomicU64, new_value: u64) -> bool {
        let mut current_max = atomic.load(Ordering::Relaxed);
        while new_value > current_max {
            match atomic.compare_exchange_weak(
                current_max,
                new_value,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => current_max = actual,
            }
        }
        false
    }

    /// Record a completed task result. Called from main thread's tick().
    fn record_result(&self, result: &TaskResult) {
        self.tasks_completed.fetch_add(1, Ordering::Relaxed);
        self.total_task_duration_ms.fetch_add(result.duration_ms, Ordering::Relaxed);
        self.total_queue_wait_ms.fetch_add(result.queue_wait_ms, Ordering::Relaxed);

        // Atomic max update for task duration
        if Self::update_max(&self.max_task_ms, result.duration_ms) {
            // Update label under mutex if we set a new max
            if let Ok(mut inner) = self.inner.lock() {
                inner.max_task_label = result.label.clone();
            }
        }

        // Atomic max update for queue wait
        Self::update_max(&self.max_queue_wait_ms, result.queue_wait_ms);

        // Thread stats under mutex
        if let Some(ref thread_stats) = result.thread_stats {
            if let Ok(mut inner) = self.inner.lock() {
                inner.thread_stats_map.insert(thread_stats.thread_id, thread_stats.clone());
            }
        }
    }

    /// Get a snapshot of current stats for UI display.
    fn snapshot(&self) -> WorkerStats {
        let tasks = self.tasks_completed.load(Ordering::Relaxed);
        let total_duration = self.total_task_duration_ms.load(Ordering::Relaxed);
        let total_queue = self.total_queue_wait_ms.load(Ordering::Relaxed);
        let max_task = self.max_task_ms.load(Ordering::Relaxed);
        let max_queue = self.max_queue_wait_ms.load(Ordering::Relaxed);

        let inner = self.inner.lock().unwrap();

        // Aggregate per-thread stats
        let active_threads = inner.thread_stats_map.len();
        let mut total_db_read_us = 0u64;
        let mut total_db_reads = 0u64;
        let mut max_db_read_us = 0u64;
        let mut all_samples: Vec<u64> = Vec::new();

        for stats in inner.thread_stats_map.values() {
            total_db_read_us += stats.total_db_read_us;
            total_db_reads += stats.db_read_count;
            if stats.max_db_read_us > max_db_read_us {
                max_db_read_us = stats.max_db_read_us;
            }
            // Collect samples for median calculation
            all_samples.extend_from_slice(&stats.read_samples[..stats.read_sample_count]);
        }

        let avg_db_read_us = if total_db_reads > 0 {
            total_db_read_us / total_db_reads
        } else {
            0
        };

        // Compute median from samples
        let median_db_read_us = if all_samples.is_empty() {
            0
        } else {
            all_samples.sort_unstable();
            all_samples[all_samples.len() / 2]
        };

        WorkerStats {
            tasks_completed: tasks,
            avg_task_ms: if tasks > 0 { total_duration / tasks } else { 0 },
            max_task_ms: max_task,
            max_task_label: inner.max_task_label.clone(),
            queue_wait_avg_ms: if tasks > 0 { total_queue / tasks } else { 0 },
            queue_wait_max_ms: max_queue,
            active_threads,
            avg_db_read_us,
            median_db_read_us,
            max_db_read_us,
            total_db_reads,
        }
    }

    /// Debug: get raw values for logging
    fn debug_values(&self) -> (u64, u64, u64) {
        (
            self.tasks_completed.load(Ordering::Relaxed),
            self.total_queue_wait_ms.load(Ordering::Relaxed),
            self.max_queue_wait_ms.load(Ordering::Relaxed),
        )
    }
}

// ============================================================================
// Task Daemon
// ============================================================================

/// Simple parallel task queue with state machine.
pub struct TaskDaemon {
    // Rayon-based task execution with channel for results
    result_tx: Sender<TaskResult>,
    result_rx: Receiver<TaskResult>,

    /// When this daemon instance was created.
    /// Used for signal freshness tracking (`discovered_at > launch_time` = new signal).
    launch_time: DateTime<Utc>,

    // State machine
    state: DaemonState,

    // Eye and corpus observation state
    eye_state: EyeState,
    observation_state: CorpusObservationState,

    /// Whether mutations are accepted. Only becomes true when eyeballing completes,
    /// and only if read_only_mode opinion is false. Never reverts to false.
    accepting_mutations: bool,

    /// When true, mutations are permanently disabled (read-only debug mode).
    read_only_mode: bool,

    // Session tracking
    session_start: Option<Instant>,
    session_queued: usize,
    total_processed: usize,
    total_failed: usize,

    /// Tasks that have been sent to the worker but not yet drained from results.
    /// This includes both queued tasks AND tasks currently executing on worker threads.
    /// Incremented on add_task(), decremented when result is drained from get().
    in_flight: usize,

    // Status tracking
    recent_errors: VecDeque<String>,
    task_counts: HashMap<String, usize>,
    current_label: Option<String>,

    // Completed session for lingering display
    completed_session: Option<CompletedSession>,
    completed_at: Option<Instant>,

    // Transaction state
    pending_transaction: Option<PendingTransaction>,

    /// Cached read-only database connection for UI queries.
    /// Only accessed from the main thread via `read_only_db()`.
    /// UI code should use this instead of creating direct connections.
    read_only_conn: Option<Database>,

    /// Handle to the dedicated DB write thread.
    /// Provides stats access and shutdown coordination.
    db_thread_handle: DbThreadHandle,

    // -------------------------------------------------------------------------
    // Worker Performance Stats (Thread-Safe, Isolated)
    // -------------------------------------------------------------------------

    /// Thread-safe worker stats in separate heap allocation.
    /// None when timing instrumentation is disabled.
    worker_stats_shared: Option<Arc<SharedWorkerStats>>,
}

impl TaskDaemon {
    /// Linger duration for completed session display.
    const LINGER_DURATION: Duration = Duration::from_secs(30);

    pub fn new() -> Self {
        // Use rayon's global thread pool with work-stealing for better performance.
        // Thread count from config (default: 2x logical cores for I/O-bound workloads).
        let num_threads = config::get_worker_thread_count();

        // Configure rayon's global thread pool (only first call takes effect)
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build_global();

        let _ = config::log_message(&format!(
            "[WORKER] Using rayon thread pool with {} threads",
            num_threads
        ));

        // Channel for receiving task results
        let (result_tx, result_rx) = mpsc::channel();

        // Spawn the dedicated DB write thread
        let db_thread_handle = db_thread::spawn();

        // Create isolated worker stats only when timing instrumentation is enabled
        let worker_stats_shared = if config::is_timing_enabled() {
            Some(Arc::new(SharedWorkerStats::new()))
        } else {
            None
        };

        let daemon = Self {
            result_tx,
            result_rx,
            launch_time: Utc::now(),
            state: DaemonState::Idle,
            eye_state: EyeState::Closed,
            observation_state: CorpusObservationState::Unseen,
            accepting_mutations: false,
            read_only_mode: false,
            session_start: None,
            session_queued: 0,
            total_processed: 0,
            total_failed: 0,
            in_flight: 0,
            recent_errors: VecDeque::with_capacity(5),
            task_counts: HashMap::new(),
            current_label: None,
            completed_session: None,
            completed_at: None,
            pending_transaction: None,
            read_only_conn: None,
            db_thread_handle,
            worker_stats_shared,
        };

        // DEBUG: Verify initialization (only when timing enabled)
        if let Some(ref stats) = daemon.worker_stats_shared {
            let (_tasks, total_qw, max_qw) = stats.debug_values();
            let _ = config::log_message(&format!(
                "[PERF INIT] TaskDaemon::new() - total_queue_wait_ms={}, max_queue_wait_ms={}, total_processed={}",
                total_qw, max_qw, daemon.total_processed
            ));
        }

        daemon
    }

    /// Create a new daemon with opinions applied.
    pub fn with_opinions(read_only_mode: bool) -> Self {
        let mut daemon = Self::new();
        daemon.read_only_mode = read_only_mode;
        daemon
    }

    // -------------------------------------------------------------------------
    // State Machine API
    // -------------------------------------------------------------------------

    /// O(1) state check - returns current daemon state.
    pub fn state(&self) -> DaemonState {
        self.state
    }

    /// Get the timestamp when this daemon was created.
    ///
    /// Used for signal freshness tracking: signals with `discovered_at > launch_time`
    /// are new this session.
    pub fn launch_time(&self) -> DateTime<Utc> {
        self.launch_time
    }

    // -------------------------------------------------------------------------
    // Eye and Observation State
    // -------------------------------------------------------------------------

    /// Get current eye state for UI rendering decisions.
    pub fn eye_state(&self) -> EyeState {
        self.eye_state
    }

    /// Get current corpus observation state.
    pub fn observation_state(&self) -> CorpusObservationState {
        self.observation_state
    }

    /// Check if daemon is accepting mutations.
    ///
    /// Only becomes true after first eyeballing completes, and only if
    /// read_only_mode is false. Never reverts to false.
    pub fn is_accepting_mutations(&self) -> bool {
        self.accepting_mutations
    }

    /// Check if read-only mode is enabled.
    pub fn is_read_only(&self) -> bool {
        self.read_only_mode
    }

    /// Check if eyeballing is currently in progress (Lazy or Paranoid).
    pub fn is_eyeballing(&self) -> bool {
        matches!(
            self.observation_state,
            CorpusObservationState::Lazy | CorpusObservationState::Paranoid
        )
    }

    /// Start lazy eyeballing. Returns false if eyeballing already in progress.
    ///
    /// Queues WalkCorpus computations for corpus and optional legacy library.
    pub fn start_lazy_eyeball(
        &mut self,
        corpus_root: &std::path::Path,
        legacy: Option<&std::path::Path>,
    ) -> bool {
        if self.is_eyeballing() {
            return false;
        }

        self.observation_state = CorpusObservationState::Lazy;
        self.queue_eyeballing_computations(corpus_root, legacy, false);
        true
    }

    /// Start paranoid eyeballing. Returns false if eyeballing already in progress.
    ///
    /// Queues WalkCorpus computations with paranoid=true flag.
    pub fn start_paranoid_eyeball(
        &mut self,
        corpus_root: &std::path::Path,
        legacy: Option<&std::path::Path>,
    ) -> bool {
        if self.is_eyeballing() {
            return false;
        }

        self.observation_state = CorpusObservationState::Paranoid;
        self.queue_eyeballing_computations(corpus_root, legacy, true);
        true
    }

    /// Queue eyeballing computations (internal helper).
    fn queue_eyeballing_computations(
        &mut self,
        corpus_root: &std::path::Path,
        legacy: Option<&std::path::Path>,
        paranoid: bool,
    ) {
        use crate::corpus::computations::Computation;

        // Queue corpus walk
        self.queue_computation_with_label(
            Computation::WalkCorpus {
                root: corpus_root.to_path_buf(),
                source: "corpus".to_string(),
                paranoid,
            },
            Some("Eyeballing corpus".to_string()),
        );

        // Queue legacy library walk if configured
        if let Some(legacy_path) = legacy {
            self.queue_computation_with_label(
                Computation::WalkCorpus {
                    root: legacy_path.to_path_buf(),
                    source: "legacy".to_string(),
                    paranoid,
                },
                Some("Eyeballing legacy".to_string()),
            );
        }
    }

    /// Advance internal processing. MUST be called exactly once per frame from the event loop.
    ///
    /// **IMPORTANT**: Only call from `run_app()` event loop. Use `status()` for readonly access elsewhere.
    /// Calling tick() multiple times per frame will corrupt task counting.
    ///
    /// - Drains completed task results from worker
    /// - Auto-queues any spawned follow-up computations
    /// - Updates state machine transitions
    /// - Aggregates worker performance stats (when timing enabled)
    pub fn tick(&mut self) -> DaemonStatus {
        // DEBUG: Log first tick state (only when timing enabled)
        if let Some(ref stats) = self.worker_stats_shared {
            let (_, total_qw_before, _) = stats.debug_values();
            if self.total_processed == 0 && total_qw_before != 0 {
                let _ = config::log_message(&format!(
                    "[PERF BUG] tick() called with total_processed=0 but total_queue_wait_ms={}!",
                    total_qw_before
                ));
            }
        }

        let mut completed = 0;
        let mut failed = 0;

        // Drain completed results and collect spawned computations
        let mut spawned: Vec<Computation> = Vec::new();

        while let Ok(result) = self.result_rx.try_recv() {
            // Task has completed - no longer in flight
            self.in_flight = self.in_flight.saturating_sub(1);
            self.total_processed += 1;

            // Track by task type
            *self.task_counts.entry(result.label.clone()).or_insert(0) += 1;

            // Record stats via thread-safe interface (only when timing enabled)
            if let Some(ref stats) = self.worker_stats_shared {
                stats.record_result(&result);

                // DEBUG: Log queue wait values
                let (_, total_qw_now, max_qw_now) = stats.debug_values();
                if self.total_processed == 1 {
                    let _ = config::log_message(&format!(
                        "[PERF FIRST] FIRST TASK: queue_wait_ms={}, total_queue_wait_ms={} (should equal queue_wait_ms!), max_queue_wait_ms={}",
                        result.queue_wait_ms, total_qw_now, max_qw_now
                    ));
                }
                if self.total_processed <= 50 || self.total_processed % 500 == 0 || result.queue_wait_ms > 50000 {
                    let _ = config::log_message(&format!(
                        "[PERF DEBUG] queue_wait_ms={} for task={}, total_processed={}, total_queue_wait_ms={}, max_queue_wait_ms={}",
                        result.queue_wait_ms, result.label, self.total_processed, total_qw_now, max_qw_now
                    ));
                }
            }

            if result.success {
                completed += 1;
            } else {
                failed += 1;
                self.total_failed += 1;
                if let Some(err) = result.error {
                    if self.recent_errors.len() >= 5 {
                        self.recent_errors.pop_front();
                    }
                    self.recent_errors.push_back(err);
                }
            }

            // Collect spawned follow-up computations
            spawned.extend(result.spawn);
        }

        // Queue spawned follow-up computations (chaining)
        // IMPORTANT: This happens BEFORE we check in_flight for state transitions,
        // ensuring spawned tasks are counted before we decide to transition.
        for comp in spawned {
            self.queue_computation_internal(comp, None);
        }

        // Capture current state for status
        let current_in_flight = self.in_flight;
        let current_total_processed = self.total_processed;
        let current_session_queued = self.session_queued;
        let current_task_counts = self.task_counts.clone();
        let current_errors = self.recent_errors.iter().cloned().collect();
        let elapsed = self.session_start.map(|start| start.elapsed());

        // State machine transitions (uses self.in_flight internally)
        self.update_state();

        DaemonStatus {
            state: self.state.into(),
            pending: current_in_flight,
            completed,
            failed,
            total_processed: current_total_processed,
            session_queued: current_session_queued,
            recent_errors: current_errors,
            task_counts: current_task_counts,
            elapsed,
            completed_session: self.completed_session.clone(),
        }
    }

    /// Update state machine based on in-flight tasks and timing.
    fn update_state(&mut self) {
        match self.state {
            DaemonState::Idle => {
                // Idle → Working: handled in queue methods
            }
            DaemonState::Working => {
                // Working → Completed: when all work finishes (no in-flight tasks AND
                // db_thread queue empty). Uses centralized has_pending() for consistency
                // with exit handlers and UI state display.
                if !self.has_pending() && self.total_processed > 0 {
                    self.transition_to_completed();
                }
            }
            DaemonState::Completed => {
                // Completed → Idle: after linger timeout
                if let Some(completed_at) = self.completed_at {
                    if completed_at.elapsed() >= Self::LINGER_DURATION {
                        self.transition_to_idle();
                    }
                }
                // Completed → Working: handled in queue methods
            }
        }
    }

    fn transition_to_completed(&mut self) {
        let duration = self.session_start.map(|s| s.elapsed()).unwrap_or_default();

        // Flag to queue awakening computations after session reset
        let mut queue_awakening_after_reset = false;

        self.completed_session = Some(CompletedSession {
            completed_at: Instant::now(),
            duration,
            total_processed: self.total_processed,
            failed: self.total_failed,
            task_counts: self.task_counts.clone(),
        });

        self.completed_at = Some(Instant::now());
        self.state = DaemonState::Completed;

        // Handle eyeballing completion (first-level computations)
        if self.is_eyeballing() {
            self.observation_state = CorpusObservationState::Complete;

            match self.eye_state {
                EyeState::Closed => {
                    // Eyeballing complete - enter Awakening for directory derivations
                    let _ = config::log_message(&format!(
                        "[STATE] Eyeballing complete. Transitioning Closed → Awakening. \
                         Processed {} tasks.",
                        self.total_processed
                    ));
                    self.eye_state = EyeState::Awakening;
                    // Flag to queue awakening computations AFTER session reset
                    // (avoids off-by-one: task queued before reset, completed after)
                    queue_awakening_after_reset = true;
                }
                EyeState::Awake => {
                    let _ = config::log_message(
                        "[STATE] Re-eyeballing complete while Awake. NOP."
                    );
                }
                EyeState::Awakening => {
                    // Invalid: can't complete eyeballing while already awakening
                    panic!("Invalid state: eyeballing completed while eye is Awakening");
                }
            }
        }
        // Handle awakening completion - transition directly to Awake
        // ContentAnalysis is triggered separately via queue_content_analysis() after intake
        else if self.eye_state == EyeState::Awakening {
            let _ = config::log_message(&format!(
                "[STATE] Awakening complete. Transitioning Awakening → Awake. \
                 Processed {} tasks.",
                self.total_processed
            ));
            self.eye_state = EyeState::Awake;

            // Enable mutations - ONE-TIME transition from read-only init to read-write operation
            if !self.read_only_mode {
                self.accepting_mutations = true;
                let _ = config::log_message(
                    "[STATE] Mutations now enabled (read-write mode)."
                );
            }
        }

        // Reset session state
        self.session_start = None;
        self.total_processed = 0;
        self.total_failed = 0;
        self.session_queued = 0;
        self.task_counts.clear();
        self.recent_errors.clear();
        self.current_label = None;

        // Queue awakening computations AFTER reset to fix off-by-one counting
        // (if queued before reset, the task's queue count gets wiped but it still completes)
        if queue_awakening_after_reset {
            self.queue_awakening_computations();
        }
    }

    /// Queue second-level signal computations during Awakening.
    ///
    /// This queues `ScheduleSecondLevelDerivations` which will spawn per-directory
    /// computations to derive signals like UnindexedFile, MissingFile, etc.
    fn queue_awakening_computations(&mut self) {
        use crate::corpus::computations::Computation;

        let _ = config::log_message(
            "[STATE] Queueing ScheduleSecondLevelDerivations for Awakening"
        );

        // Queue the orchestrator computation that will spawn per-directory derivations
        self.queue_computation_with_label(
            Computation::ScheduleSecondLevelDerivations,
            Some("Computing directory signals".to_string()),
        );
    }

    /// Queue content analysis computations.
    ///
    /// This queues `ScheduleContentAnalysis` which will spawn bulk detection
    /// computations for FingerprintDuplicate, MissingTag, etc.
    ///
    /// Called from UI after intake flow completes (Eye is already Awake).
    pub fn queue_content_analysis(&mut self) {
        use crate::corpus::computations::Computation;

        let _ = config::log_message(
            "[STATE] Queueing ScheduleContentAnalysis for content analysis"
        );

        // Queue the orchestrator computation that will spawn all detection computations
        self.queue_computation_with_label(
            Computation::ScheduleContentAnalysis,
            Some("Analyzing metadata".to_string()),
        );
    }

    fn transition_to_idle(&mut self) {
        self.state = DaemonState::Idle;
        self.completed_session = None;
        self.completed_at = None;
    }

    fn transition_to_working(&mut self) {
        if self.session_start.is_none() {
            self.session_start = Some(Instant::now());
        }
        self.state = DaemonState::Working;
    }

    // -------------------------------------------------------------------------
    // Task Spawning Helper
    // -------------------------------------------------------------------------

    /// Spawn a task on the rayon thread pool with panic catching.
    ///
    /// If the task panics, we still send a failure result so the daemon's
    /// in_flight counter stays accurate and we don't lose tasks silently.
    fn spawn_task(&self, task: Task, label: String, queue_time: Instant) {
        let tx = self.result_tx.clone();
        let label_for_panic = label.clone();
        rayon::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                execute_task(task, label, queue_time)
            }));
            let result = match result {
                Ok(r) => r,
                Err(e) => {
                    let panic_msg = if let Some(s) = e.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = e.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "Unknown panic".to_string()
                    };
                    TaskResult {
                        success: false,
                        error: Some(format!("Task panicked: {}", panic_msg)),
                        label: label_for_panic,
                        spawn: Vec::new(),
                        duration_ms: queue_time.elapsed().as_millis() as u64,
                        queue_wait_ms: 0,
                        thread_stats: None,
                    }
                }
            };
            let _ = tx.send(result);
        });
    }

    // -------------------------------------------------------------------------
    // Label Resolution Helper
    // -------------------------------------------------------------------------

    /// Resolve task label from explicit label, current_label, or task fallback.
    fn resolve_label(&self, explicit_label: Option<String>, task: &Task) -> String {
        explicit_label
            .or_else(|| self.current_label.clone())
            .unwrap_or_else(|| TaskLabel::from_task(task).0)
    }

    // -------------------------------------------------------------------------
    // Internal Mutation Queueing (used by transaction API)
    // -------------------------------------------------------------------------

    // NOTE: Direct mutation queueing methods (queue, queue_all, etc.) were removed.
    // All mutations must go through the transaction API:
    //   1. start_transaction(label)
    //   2. add_decision(idx, witness, label, mutations) for each decision
    //   3. confirm_transaction(witness) to commit, or discard_transaction(witness) to abort
    //
    // This ensures proper decision witness semantics where each user action is
    // explicitly witnessed, and batch review/commit is possible.

    fn queue_mutation_internal(&mut self, mutation: Mutation, label: Option<String>) {
        self.transition_to_working();

        let task = Task::Mutation(mutation);
        let task_label = self.resolve_label(label, &task);

        self.session_queued += 1;
        self.in_flight += 1;
        self.spawn_task(task, task_label, Instant::now());
    }

    fn queue_mutations_internal(&mut self, mutations: impl IntoIterator<Item = Mutation>, label: Option<String>) {
        self.transition_to_working();

        let queue_time = Instant::now();
        let mutations: Vec<_> = mutations.into_iter().collect();

        let _ = config::log_message(&format!(
            "[WORKER] queue_mutations_internal: queueing {} mutations (label={:?})",
            mutations.len(), label
        ));

        self.session_queued += mutations.len();
        self.in_flight += mutations.len();

        for mutation in mutations {
            let task = Task::Mutation(mutation);
            let task_label = self.resolve_label(label.clone(), &task);
            self.spawn_task(task, task_label, queue_time);
        }
    }

    // -------------------------------------------------------------------------
    // Computation Queueing (no witness required)
    // -------------------------------------------------------------------------

    /// Queue a single computation (no witness required).
    ///
    /// Computations are derived facts that don't alter state - they only emit
    /// signals. They can execute without user decisions.
    pub fn queue_computation(&mut self, computation: Computation) {
        self.queue_computation_with_label(computation, None);
    }

    /// Queue a single computation with an explicit label (no witness required).
    pub fn queue_computation_with_label(&mut self, computation: Computation, label: Option<String>) {
        self.queue_computation_internal(computation, label);
    }

    fn queue_computation_internal(&mut self, computation: Computation, label: Option<String>) {
        self.transition_to_working();

        let task = Task::Computation(computation);
        let task_label = self.resolve_label(label, &task);

        self.session_queued += 1;
        self.in_flight += 1;
        self.spawn_task(task, task_label, Instant::now());
    }

    /// Queue multiple computations (no witness required).
    ///
    /// Computations are derived facts that don't alter state - they only emit
    /// signals. They can execute without user decisions.
    pub fn queue_computations(&mut self, computations: impl IntoIterator<Item = Computation>) {
        self.queue_computations_with_label(computations, None);
    }

    /// Queue multiple computations with an explicit label (no witness required).
    pub fn queue_computations_with_label(&mut self, computations: impl IntoIterator<Item = Computation>, label: Option<String>) {
        self.transition_to_working();

        let queue_time = Instant::now();
        let computations: Vec<_> = computations.into_iter().collect();

        self.session_queued += computations.len();
        self.in_flight += computations.len();

        for computation in computations {
            let task = Task::Computation(computation);
            let task_label = self.resolve_label(label.clone(), &task);
            self.spawn_task(task, task_label, queue_time);
        }
    }

    // -------------------------------------------------------------------------
    // Migration Queueing (requires witness, bypasses accepting_mutations)
    // -------------------------------------------------------------------------

    /// Queue a single migration for execution.
    ///
    /// Migrations require a [`DecisionWitness`] (user approval) but bypass the
    /// `accepting_mutations` gate. They can run before eyeballing completes.
    pub fn queue_migration(&mut self, migration: Migration, _witness: &DecisionWitness) {
        self.queue_migration_internal(migration, None);
    }

    /// Queue a single migration with an explicit label.
    pub fn queue_migration_with_label(
        &mut self,
        migration: Migration,
        label: Option<String>,
        _witness: &DecisionWitness,
    ) {
        self.queue_migration_internal(migration, label);
    }

    /// Queue multiple migrations for execution.
    pub fn queue_migrations(
        &mut self,
        migrations: impl IntoIterator<Item = Migration>,
        _witness: &DecisionWitness,
    ) {
        self.queue_migrations_internal(migrations, None);
    }

    fn queue_migration_internal(&mut self, migration: Migration, label: Option<String>) {
        self.transition_to_working();

        let task = Task::Migration(migration);
        let task_label = self.resolve_label(label, &task);

        self.session_queued += 1;
        self.in_flight += 1;
        self.spawn_task(task, task_label, Instant::now());
    }

    fn queue_migrations_internal(
        &mut self,
        migrations: impl IntoIterator<Item = Migration>,
        label: Option<String>,
    ) {
        self.transition_to_working();

        let queue_time = Instant::now();
        let migrations: Vec<_> = migrations.into_iter().collect();

        self.session_queued += migrations.len();
        self.in_flight += migrations.len();

        for migration in migrations {
            let task = Task::Migration(migration);
            let task_label = self.resolve_label(label.clone(), &task);
            self.spawn_task(task, task_label, queue_time);
        }
    }

    // -------------------------------------------------------------------------
    // Transaction Helpers
    // -------------------------------------------------------------------------

    /// Require an active transaction, returning error if none exists.
    fn require_active_transaction(&self, operation: &str) -> Result<(), TransactionError> {
        if self.pending_transaction.is_none() {
            let _ = config::log_message(&format!(
                "[TRANSACTION] {} REJECTED: no active transaction",
                operation
            ));
            Err(TransactionError::NoActiveTransaction)
        } else {
            Ok(())
        }
    }

    // -------------------------------------------------------------------------
    // Transaction API
    // -------------------------------------------------------------------------

    /// Start a new transaction.
    ///
    /// Called by modals when the user is about to be presented with Decisions.
    /// Only one transaction may be active at a time.
    ///
    /// Returns Err if a transaction is already active.
    pub fn start_transaction(&mut self, label: &str) -> Result<(), TransactionError> {
        if self.pending_transaction.is_some() {
            let _ = config::log_message(&format!(
                "[TRANSACTION] start_transaction({:?}) REJECTED: already active",
                label
            ));
            return Err(TransactionError::AlreadyActive);
        }

        let _ = config::log_message(&format!(
            "[TRANSACTION] start_transaction({:?}) OK",
            label
        ));
        self.pending_transaction = Some(PendingTransaction::new(label));
        Ok(())
    }

    /// Check if a transaction is currently active.
    pub fn has_transaction(&self) -> bool {
        self.pending_transaction.is_some()
    }

    /// Get transaction info for UI display.
    pub fn transaction_info(&self) -> Option<TransactionInfo> {
        self.pending_transaction.as_ref().map(|txn| TransactionInfo {
            label: txn.label.clone(),
            decision_count: txn.decision_count(),
            mutation_count: txn.mutation_count(),
            started_at: txn.started_at,
        })
    }

    /// Add a witnessed decision to the transaction.
    ///
    /// - `idx`: UI-provided index (may have gaps, largely sequential)
    /// - `witness`: Proof of operator confirmation
    /// - `label`: Human-readable description
    /// - `mutations`: The mutations this decision represents
    ///
    /// Overwrites any existing decision at the same index.
    /// Returns Err if no transaction is active.
    pub fn add_decision(
        &mut self,
        idx: usize,
        _witness: &DecisionWitness,
        label: impl Into<String>,
        mutations: Vec<Mutation>,
    ) -> Result<(), TransactionError> {
        let label_str: String = label.into();
        let mutation_count = mutations.len();

        self.require_active_transaction(&format!(
            "add_decision(idx={}, label={:?}, mutations={})",
            idx, label_str, mutation_count
        ))?;

        let txn = self.pending_transaction.as_mut().unwrap();

        let _ = config::log_message(&format!(
            "[TRANSACTION] add_decision(idx={}, label={:?}, mutations={}) OK - txn now has {} decisions",
            idx, label_str, mutation_count, txn.decision_count() + 1
        ));

        // Log each mutation for debugging
        for (i, m) in mutations.iter().enumerate() {
            let _ = config::log_message(&format!(
                "[TRANSACTION]   mutation[{}]: {:?}",
                i, m
            ));
        }

        txn.decisions.insert(
            idx,
            WitnessedDecision {
                label: label_str,
                mutations,
            },
        );

        Ok(())
    }

    /// Discard a single decision by index.
    ///
    /// Requires a witness - discarding is also a decision.
    /// Used primarily for review screen before confirm/discard.
    ///
    /// Returns the discarded decision, or None if no decision at that index.
    pub fn discard_decision(
        &mut self,
        idx: usize,
        _witness: &DecisionWitness,
    ) -> Result<Option<WitnessedDecision>, TransactionError> {
        self.require_active_transaction(&format!("discard_decision(idx={})", idx))?;

        let txn = self.pending_transaction.as_mut().unwrap();
        Ok(txn.decisions.remove(&idx))
    }

    /// Fetch a decision by index.
    ///
    /// Returns None if no decision stored at that index.
    pub fn get_decision(&self, idx: usize) -> Option<&WitnessedDecision> {
        self.pending_transaction
            .as_ref()
            .and_then(|txn| txn.decisions.get(&idx))
    }

    /// List all decision indices in the current transaction.
    pub fn decision_indices(&self) -> Vec<usize> {
        self.pending_transaction
            .as_ref()
            .map(|txn| txn.indices())
            .unwrap_or_default()
    }

    /// Confirm the transaction - queue all mutations for execution.
    ///
    /// This is the primary way to add mutations to the execution queue.
    /// Requires a witness for the commit decision itself.
    ///
    /// Returns summary of what was committed, or error if mutations not accepted.
    pub fn confirm_transaction(
        &mut self,
        _witness: &DecisionWitness,
    ) -> Result<CommitSummary, TransactionError> {
        // Gate: mutations must be accepted (eyeballing complete, not read-only)
        if !self.accepting_mutations {
            let _ = config::log_message(
                "[TRANSACTION] confirm_transaction REJECTED: not accepting mutations (eyeballing incomplete or read-only mode)"
            );
            return Err(TransactionError::NotAcceptingMutations);
        }

        self.require_active_transaction("confirm_transaction")?;

        let txn = self.pending_transaction.take().unwrap();

        let decision_count = txn.decision_count();
        let mut mutation_count = 0;

        // Collect all mutations from all decisions
        let all_mutations: Vec<Mutation> = txn
            .decisions
            .into_values()
            .flat_map(|d| {
                mutation_count += d.mutations.len();
                d.mutations
            })
            .collect();

        let _ = config::log_message(&format!(
            "[TRANSACTION] confirm_transaction OK - {} decisions, {} mutations queued",
            decision_count, mutation_count
        ));

        // Queue mutations for execution
        if !all_mutations.is_empty() {
            self.queue_mutations_internal(all_mutations, Some(txn.label));
        } else {
            let _ = config::log_message(
                "[TRANSACTION] confirm_transaction: no mutations to queue (empty transaction)"
            );
        }

        Ok(CommitSummary {
            decision_count,
            mutation_count,
        })
    }

    /// Discard the transaction - drop all accumulated decisions.
    ///
    /// Requires a witness - discarding is also a decision.
    ///
    /// Returns summary of what was discarded.
    pub fn discard_transaction(
        &mut self,
        _witness: &DecisionWitness,
    ) -> Result<DiscardSummary, TransactionError> {
        self.require_active_transaction("discard_transaction")?;

        let txn = self.pending_transaction.take().unwrap();

        let _ = config::log_message(&format!(
            "[TRANSACTION] discard_transaction OK - discarded {} decisions, {} mutations",
            txn.decision_count(), txn.mutation_count()
        ));

        Ok(DiscardSummary {
            decision_count: txn.decision_count(),
            mutation_count: txn.mutation_count(),
        })
    }

    // -------------------------------------------------------------------------
    // Read-Only Database Access (UI Queries)
    // -------------------------------------------------------------------------

    /// Get a read-only database connection for UI queries.
    ///
    /// This connection is cached for the daemon's lifetime. All UI code
    /// should use this instead of creating direct `Database::open()` connections.
    ///
    /// The connection uses `PRAGMA query_only = ON` to prevent any writes,
    /// ensuring UI code cannot accidentally mutate the database.
    ///
    /// # Panics
    ///
    /// Panics if database path is not configured or database cannot be opened.
    pub fn read_only_db(&mut self) -> &Database {
        if self.read_only_conn.is_none() {
            let db_path = config::get_db_path().expect("Database path not configured");
            let db = Database::open_read_only(&db_path)
                .expect("Failed to open read-only database connection");
            self.read_only_conn = Some(db);
        }
        self.read_only_conn.as_ref().unwrap()
    }

    /// Invalidate the cached read-only connection.
    ///
    /// Call this after schema migrations to ensure the UI sees the updated schema.
    /// The next call to `read_only_db()` will open a fresh connection.
    pub fn invalidate_read_only_conn(&mut self) {
        self.read_only_conn = None;
    }

    // -------------------------------------------------------------------------
    // Utility Methods
    // -------------------------------------------------------------------------

    /// Get current daemon status snapshot (readonly, does not advance state).
    ///
    /// Safe to call from render code, utility functions, etc.
    /// Use `tick()` only from the main event loop to advance state.
    pub fn status(&self) -> DaemonStatus {
        DaemonStatus {
            state: self.state.into(),
            pending: self.in_flight,
            completed: 0, // Only meaningful from tick() result
            failed: 0,    // Only meaningful from tick() result
            total_processed: self.total_processed,
            session_queued: self.session_queued,
            recent_errors: self.recent_errors.iter().cloned().collect(),
            task_counts: self.task_counts.clone(),
            elapsed: self.session_start.map(|start| start.elapsed()),
            completed_session: self.completed_session.clone(),
        }
    }

    /// Check if there's pending work (tasks queued or in-flight).
    pub fn has_pending(&self) -> bool {
        self.in_flight > 0 || !self.db_thread_handle.queue_empty()
    }

    /// Get current DB thread stats for UI display.
    /// Returns None if timing instrumentation is disabled.
    pub fn db_stats(&self) -> Option<DbThreadStats> {
        self.db_thread_handle.stats()
    }

    /// Get pending DB write queue depth (always available, no timing guard).
    pub fn db_queue_depth(&self) -> u64 {
        self.db_thread_handle.queue_depth()
    }

    /// Get current worker performance stats for UI display.
    /// Returns None if timing instrumentation is disabled.
    pub fn worker_stats(&self) -> Option<WorkerStats> {
        let stats = self.worker_stats_shared.as_ref()?.snapshot();

        // DEBUG: Log if avg > max (should never happen now with isolated stats)
        if stats.tasks_completed > 0 && stats.queue_wait_avg_ms > stats.queue_wait_max_ms {
            let _ = config::log_message(&format!(
                "[PERF BUG] avg > max! tasks={}, avg={}, max={}",
                stats.tasks_completed, stats.queue_wait_avg_ms, stats.queue_wait_max_ms
            ));
        }

        Some(stats)
    }

    /// Check if there's a lingering completed session to display.
    pub fn has_completed_session(&self) -> bool {
        self.state == DaemonState::Completed
    }

    /// Cancel all pending tasks.
    ///
    /// Note: With rayon, spawned tasks will still complete but their results
    /// are drained and discarded. The in_flight counter is reset to 0.
    pub fn cancel(&mut self) {
        // Drain any pending results (discard them)
        while self.result_rx.try_recv().is_ok() {}
        self.in_flight = 0;
    }
}

impl Default for TaskDaemon {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Task Execution Helpers
// ============================================================================

/// Open database for task execution, returning error TaskResult if it fails.
fn open_db_for_task(label: String, start: Instant, queue_wait_ms: u64) -> Result<Database, TaskResult> {
    match config::get_db_path().and_then(|p| Database::open(&p).map_err(|e| e.into())) {
        Ok(db) => Ok(db),
        Err(e) => {
            let _ = config::log_message(&format!(
                "[EXECUTION] DB open FAILED: {}",
                e
            ));
            Err(TaskResult {
                success: false,
                error: Some(format!("DB error: {}", e)),
                label,
                spawn: Vec::new(),
                duration_ms: start.elapsed().as_millis() as u64,
                queue_wait_ms,
                thread_stats: None,
            })
        }
    }
}

// ============================================================================
// Task Execution
// ============================================================================

/// Execute a single task (mutation, computation, or migration). Opens DB connection as needed.
fn execute_task(task: Task, label: String, queue_time: Instant) -> TaskResult {
    let queue_wait_ms = queue_time.elapsed().as_millis() as u64;

    match task {
        Task::Mutation(mutation) => execute_mutation(mutation, label, queue_wait_ms),
        Task::Computation(computation) => execute_computation(computation, label, queue_wait_ms),
        Task::Migration(migration) => execute_migration(migration, label, queue_wait_ms),
    }
}

/// Execute a single mutation. Opens DB connection as needed.
fn execute_mutation(mutation: Mutation, label: String, queue_wait_ms: u64) -> TaskResult {
    use crate::corpus::mutations::{file_ops, tag_edit, indexing, MutationCategory};

    let start = Instant::now();

    let _ = config::log_message(&format!(
        "[EXECUTION] execute_mutation START: {:?} (label={:?})",
        mutation.category(), label
    ));

    // Create execution witness - proves we're inside daemon execution context
    let witness = MutationExecutionWitness::new();

    // Open database
    let db = match open_db_for_task(label.clone(), start, queue_wait_ms) {
        Ok(db) => db,
        Err(result) => return result,
    };

    let session_id = "daemon";

    let (success, error) = match mutation.category() {
        MutationCategory::TagEdit => {
            let _ = config::log_message(&format!(
                "[EXECUTION] TagEdit mutation: {:?}",
                mutation
            ));
            let r = tag_edit::execute_single(&db, &mutation, session_id, &witness);
            (r.success, r.error)
        }
        MutationCategory::FileMove | MutationCategory::FileCopy |
        MutationCategory::FileDelete | MutationCategory::Deployment => {
            let r = file_ops::execute_single(Some(&db), &mutation, &witness);
            (r.success, r.error)
        }
        MutationCategory::Indexing => {
            let r = indexing::execute_single(&db, &mutation, &witness);
            (r.success, r.error)
        }
        MutationCategory::Migration => {
            (false, Some("Migrations not supported in daemon".to_string()))
        }
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    let _ = config::log_message(&format!(
        "[EXECUTION] execute_mutation END: success={}, error={:?}, duration={}ms",
        success, error, duration_ms
    ));

    // Queue per-file signal updates for affected paths
    // This ensures signals like UnindexedFile → HealthyFile are updated
    let spawn = if success {
        mutation
            .affected_paths()
            .into_iter()
            .map(|path| Computation::UpdateFileSignals { path })
            .collect()
    } else {
        Vec::new()
    };

    TaskResult {
        success,
        error,
        label,
        spawn,
        duration_ms,
        queue_wait_ms,
        thread_stats: None, // Mutations don't use thread-local stats
    }
}

/// Execute a single computation. Uses thread-local DB connection.
fn execute_computation(computation: Computation, label: String, queue_wait_ms: u64) -> TaskResult {
    use crate::corpus::computations;

    let result = computations::execute_single(&computation);

    // Capture thread stats after execution
    let thread_stats = Some(computations::get_thread_stats());

    TaskResult {
        success: result.success,
        error: result.error,
        label,
        spawn: result.spawn,
        duration_ms: result.duration_ms,
        queue_wait_ms,
        thread_stats,
    }
}

/// Execute a single migration. Opens DB connection and runs the migration.
fn execute_migration(migration: Migration, label: String, queue_wait_ms: u64) -> TaskResult {
    use crate::corpus::mutations::MigrationRegistry;

    let start = Instant::now();

    // Create migration witness - proves we're inside daemon execution context
    let witness = MigrationWitness::new();

    // Open database
    let db = match open_db_for_task(label.clone(), start, queue_wait_ms) {
        Ok(db) => db,
        Err(result) => return result,
    };

    // Apply the specific migration
    let registry = MigrationRegistry::new();
    let result = registry.apply_migration(&db, migration.to_version, &witness);

    let (success, error) = match result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(e.to_string())),
    };

    TaskResult {
        success,
        error,
        label,
        spawn: Vec::new(),
        duration_ms: start.elapsed().as_millis() as u64,
        queue_wait_ms,
        thread_stats: None, // Migrations don't use thread-local stats
    }
}
