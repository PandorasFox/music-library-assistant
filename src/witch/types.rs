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

use crate::config::Config;
use crate::meta::computations::Computation;
use crate::meta::maintenance::DbMaintenanceTask;
use crate::meta::mutations::Mutation;
use crate::meta::recomputation::RecomputationScope;

// Re-export protocol-visible types from mm-meta.
pub use mm_meta::witch_types::*;

// Re-export from mm-meta
pub use mm_meta::computations::types::ObservedInodeMeta;

// ============================================================================
// Zone-Keyed Observation State
// ============================================================================

/// Authoritative inode→metadata maps for all watched zones.
///
/// The watcher thread populates these maps during initial scan and updates
/// them incrementally via steady-state events. Derivation computations
/// receive cloned snapshots.
pub(super) struct ObservedInodes {
    pub(super) corpus: HashMap<i64, ObservedInodeMeta>,
    pub(super) library: HashMap<i64, ObservedInodeMeta>,
}

impl ObservedInodes {
    pub(super) fn new() -> Self {
        Self {
            corpus: HashMap::new(),
            library: HashMap::new(),
        }
    }

    /// Runtime zone dispatch — returns the inode map for the given zone.
    pub(super) fn for_zone_mut(&mut self, zone: crate::db::types::Zone) -> Option<&mut HashMap<i64, ObservedInodeMeta>> {
        match zone {
            crate::db::types::Zone::Corpus => Some(&mut self.corpus),
            crate::db::types::Zone::Library => Some(&mut self.library),
        }
    }

    pub(super) fn clear(&mut self) {
        self.corpus.clear();
        self.library.clear();
    }
}

/// Build the zone→root list for watcher start commands.
///
/// Always includes corpus. Includes library if its directory exists on disk,
/// filtered to only configured deployment directories.
pub(super) fn watched_zones(config: Option<&crate::config::Config>) -> Vec<super::fs_thread::WatchedZone> {
    let resolver = crate::corpus::paths::get_resolver();
    let mut zones = vec![super::fs_thread::WatchedZone {
        zone: crate::db::types::Zone::Corpus,
        root: resolver.corpus_dir(),
        allowed_subdirs: None,
    }];

    let libraries_dir = resolver.libraries_dir();
    if libraries_dir.is_dir() {
        let allowed_subdirs = config
            .map(crate::meta::computations::helpers::get_configured_library_names);
        zones.push(super::fs_thread::WatchedZone {
            zone: crate::db::types::Zone::Library,
            root: libraries_dir,
            allowed_subdirs,
        });
    }

    zones
}

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
/// Four kinds: Mutations (operator-confirmed corpus changes), SoftMutations
/// (automated library-zone operations), Computations (read-only signal
/// derivation), and Maintenance (operator-approved DB infrastructure tasks).
///
/// External metadata fetching (AcoustID/MusicBrainz) is handled by the
/// scheduler thread directly — it does not flow through the rayon pool.
#[derive(Debug, Clone)]
pub enum Task {
    /// A state-altering mutation (requires ConfirmationGesture to stage).
    Mutation(Box<Mutation>),
    /// A library-zone filesystem operation (no gesture required, auto-deploy).
    SoftMutation(mm_meta::soft_mutations::SoftMutation),
    /// A read-only computation that emits signals (no gesture required).
    Computation(Computation),
    /// A database maintenance task (requires operator approval, bypasses accepting_mutations).
    Maintenance(DbMaintenanceTask),
}

/// Fieldless discriminant for completed task type.
/// Mirror of `Task` — exhaustive match on `Task` enforces sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    Mutation,
    SoftMutation,
    Computation,
    Maintenance,
}

impl TaskKind {
    pub fn from_task(task: &Task) -> Self {
        match task {
            Task::Mutation(_) => TaskKind::Mutation,
            Task::SoftMutation(_) => TaskKind::SoftMutation,
            Task::Computation(_) => TaskKind::Computation,
            Task::Maintenance(_) => TaskKind::Maintenance,
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

    /// Create label from a soft mutation.
    pub fn from_soft_mutation(sm: &mm_meta::soft_mutations::SoftMutation) -> Self {
        Self(sm.label().to_string())
    }

    /// Create label from a task (mutation, computation, or maintenance).
    pub fn from_task(task: &Task) -> Self {
        match task {
            Task::Mutation(m) => Self::from_mutation(m),
            Task::SoftMutation(sm) => Self::from_soft_mutation(sm),
            Task::Computation(c) => Self::from_computation(c),
            Task::Maintenance(t) => Self::from_maintenance(t),
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

// ============================================================================
// Offload Types (spawn_blocking results back to main loop)
// ============================================================================

/// Results from `spawn_blocking` tasks offloaded from the main loop.
///
/// Every blocking operation in the Witch's `select!` loop is replaced by a
/// `spawn_blocking` call that sends its result back through this enum via
/// the unified offload channel.
pub(super) enum OffloadResult {
    /// Watcher DB cache built (for start_watching / poll re-walk).
    WatcherDbCache {
        cache: HashMap<i64, super::fs_thread::CachedInodeState>,
    },
    /// Auto-index query result: mutations to queue (empty = no unindexed files).
    AutoIndexResult {
        mutations: Vec<Mutation>,
        /// Inodes whose signal_unindexed_file entries reference paths that no
        /// longer exist on disk.  The offload handler clears these so the
        /// auto-indexer doesn't spin on them forever.
        stale_inodes: Vec<i64>,
        source: AutoIndexSource,
    },
    /// Auto-deploy query result: soft mutations to queue (empty = nothing to deploy).
    AutoDeployResult {
        soft_mutations: Vec<mm_meta::soft_mutations::SoftMutation>,
    },
    /// Sidecar-only deploy result (eager path, doesn't wait for idle).
    SidecarDeployResult {
        soft_mutations: Vec<mm_meta::soft_mutations::SoftMutation>,
    },
    /// Vacuum check result from PRAGMA queries.
    VacuumCheck {
        needed: bool,
    },
    /// First-time setup completed (or failed).
    SetupComplete {
        result: Result<SetupOutput, String>,
        reply: tokio::sync::oneshot::Sender<
            Result<
                crate::meta::protocol::UnauthenticatedResponse,
                crate::meta::protocol::ProtocolError,
            >,
        >,
    },
}

/// Distinguishes auto-index check call sites for correct follow-up routing.
pub(super) enum AutoIndexSource {
    /// Called at Inodes→Full transition. If no mutations, queue content analysis.
    InodesTransition,
    /// Called during steady-state Full. Informational only.
    SteadyState,
}

// ============================================================================
// Pipeline Scheduling
// ============================================================================

/// Bitmask of pipeline actions pending after work completes.
///
/// Replaces scattered boolean flags with a single typed value that makes
/// scheduling policy explicit: some flags are consumed at idle (30s linger +
/// priority chain), while `SIDECAR_DEPLOY` fires eagerly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PendingWork(u8);

impl PendingWork {
    /// No pending work.
    pub const EMPTY: Self = Self(0);
    /// New files indexed → trigger AcoustID fetch at idle.
    pub const FETCH: Self = Self(1 << 0);
    /// External fetch matched → trigger release packing at idle.
    pub const PACKING: Self = Self(1 << 1);
    /// Content analysis ran → trigger audio file deploy at idle.
    pub const AUDIO_DEPLOY: Self = Self(1 << 2);
    /// Content analysis ran → trigger sidecar deploy eagerly.
    pub const SIDECAR_DEPLOY: Self = Self(1 << 3);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

impl std::ops::BitOr for PendingWork {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Output from the blocking first-time setup task.
pub(super) struct SetupOutput {
    pub config: crate::config::Config,
    pub force_check: bool,
    pub vacuum_threshold: f64,
}

/// Deferred watcher command waiting for DB cache to be built.
pub(super) enum PendingWatcherCommand {
    Start { zones: Vec<super::fs_thread::WatchedZone> },
    Poll { interval_secs: u64 },
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
    /// Barrier-separated follow-up computation phases (pipeline orchestrators only).
    pub deferred_phases:
        std::collections::VecDeque<(crate::meta::computations::PipelineStage, Vec<Computation>)>,
    /// Fetch requests — MB entities to fetch, then queue follow-up computations.
    pub fetch_requests: Vec<crate::meta::computations::FetchRequest>,
}
