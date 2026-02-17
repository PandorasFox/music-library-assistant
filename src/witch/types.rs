//! Core types, enums, and witness system for the Witch.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use std::collections::HashMap;

use crate::meta::computations::Computation;
use crate::meta::mutations::Mutation;

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
/// Tasks are either Mutations (require ConfirmationGesture to stage), Computations
/// (no gesture required), or Migrations (require operator approval but bypass
/// accepting_mutations gate).
#[derive(Debug, Clone)]
pub enum Task {
    /// A state-altering mutation (requires ConfirmationGesture to stage).
    Mutation(Mutation),
    /// A read-only computation that emits signals (no gesture required).
    Computation(Computation),
    /// A schema migration (requires operator approval but bypasses accepting_mutations).
    Migration(Migration),
}

// ============================================================================
// Migration Types
// ============================================================================

/// A database schema migration.
///
/// Migrations require operator approval but bypass the `accepting_mutations`
/// gate. They must run before indexing can happen if schema changes are required.
///
/// Note: Description is not stored here - it's looked up from MigrationRegistry
/// during execution by version number. UI gets descriptions via
/// `Witch::pending_migration_descriptions()`.
#[derive(Debug, Clone)]
pub struct Migration {
    /// Version number this migration starts from.
    pub from_version: u32,
    /// Version number after migration completes.
    pub to_version: u32,
}

// ============================================================================
// Execution Witnesses (Sealed Access Control)
// ============================================================================

/// Sealed witness types for execution context proofs.
///
/// These witnesses ensure certain operations can only be performed from
/// within specific execution contexts (mutation worker, migration worker, etc.).
///
/// Decision authority (ConfirmationGesture) is handled separately in
/// `ui/action_handlers/witness.rs` and flows through `meta/decisions/`.
pub mod sealed {
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

        /// Create an authorized SpawnedMutation from a Mutation.
        ///
        /// Only callable from within mutation execution context. The existence
        /// of SpawnedMutation IS the proof - it can only be created here.
        ///
        /// Use case: `ApplyTagOps` spawns `ApplyDbTagsToDisk` after DB write succeeds.
        pub fn spawn_mutation(&self, mutation: crate::meta::mutations::Mutation) -> SpawnedMutation {
            SpawnedMutation { mutation }
        }
    }

    /// A mutation spawned by another mutation during execution.
    ///
    /// Can ONLY be created inside mutation execution context via
    /// [`MutationExecutionWitness::spawn_mutation()`]. The existence of this
    /// type IS the proof of authorization - no separate witness token needed.
    ///
    /// Use case: `ApplyTagOps` spawns `ApplyDbTagsToDisk` after DB write succeeds.
    #[derive(Debug, Clone)]
    pub struct SpawnedMutation {
        pub(super) mutation: crate::meta::mutations::Mutation,
    }

    impl SpawnedMutation {
        /// Extract the inner mutation, consuming the wrapper.
        pub fn into_inner(self) -> crate::meta::mutations::Mutation {
            self.mutation
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
pub use sealed::MigrationWitness;
pub use sealed::MutationExecutionWitness;
pub use sealed::SpawnedMutation;

// ============================================================================
// Labels and Status Types
// ============================================================================

/// Human-readable task label for status display.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskLabel(pub String);

impl TaskLabel {
    /// Create label from a mutation (fallback if no explicit label provided).
    pub fn from_mutation(mutation: &Mutation) -> Self {
        Self(mutation.label().to_string())
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
    /// Breakdown of pending tasks by type label
    pub pending_by_label: HashMap<String, usize>,
    /// Whether an idle rescan is currently in progress.
    pub idle_rescan_active: bool,
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
// Transaction Types (re-exported from meta::decisions)
// ============================================================================

pub use crate::meta::decisions::PendingTransaction;

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
    /// Follow-up mutations to queue (from mutation spawn chaining)
    pub spawn_mutations: Vec<SpawnedMutation>,
    /// Task execution duration in milliseconds
    pub duration_ms: u64,
    /// Time task waited in queue before execution (milliseconds)
    pub queue_wait_ms: u64,
    /// Snapshot of thread stats after execution (for computations)
    pub thread_stats: Option<crate::meta::computations::ThreadStats>,
    /// Updated config from ApplyConfigEdits mutation (applied to SharedConfig in tick()).
    pub config_update: Option<crate::config::Config>,
    /// Corpus inodes observed on disk during this computation (inode → relative path).
    pub observed_corpus_inodes: HashMap<i64, String>,
    /// Inbox inodes observed on disk during this computation (inode → relative path).
    pub observed_inbox_inodes: HashMap<i64, String>,
    /// Library files observed on disk during ScanLibraryDirectory.
    pub observed_library_files: Vec<crate::meta::computations::awakening::ObservedLibraryFile>,
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
