//! Core types, enums, and witness system for the Witch.
//!
//! Protocol-visible snapshot types (WitchStatus, WorkStatus, etc.) are defined
//! in mm-meta and re-exported here. Server-only types (WorkState, Task,
//! sealed witnesses, TaskLabel, TaskResult) remain local.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use super::external_fetch::ExternalFetchTask;
use crate::config::Config;
use crate::meta::computations::Computation;
use crate::meta::maintenance::DbMaintenanceTask;
use crate::meta::mutations::Mutation;
use crate::meta::recomputation::RecomputationScope;

// Re-export protocol-visible types from mm-meta.
pub use mm_meta::witch_types::*;

// ============================================================================
// HadesSnapshot — Phase-Level Read-Only Data Envelope
// ============================================================================

/// Point-in-time snapshot of Hades's read-only pipeline data.
///
/// Arc-cloned into each dispatched task. Each field is individually Arc'd
/// so snapshot creation is just atomic refcount bumps, not deep copies.
/// Adding new phase-level data = add a field here.
#[derive(Clone)]
pub struct HadesSnapshot {
    /// Current config at dispatch time. `None` only during AwaitingSetup
    /// (before first-time setup completes — no config exists on disk yet).
    pub config: Option<Arc<Config>>,
    // Future: pub proposals: Option<Arc<ProposalSet>>,
}

// ============================================================================
// ManagedThread Trait
// ============================================================================

/// Unified shutdown idiom for threads managed by the Witch.
///
/// All Witch-managed threads follow the same pattern: send a shutdown
/// sentinel via channel, then join the thread handle. This trait
/// codifies that pattern.
pub trait ManagedThread {
    /// Send the shutdown sentinel to the thread.
    fn send_shutdown(&self);

    /// Take the join handle (None if already taken/joined).
    fn take_handle(&mut self) -> Option<std::thread::JoinHandle<()>>;

    /// Send shutdown and join the thread.
    fn shutdown(&mut self) {
        self.send_shutdown();
        if let Some(h) = self.take_handle() {
            let _ = h.join();
        }
    }
}

// ============================================================================
// State Machine (server-only — contains Instant, not serializable)
// ============================================================================

/// High-level Witch work state with session data carried inline.
///
/// Replaces the old flat `TaskExecutionState` + scattered session fields.
/// Session-scoped counters live inside `Working`/`Done` variants.
#[derive(Debug, Clone)]
pub enum WorkState {
    /// No work in progress, no lingering status.
    Idle,
    /// Tasks are queued/executing.
    Working {
        queued: usize,
        in_flight: usize,
        processed: usize,
        by_label: HashMap<String, usize>,
        label: Option<String>,
    },
    /// All tasks complete, lingering status available (LINGER_DURATION → Idle).
    Done {
        finished_at: Instant,
        total_processed: usize,
    },
}

impl WorkState {
    /// Increment in-flight counter. No-op if not Working.
    pub fn inc_in_flight(&mut self) {
        if let WorkState::Working {
            ref mut in_flight, ..
        } = self
        {
            *in_flight += 1;
        }
    }

    /// Decrement in-flight counter (saturating). No-op if not Working.
    pub fn dec_in_flight(&mut self) {
        if let WorkState::Working {
            ref mut in_flight, ..
        } = self
        {
            *in_flight = in_flight.saturating_sub(1);
        }
    }

    /// Increment queued counter by n. No-op if not Working.
    pub fn inc_queued(&mut self, n: usize) {
        if let WorkState::Working { ref mut queued, .. } = self {
            *queued += n;
        }
    }

    /// Increment processed counter. No-op if not Working.
    pub fn inc_processed(&mut self) {
        if let WorkState::Working {
            ref mut processed, ..
        } = self
        {
            *processed += 1;
        }
    }

    /// Get current in-flight count (0 if not Working).
    pub fn in_flight(&self) -> usize {
        match self {
            WorkState::Working { in_flight, .. } => *in_flight,
            _ => 0,
        }
    }

    /// Get current queued count (0 if not Working).
    pub fn queued(&self) -> usize {
        match self {
            WorkState::Working { queued, .. } => *queued,
            _ => 0,
        }
    }

    /// Get current processed count (0 if not Working, total if Done).
    pub fn processed(&self) -> usize {
        match self {
            WorkState::Working { processed, .. } => *processed,
            WorkState::Done {
                total_processed, ..
            } => *total_processed,
            WorkState::Idle => 0,
        }
    }

    /// Check if in Idle state.
    pub fn is_idle(&self) -> bool {
        matches!(self, WorkState::Idle)
    }

    /// Check if in Working state.
    pub fn is_working(&self) -> bool {
        matches!(self, WorkState::Working { .. })
    }

    /// Get pending-by-label map ref (empty if not Working).
    pub fn by_label(&self) -> Option<&HashMap<String, usize>> {
        match self {
            WorkState::Working { ref by_label, .. } => Some(by_label),
            _ => None,
        }
    }

    /// Increment pending count for a label. No-op if not Working.
    pub fn inc_label(&mut self, label: &str) {
        if let WorkState::Working {
            ref mut by_label, ..
        } = self
        {
            *by_label.entry(label.to_string()).or_insert(0) += 1;
        }
    }

    /// Decrement pending count for a label. No-op if not Working.
    pub fn dec_label(&mut self, label: &str) {
        if let WorkState::Working {
            ref mut by_label, ..
        } = self
        {
            if let Some(count) = by_label.get_mut(label) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    by_label.remove(label);
                }
            }
        }
    }
}

impl From<&WorkState> for WorkStateSnapshot {
    fn from(state: &WorkState) -> Self {
        match state {
            WorkState::Idle => WorkStateSnapshot::Idle,
            WorkState::Working { .. } => WorkStateSnapshot::Working,
            WorkState::Done { .. } => WorkStateSnapshot::Done,
        }
    }
}

// ============================================================================
// Task Types
// ============================================================================

/// A task that can be queued for execution.
///
/// Four kinds: Mutations (operator-confirmed corpus changes), Computations
/// (read-only signal derivation), Maintenance (operator-approved DB
/// infrastructure tasks that run before observing), and ExternalFetch
/// (HTTP API calls for external metadata, dispatched by the scheduler thread).
#[derive(Debug, Clone)]
pub enum Task {
    /// A state-altering mutation (requires ConfirmationGesture to stage).
    Mutation(Box<Mutation>),
    /// A read-only computation that emits signals (no gesture required).
    Computation(Computation),
    /// A database maintenance task (requires operator approval, bypasses accepting_mutations).
    Maintenance(DbMaintenanceTask),
    /// An external API fetch (AcoustID lookup or MusicBrainz entity fetch).
    /// Dispatched by the scheduler thread, executed on rayon, does NOT affect work_state.
    ExternalFetch(ExternalFetchTask),
}

/// Fieldless discriminant for completed task type.
/// Mirror of `Task` — exhaustive match on `Task` enforces sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    Mutation,
    Computation,
    Maintenance,
    ExternalFetch,
}

impl TaskKind {
    pub fn from_task(task: &Task) -> Self {
        match task {
            Task::Mutation(_) => TaskKind::Mutation,
            Task::Computation(_) => TaskKind::Computation,
            Task::Maintenance(_) => TaskKind::Maintenance,
            Task::ExternalFetch(_) => TaskKind::ExternalFetch,
        }
    }
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
        pub fn spawn_mutation(
            &self,
            mutation: crate::meta::mutations::Mutation,
        ) -> SpawnedMutation {
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

    /// A zero-sized token proving code is executing inside the Witch's maintenance worker.
    ///
    /// Maintenance task functions (migration apply, vacuum) require this witness,
    /// ensuring they can only be called from within `execute_maintenance()`.
    ///
    /// Cannot be constructed outside the Witch's `execute_maintenance()` function.
    #[derive(Clone, Copy)]
    pub struct MaintenanceWitness(());

    impl MaintenanceWitness {
        /// Create a witness for the DB write thread.
        ///
        /// The DB thread applies migrations that were enqueued from legitimate
        /// maintenance contexts. This constructor allows the DB thread to obtain
        /// a witness for the actual migration call.
        pub(crate) fn new_for_db_thread() -> Self {
            Self(())
        }
    }

    /// A zero-sized token proving content analysis is being queued from a valid context.
    ///
    /// Content analysis can only be triggered from `transition_to_completed` when
    /// mutations drain while reasoning is Full. This prevents accidental queueing
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
pub use sealed::MaintenanceWitness;
pub use sealed::MutationExecutionWitness;
pub use sealed::SpawnedMutation;

// ============================================================================
// Labels
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

    /// Create label from a maintenance task.
    pub fn from_maintenance(task: &DbMaintenanceTask) -> Self {
        Self(task.label())
    }

    /// Create label from an external fetch task.
    pub fn from_external_fetch(task: &ExternalFetchTask) -> Self {
        Self(task.label().to_string())
    }

    /// Create label from a task (mutation, computation, maintenance, or external fetch).
    pub fn from_task(task: &Task) -> Self {
        match task {
            Task::Mutation(m) => Self::from_mutation(m),
            Task::Computation(c) => Self::from_computation(c),
            Task::Maintenance(t) => Self::from_maintenance(t),
            Task::ExternalFetch(f) => Self::from_external_fetch(f),
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
    pub kind: TaskKind,
    /// Follow-up computations to queue (from computation chaining)
    pub spawn: Vec<Computation>,
    /// Follow-up mutations to queue (from mutation spawn chaining)
    pub spawn_mutations: Vec<SpawnedMutation>,
    /// Updated config from ApplyConfigEdits mutation (applied to SharedConfig in tick()).
    pub config_update: Option<crate::config::Config>,
    /// Recomputation scope from this mutation (which domains it dirtied).
    /// EMPTY for computations, migrations, and failed mutations.
    pub recomputation_scope: RecomputationScope,
    /// External fetch result data (only populated for ExternalFetch tasks).
    pub fetch_result: Option<super::external_fetch::FetchOutcome>,
    /// Barrier-separated follow-up computation phases (pipeline orchestrators only).
    pub deferred_phases:
        std::collections::VecDeque<(crate::meta::computations::PipelineStage, Vec<Computation>)>,
}
