//! Task Daemon - Core execution subsystem for MLA.
//!
//! This module is the critical junction point for all corpus mutations and
//! computations. It enforces the DecisionWitness pattern for mutations.
//!
//! NOTE: Do not modify this module without explicit authorization.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};

use parallel_worker::{CancelableWorker, WorkerInit, WorkerMethods};

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
}

// ============================================================================
// Task Daemon
// ============================================================================

/// Simple parallel task queue with state machine.
pub struct TaskDaemon {
    worker: CancelableWorker<(Task, String), TaskResult>,

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
}

impl TaskDaemon {
    /// Linger duration for completed session display.
    const LINGER_DURATION: Duration = Duration::from_secs(30);

    pub fn new() -> Self {
        let worker = CancelableWorker::new(|(task, label): (Task, String), _state| {
            Some(execute_task(task, label))
        });

        // Spawn the dedicated DB write thread
        let db_thread_handle = db_thread::spawn();

        Self {
            worker,
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
        }
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

    /// Advance internal processing. Call each frame from UI loop.
    ///
    /// - Drains completed task results from worker
    /// - Auto-queues any spawned follow-up computations
    /// - Updates state machine transitions
    pub fn tick(&mut self) -> DaemonStatus {
        let mut completed = 0;
        let mut failed = 0;

        // Drain completed results and collect spawned computations
        let mut spawned: Vec<Computation> = Vec::new();

        while let Some(result) = self.worker.get() {
            // Task has completed - no longer in flight
            self.in_flight = self.in_flight.saturating_sub(1);
            self.total_processed += 1;

            // Track by task type
            *self.task_counts.entry(result.label.clone()).or_insert(0) += 1;

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
                // Working → Completed: when all tasks finish (none in-flight)
                // We use in_flight which tracks tasks from add_task() to result drain,
                // ensuring we don't transition while tasks are executing on worker threads.
                if self.in_flight == 0 && self.total_processed > 0 {
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
                    let _ = crate::config::log_message(&format!(
                        "[STATE] Eyeballing complete. Transitioning Closed → Awakening. \
                         Processed {} tasks.",
                        self.total_processed
                    ));
                    self.eye_state = EyeState::Awakening;
                    self.queue_awakening_computations();
                    // transition_to_completed will be called again when those complete
                }
                EyeState::Awake => {
                    let _ = crate::config::log_message(
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
            let _ = crate::config::log_message(&format!(
                "[STATE] Awakening complete. Transitioning Awakening → Awake. \
                 Processed {} tasks.",
                self.total_processed
            ));
            self.eye_state = EyeState::Awake;

            // Enable mutations - ONE-TIME transition from read-only init to read-write operation
            if !self.read_only_mode {
                self.accepting_mutations = true;
                let _ = crate::config::log_message(
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
    }

    /// Queue second-level signal computations during Awakening.
    ///
    /// This queues `ScheduleSecondLevelDerivations` which will spawn per-directory
    /// computations to derive signals like UnindexedFile, MissingFile, etc.
    fn queue_awakening_computations(&mut self) {
        use crate::corpus::computations::Computation;

        let _ = crate::config::log_message(
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

        let _ = crate::config::log_message(
            "[STATE] Queueing ScheduleContentAnalysis for content analysis"
        );

        // Queue the orchestrator computation that will spawn all detection computations
        self.queue_computation_with_label(
            Computation::ScheduleContentAnalysis,
            Some("Analyzing content".to_string()),
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

        let task_label = label
            .or_else(|| self.current_label.clone())
            .unwrap_or_else(|| TaskLabel::from_mutation(&mutation).0);

        self.session_queued += 1;
        self.in_flight += 1;
        self.worker.add_task((Task::Mutation(mutation), task_label));
    }

    fn queue_mutations_internal(&mut self, mutations: impl IntoIterator<Item = Mutation>, label: Option<String>) {
        self.transition_to_working();

        let tasks: Vec<(Task, String)> = mutations
            .into_iter()
            .map(|m| {
                let task_label = label.clone()
                    .or_else(|| self.current_label.clone())
                    .unwrap_or_else(|| TaskLabel::from_mutation(&m).0);
                (Task::Mutation(m), task_label)
            })
            .collect();

        let _ = config::log_message(&format!(
            "[WORKER] queue_mutations_internal: queueing {} mutations (label={:?})",
            tasks.len(), label
        ));

        self.session_queued += tasks.len();
        self.in_flight += tasks.len();
        self.worker.add_tasks(tasks);
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

        let task_label = label
            .or_else(|| self.current_label.clone())
            .unwrap_or_else(|| TaskLabel::from_computation(&computation).0);

        self.session_queued += 1;
        self.in_flight += 1;
        self.worker.add_task((Task::Computation(computation), task_label));
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

        let tasks: Vec<(Task, String)> = computations
            .into_iter()
            .map(|c| {
                let task_label = label.clone()
                    .or_else(|| self.current_label.clone())
                    .unwrap_or_else(|| TaskLabel::from_computation(&c).0);
                (Task::Computation(c), task_label)
            })
            .collect();

        self.session_queued += tasks.len();
        self.in_flight += tasks.len();
        self.worker.add_tasks(tasks);
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

        let task_label = label
            .or_else(|| self.current_label.clone())
            .unwrap_or_else(|| TaskLabel::from_migration(&migration).0);

        self.session_queued += 1;
        self.in_flight += 1;
        self.worker.add_task((Task::Migration(migration), task_label));
    }

    fn queue_migrations_internal(
        &mut self,
        migrations: impl IntoIterator<Item = Migration>,
        label: Option<String>,
    ) {
        self.transition_to_working();

        let tasks: Vec<(Task, String)> = migrations
            .into_iter()
            .map(|m| {
                let task_label = label
                    .clone()
                    .or_else(|| self.current_label.clone())
                    .unwrap_or_else(|| TaskLabel::from_migration(&m).0);
                (Task::Migration(m), task_label)
            })
            .collect();

        self.session_queued += tasks.len();
        self.in_flight += tasks.len();
        self.worker.add_tasks(tasks);
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

        let txn = match self.pending_transaction.as_mut() {
            Some(t) => t,
            None => {
                let _ = config::log_message(&format!(
                    "[TRANSACTION] add_decision(idx={}, label={:?}, mutations={}) REJECTED: no active transaction",
                    idx, label_str, mutation_count
                ));
                return Err(TransactionError::NoActiveTransaction);
            }
        };

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
        let txn = self
            .pending_transaction
            .as_mut()
            .ok_or(TransactionError::NoActiveTransaction)?;

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

        let txn = match self.pending_transaction.take() {
            Some(t) => t,
            None => {
                let _ = config::log_message(
                    "[TRANSACTION] confirm_transaction REJECTED: no active transaction"
                );
                return Err(TransactionError::NoActiveTransaction);
            }
        };

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
        let txn = match self.pending_transaction.take() {
            Some(t) => t,
            None => {
                let _ = config::log_message(
                    "[TRANSACTION] discard_transaction REJECTED: no active transaction"
                );
                return Err(TransactionError::NoActiveTransaction);
            }
        };

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

    /// Get current daemon status snapshot (without advancing state).
    ///
    /// Use `tick()` to advance state and get status. Use this for read-only
    /// status checks when you don't want to advance the state machine.
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
    pub fn db_stats(&self) -> DbThreadStats {
        self.db_thread_handle.stats()
    }

    /// Check if there's a lingering completed session to display.
    pub fn has_completed_session(&self) -> bool {
        self.state == DaemonState::Completed
    }

    /// Cancel all pending tasks.
    pub fn cancel(&mut self) {
        self.worker.cancel_tasks();
        // Note: in_flight may not accurately reflect cancelled state since
        // cancelled tasks won't produce results to drain. Reset to 0.
        self.in_flight = 0;
    }
}

impl Default for TaskDaemon {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Task Execution
// ============================================================================

/// Execute a single task (mutation, computation, or migration). Opens DB connection as needed.
fn execute_task(task: Task, label: String) -> TaskResult {
    match task {
        Task::Mutation(mutation) => execute_mutation(mutation, label),
        Task::Computation(computation) => execute_computation(computation, label),
        Task::Migration(migration) => execute_migration(migration, label),
    }
}

/// Execute a single mutation. Opens DB connection as needed.
fn execute_mutation(mutation: Mutation, label: String) -> TaskResult {
    use crate::config;
    use crate::corpus::db::Database;
    use crate::corpus::mutations::{file_ops, tag_edit, indexing, MutationCategory};

    let _ = config::log_message(&format!(
        "[EXECUTION] execute_mutation START: {:?} (label={:?})",
        mutation.category(), label
    ));

    // Create execution witness - proves we're inside daemon execution context
    let witness = MutationExecutionWitness::new();

    // Open database
    let db = match config::get_db_path().and_then(|p| Database::open(&p).map_err(|e| e.into())) {
        Ok(db) => db,
        Err(e) => {
            let _ = config::log_message(&format!(
                "[EXECUTION] execute_mutation FAILED: DB error: {}",
                e
            ));
            return TaskResult {
                success: false,
                error: Some(format!("DB error: {}", e)),
                label,
                spawn: Vec::new(),
            };
        }
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

    let _ = config::log_message(&format!(
        "[EXECUTION] execute_mutation END: success={}, error={:?}",
        success, error
    ));

    // Queue computations to recompute signals for affected directories
    // This ensures signals like UnindexedFile → HealthyFile are updated
    let spawn = if success {
        mutation
            .affected_directories()
            .into_iter()
            .map(|directory| Computation::DeriveDirectorySignals { directory })
            .collect()
    } else {
        Vec::new()
    };

    TaskResult { success, error, label, spawn }
}

/// Execute a single computation. Opens DB connection as needed.
fn execute_computation(computation: Computation, label: String) -> TaskResult {
    use crate::corpus::computations;

    let result = computations::execute_single(&computation);

    TaskResult {
        success: result.success,
        error: result.error,
        label,
        spawn: result.spawn,
    }
}

/// Execute a single migration. Opens DB connection and runs the migration.
fn execute_migration(migration: Migration, label: String) -> TaskResult {
    use crate::config;
    use crate::corpus::db::Database;
    use crate::corpus::mutations::MigrationRegistry;

    // Create migration witness - proves we're inside daemon execution context
    let witness = MigrationWitness::new();

    // Open database
    let db = match config::get_db_path().and_then(|p| Database::open(&p).map_err(|e| e.into())) {
        Ok(db) => db,
        Err(e) => {
            return TaskResult {
                success: false,
                error: Some(format!("DB error: {}", e)),
                label,
                spawn: Vec::new(),
            }
        }
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
    }
}
