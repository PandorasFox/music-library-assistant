//! Core types, enums, and witness system for the Witch.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::corpus::computations::Computation;
use crate::corpus::mutations::Mutation;

// ============================================================================
// State Machine
// ============================================================================

/// High-level Witch state for simple O(1) checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskExecutionState {
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

/// Eye lifecycle state - controlled by the Witch.
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
/// - Unseen/Observing → Mutations rejected
/// - Complete → Mutations accepted
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CorpusObservationState {
    /// Initial unknown state (launch only). Corpus has never been eyeballed.
    #[default]
    Unseen,
    /// Eyeballing in progress.
    Observing,
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
/// a user-led Decision context. The Witch's mutation queueing methods require
/// this token, preventing code from queueing state-altering mutations without
/// explicit operator decisions.
///
/// The token can ONLY be obtained via [`DecisionScope`], which is created by
/// [`Witch::with_operator_decision()`]. That method should ONLY be called from
/// `ui/operator_decisions.rs`, which provides sealed handler functions for
/// Enter keypress handlers in confirmation modals.
///
/// Computations (health signals, verification) use separate queue methods
/// that do NOT require a witness, as they are decisionless by design.
pub mod sealed {
    /// A zero-sized token proving mutations come from a user-led Decision context.
    ///
    /// Cannot be constructed outside [`DecisionScope`].
    /// See `ui/operator_decisions.rs` for the sanctioned access pattern.
    #[derive(Clone, Copy)]
    pub struct DecisionWitness(());

    impl DecisionWitness {
        /// Internal constructor - only callable from DecisionScope::new()
        pub(super) fn new() -> Self {
            Self(())
        }
    }

    /// A zero-sized token proving code is executing inside the Witch's mutation worker.
    ///
    /// All index-mutating database functions require this witness, ensuring they
    /// can only be called from within the Witch's execution context.
    ///
    /// Cannot be constructed outside the Witch's `execute_mutation()` function.
    #[derive(Clone, Copy)]
    pub struct MutationExecutionWitness(());

    impl MutationExecutionWitness {
        /// Internal constructor - only callable from execute_mutation()
        pub(in crate::witch) fn new() -> Self {
            Self(())
        }
    }

    /// A zero-sized token proving code is executing inside the Witch's migration worker.
    ///
    /// Migration apply functions require this witness, ensuring they can only be
    /// called from within the Witch's `execute_migration()` function.
    ///
    /// Cannot be constructed outside the Witch's `execute_migration()` function.
    #[derive(Clone, Copy)]
    pub struct MigrationWitness(());

    impl MigrationWitness {
        /// Internal constructor - only callable from execute_migration()
        pub(in crate::witch) fn new() -> Self {
            Self(())
        }
    }

    /// A zero-sized token proving content analysis is being queued from a valid context.
    ///
    /// Content analysis can only be triggered from `transition_to_completed` when
    /// mutations drain while the Eye is Awake. This prevents accidental queueing
    /// from UI code or other invalid contexts.
    ///
    /// Cannot be constructed outside the Witch's `transition_to_completed()` function.
    #[derive(Clone, Copy)]
    pub struct ContentAnalysisWitness(());

    impl ContentAnalysisWitness {
        /// Internal constructor - only callable from transition_to_completed()
        pub(in crate::witch) fn new() -> Self {
            Self(())
        }
    }
}

pub use sealed::ContentAnalysisWitness;
pub use sealed::DecisionWitness;
pub use sealed::MigrationWitness;
pub use sealed::MutationExecutionWitness;

// ============================================================================
// Operator Decision Scope (Sealed Access Pattern)
// ============================================================================

/// A scoped decision context where operator decisions can be made.
///
/// `DecisionWitness` exists only within this scope and cannot escape.
/// All decision operations (add_decision, confirm_transaction, etc.) are
/// performed through this scope's methods.
///
/// # Sealed Access Pattern
///
/// This type is created ONLY via [`Witch::with_operator_decision()`], which
/// should ONLY be called from `ui/operator_decisions.rs`. The callback pattern
/// ensures the witness cannot be stored, returned, or passed elsewhere.
///
/// See `ui/operator_decisions.rs` for the sanctioned call sites.
pub struct DecisionScope<'a> {
    witch: &'a mut super::Witch,
    witness: DecisionWitness,
}

impl<'a> DecisionScope<'a> {
    /// Create a new DecisionScope. Only callable from within the witch module.
    pub(super) fn new(witch: &'a mut super::Witch) -> Self {
        Self {
            witch,
            witness: DecisionWitness::new(),
        }
    }

    /// Start a new transaction.
    ///
    /// Called when the user is about to be presented with Decisions.
    /// Only one transaction may be active at a time.
    pub fn start_transaction(&mut self, label: &str) -> Result<(), TransactionError> {
        self.witch.start_transaction(label)
    }

    /// Add a witnessed decision to the active transaction.
    ///
    /// - `idx`: UI-provided index (may have gaps, largely sequential)
    /// - `label`: Human-readable description
    /// - `mutations`: The mutations this decision represents
    pub fn add_decision(
        &mut self,
        idx: usize,
        label: impl Into<String>,
        mutations: Vec<Mutation>,
    ) -> Result<(), TransactionError> {
        self.witch.add_decision(idx, &self.witness, label, mutations)
    }

    /// Confirm the transaction - queue all mutations for execution.
    ///
    /// This commits all accumulated decisions and queues their mutations.
    pub fn confirm_transaction(&mut self) -> Result<CommitSummary, TransactionError> {
        self.witch.confirm_transaction(&self.witness)
    }

    /// Discard the transaction - drop all accumulated decisions.
    pub fn discard_transaction(&mut self) -> Result<DiscardSummary, TransactionError> {
        self.witch.discard_transaction(&self.witness)
    }

    /// Discard a single decision by index.
    pub fn discard_decision(&mut self, idx: usize) -> Result<Option<WitnessedDecision>, TransactionError> {
        self.witch.discard_decision(idx, &self.witness)
    }
}

// NOTE: confirm_decision() has been removed from the public API.
// All decision authority now flows through Witch::with_operator_decision()
// which should ONLY be called from ui/operator_decisions.rs.
//
// Transaction flow for mutations:
//   1. operator_decisions::start_transaction()
//   2. operator_decisions::stage_decision() - repeat for each decision
//   3. operator_decisions::commit_transaction() or discard_transaction()

/// Create a MigrationWitness for startup migrations (pre-Witch context).
///
/// Call this when the user approves database migrations at startup.
/// The returned witness can then be passed to [`MigrationRegistry::apply_all_pending`].
///
/// This is separate from the Witch's `execute_migration()` context witness -
/// it's for migrations that run before the Witch exists.
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
            MutationCategory::Deployment => "Deployment",
            MutationCategory::Migration => "Migrations",
            MutationCategory::Transcode => "Transcoding",
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

/// Summary of a completed Witch session (for lingering display).
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
    pub state: TaskExecutionStateSnapshot,
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

/// Snapshot of Witch state for status reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskExecutionStateSnapshot {
    #[default]
    Idle,
    Working,
    Completed,
}

impl From<TaskExecutionState> for TaskExecutionStateSnapshot {
    fn from(state: TaskExecutionState) -> Self {
        match state {
            TaskExecutionState::Idle => TaskExecutionStateSnapshot::Idle,
            TaskExecutionState::Working => TaskExecutionStateSnapshot::Working,
            TaskExecutionState::Completed => TaskExecutionStateSnapshot::Completed,
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
    pub(super) decisions: HashMap<usize, WitnessedDecision>,
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
    /// Attempted to confirm transaction but the Witch is not accepting mutations.
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
pub(super) struct TaskResult {
    pub success: bool,
    pub error: Option<String>,
    pub label: String,
    /// Follow-up computations to queue (from computation chaining)
    pub spawn: Vec<Computation>,
    /// Task execution duration in milliseconds
    pub duration_ms: u64,
    /// Time task waited in queue before execution (milliseconds)
    pub queue_wait_ms: u64,
    /// Snapshot of thread stats after execution (for computations)
    pub thread_stats: Option<crate::corpus::computations::ThreadStats>,
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
