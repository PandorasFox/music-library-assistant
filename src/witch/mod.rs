//! The Witch - Background task execution and orderliness enforcement
//!
//! The Witch is named such because She enforces orderliness in her domain,
//! and provides all guarantees for data which flows properly through Her.
//!
//! ## Module Organization
//! - `types.rs` - Core types, enums, witness system
//! - `worker_stats.rs` - Thread-safe performance statistics
//! - `transaction.rs` - Transaction lifecycle management
//! - `execution.rs` - Task execution (mutations, computations, maintenance)
//!
//! ## Extending the Witch
//! - New task types: Add variants to `Task` enum in `types.rs`
//! - New execution logic: Add to `execution.rs`
//! - Transaction features: Modify `transaction.rs`
//! - Performance tracking: Modify `worker_stats.rs`

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use std::sync::mpsc::{self, Receiver, Sender};

use crate::config::{self, Config, SharedConfig};
use crate::meta::computations::{Computation, observation, derivation, analysis};
use crate::db::Database;
use crate::meta::mutations::Mutation;
use crate::db::write_thread::{self, DbThreadHandle, DbThreadStats};

// Module declarations
pub(crate) mod cache_thread;
mod execution;
pub(crate) mod external_fetch;
pub mod messages;
mod transaction;
mod types;
mod worker_stats;

// Re-export public types
pub use messages::InitialUiState;
pub use types::{
    WorkStatus, WorkState, WorkStateSnapshot,
    ReasoningLevel, InodeAwarenessLevel,
    MaintenanceWitness, MutationExecutionWitness,
    PendingTransaction, SpawnedMutation, Task,
    TaskLabel, WorkerStats,
};
// Decision authority flows through ConfirmationGesture (ui/action_handlers/witness.rs)
// and WitnessedDecision (meta/decisions/mod.rs). See operator_decisions.rs for call sites.

// Internal imports
use execution::execute_task;
use types::{ContentAnalysisWitness, TaskResult};
use worker_stats::SharedWorkerStats;

// ============================================================================
// WitchNotice — Typed Witch → UI Notifications
// ============================================================================

/// Typed notifications from the Witch to the UI layer.
///
/// Sent via channel each tick. The UI drains these each frame to update
/// local state (status, errors, cache invalidation) without reaching
/// through the Witch's fields.
pub enum WitchNotice {
    /// Witch's work status snapshot (emitted every tick).
    StatusUpdate(WorkStatus),
    /// Mutations have drained — UI should invalidate caches.
    MutationsCompleted,
    /// A task failed with this error message.
    Error(String),
    /// Safety latch triggered — mutations permanently disabled this session.
    SafetyLatch(String),
    /// Config was updated by a mutation (UI should re-read shared config).
    ConfigUpdated,
}

// ============================================================================
// Global Mount Violation Flag
// ============================================================================

use std::sync::OnceLock;

/// Global flag for mount boundary violations detected by worker threads.
///
/// When a computation detects that a path crosses a filesystem mount boundary
/// (different st_dev than expected), it sets this flag. The Witch checks this
/// on each tick() and latches into read-only mode if set.
///
/// This is a OnceLock because once a violation is detected, it's permanent
/// for this process lifetime.
static MOUNT_VIOLATION: OnceLock<String> = OnceLock::new();

/// Report a mount boundary violation from a worker thread.
///
/// Called by computations when they detect a path with a different st_dev
/// than the expected root filesystem. The Witch will pick this up on next
/// tick() and latch into read-only mode.
pub fn report_mount_violation(reason: String) {
    let _ = MOUNT_VIOLATION.set(reason);
}

/// Check if a mount violation has been reported.
///
/// Returns the violation reason if one has been reported.
fn check_mount_violation() -> Option<&'static str> {
    MOUNT_VIOLATION.get().map(|s| s.as_str())
}

// ============================================================================
// The Witch
// ============================================================================

/// The Witch - enforcer of orderliness, guarantor of data integrity.
///
/// She provides a parallel task queue with state machine semantics,
/// ensuring all mutations flow through proper witness channels.
pub struct Witch {
    // Rayon-based task execution with channel for results
    result_tx: Sender<TaskResult>,
    result_rx: Receiver<TaskResult>,

    // Work state machine (carries session counters inline)
    work_state: WorkState,

    // Reasoning and inode awareness state
    reasoning_level: ReasoningLevel,
    inode_awareness: InodeAwarenessLevel,

    /// Whether legacy library observation is enabled.
    /// Derived from Config at construction time.
    legacy_enabled: bool,

    /// When true, mutations are permanently disabled (read-only debug mode).
    /// Set from config at startup.
    read_only_mode: bool,

    /// Runtime safety latch: if Some, mutations are permanently disabled for this session.
    /// Contains the reason why the safety latch was triggered (e.g., mount boundary violation).
    /// Once set, cannot be unset - operator must fix the issue and restart MM.
    safety_latch_reason: Option<String>,

    /// Accumulated recomputation scope from mutations this session.
    /// Used to determine whether re-awakening is needed and which content
    /// analysis computations to spawn.
    session_recomputation_scope: crate::meta::recomputation::RecomputationScope,

    /// Recomputation scope carried across re-awakening phases (observation → awakening → awake).
    /// None = run all content analysis (startup/initial awakening).
    /// Some(scope) = filter content analysis to these domains post-mutation.
    pending_recomputation_scope: Option<crate::meta::recomputation::RecomputationScope>,

    /// Force verification of all indexed files at startup, bypassing mtime optimization.
    /// Catches out-of-band tag changes and corrupt files.
    force_check_all_files_at_startup: bool,

    // Status tracking (survives across sessions)
    recent_errors: VecDeque<String>,
    task_counts: HashMap<String, usize>,

    // Transaction state
    pending_transaction: Option<PendingTransaction>,

    /// Remaining mutation phases from a staged transaction.
    /// Populated by `confirm_transaction()`, drained by `transition_to_completed()`.
    pending_mutation_phases: VecDeque<(crate::meta::mutations::MutationExecutionStage, Vec<Mutation>)>,

    /// Handle to the dedicated DB write thread.
    /// Provides stats access and shutdown coordination.
    /// Spawned at construction time — always present.
    db_thread_handle: DbThreadHandle,

    // -------------------------------------------------------------------------
    // Worker Performance Stats (Thread-Safe, Isolated)
    // -------------------------------------------------------------------------

    /// Thread-safe worker stats in separate heap allocation.
    /// None when timing instrumentation is disabled.
    worker_stats_shared: Option<Arc<SharedWorkerStats>>,

    /// Handle for the dedicated cache thread (periodic refreshes + one-shot queries).
    /// Spawned in new(), shut down in Drop.
    cache_thread_handle: cache_thread::CacheThreadHandle,

    /// Channel for sending typed notices to the UI layer.
    notice_tx: std::sync::mpsc::Sender<WitchNotice>,

    /// Decision key kinds with staged decisions in the active transaction.
    /// Used by the insights view to hide entries already handled.
    handled_sources: std::collections::HashSet<crate::meta::decisions::DecisionKeyKind>,

    /// Corpus inodes observed on disk during the current observation cycle.
    /// Accumulated from ScanCorpusDirectory results in tick().
    /// Consumed by queue_awakening_computations() via std::mem::take().
    observed_corpus_inodes: HashMap<i64, String>,
    /// Inbox inodes observed on disk during the current observation cycle.
    /// Accumulated from ScanCorpusDirectory results in tick().
    /// Consumed by queue_awakening_computations() via std::mem::take().
    observed_inbox_inodes: HashMap<i64, String>,

    /// Library files observed on disk during the current awakening cycle.
    /// Accumulated from ScanLibraryDirectory results in tick().
    /// Consumed by transition_to_completed() when awakening first drains,
    /// which queues ReconcileLibraryFiles with this data.
    observed_library_files: Vec<derivation::ObservedLibraryFile>,

    /// Whether library reconciliation has completed in the current awakening cycle.
    /// Used for two-stage awakening transition:
    ///   - First drain (false): queue ReconcileLibraryFiles, stay in Awakening
    ///   - Second drain (true): transition to Awake normally
    library_reconciliation_done: bool,

    /// Handle to the dedicated logging thread for shutdown coordination.
    log_thread_handle: Option<crate::logging::LogThreadHandle>,

    /// Shared config reference for runtime config updates.
    /// Set after construction via `set_shared_config()`.
    shared_config: Option<SharedConfig>,

    /// When the Witch entered Idle state while Awake. For idle rescan timer.
    idle_since: Option<Instant>,

    /// True while an idle rescan is in progress.
    idle_rescan_active: bool,

    /// UI-controlled gate: true only on safe browsing views (lateral ring).
    idle_rescan_eligible: bool,

    /// Handle for the autonomous external fetch thread (AcoustID lookups).
    /// None when no API key is configured or shared_config not yet available.
    external_fetch: Option<external_fetch::ExternalFetchHandle>,

    /// Latest progress snapshot from the external fetch thread.
    /// Updated each tick from Progress results; cleared when a new batch starts.
    fetch_progress: Option<external_fetch::FetchProgress>,
}

impl Witch {
    /// Linger duration for completed session display.
    const LINGER_DURATION: Duration = Duration::from_secs(30);

    pub fn new(cfg: &Config, log_rx: Option<std::sync::mpsc::Receiver<crate::logging::LogOp>>) -> (Self, cache_thread::CacheHandle, std::sync::mpsc::Receiver<WitchNotice>) {
        // Spawn the logging thread if we have the receiver
        let log_thread_handle = log_rx.map(crate::logging::spawn_log_thread);

        // Use rayon's global thread pool with work-stealing for better performance.
        // Thread count from config (default: 2x logical cores for I/O-bound workloads).
        let num_threads = config::get_worker_thread_count();

        // Configure rayon's global thread pool (only first call takes effect)
        let _ = rayon::ThreadPoolBuilder::new()
            .num_threads(num_threads)
            .build_global();

        crate::logging::log_general(format!(
            "[WORKER] Using rayon thread pool with {} threads",
            num_threads
        ));

        // Channel for receiving task results
        let (result_tx, result_rx) = mpsc::channel();

        // Spawn db_thread early. Migrations coexist with an idle db_thread — they open
        // their own write connections on rayon threads, and in WAL mode concurrent
        // connections work with busy_timeout. The db_thread sits idle on recv() during migrations.
        crate::logging::log_general("[WITCH] Spawning db_thread");

        // Create isolated worker stats only when timing instrumentation is enabled
        let worker_stats_shared = if config::is_timing_enabled() {
            Some(Arc::new(SharedWorkerStats::new()))
        } else {
            None
        };

        // Spawn the dedicated cache thread
        let (cache_ui_handle, cache_witch_handle) = cache_thread::spawn();

        // Create notice channel for Witch → UI notifications
        let (notice_tx, notice_rx) = mpsc::channel();

        let she = Self {
            result_tx,
            result_rx,
            work_state: WorkState::Idle,
            reasoning_level: ReasoningLevel::None,
            inode_awareness: InodeAwarenessLevel::None,
            legacy_enabled: cfg.legacy_enabled,
            read_only_mode: false,
            safety_latch_reason: None,
            session_recomputation_scope: crate::meta::recomputation::RecomputationScope::EMPTY,
            pending_recomputation_scope: None,
            force_check_all_files_at_startup: false, // Set via with_opinions()
            recent_errors: VecDeque::with_capacity(5),
            task_counts: HashMap::new(),
            pending_transaction: None,
            pending_mutation_phases: VecDeque::new(),
            db_thread_handle: write_thread::spawn(),
            worker_stats_shared,
            cache_thread_handle: cache_witch_handle,
            notice_tx,
            handled_sources: std::collections::HashSet::new(),
            observed_corpus_inodes: HashMap::new(),
            observed_inbox_inodes: HashMap::new(),
            observed_library_files: Vec::new(),
            library_reconciliation_done: false,
            log_thread_handle,
            shared_config: None,
            idle_since: None,
            idle_rescan_active: false,
            idle_rescan_eligible: false,
            external_fetch: None,
            fetch_progress: None,
        };

        // DEBUG: Verify initialization (only when timing enabled)
        if let Some(ref stats) = she.worker_stats_shared {
            let (_tasks, total_qw, max_qw) = stats.debug_values();
            crate::logging::log_perf(format!(
                "[PERF INIT] Witch::new() - total_queue_wait_ms={}, max_queue_wait_ms={}, total_processed={}",
                total_qw, max_qw, she.work_state.processed()
            ));
        }

        (she, cache_ui_handle, notice_rx)
    }

    /// Create a new Witch with opinions applied.
    pub fn with_opinions(cfg: &Config, read_only_mode: bool, force_check_all_files_at_startup: bool, log_rx: Option<std::sync::mpsc::Receiver<crate::logging::LogOp>>) -> (Self, cache_thread::CacheHandle, std::sync::mpsc::Receiver<WitchNotice>) {
        let (mut she, cache_handle, notice_rx) = Self::new(cfg, log_rx);
        she.read_only_mode = read_only_mode;
        she.force_check_all_files_at_startup = force_check_all_files_at_startup;
        if force_check_all_files_at_startup {
            crate::logging::log_general(
                "[WITCH] force_check_all_files_at_startup=true: will verify all indexed files at startup"
            );
        }
        (she, cache_handle, notice_rx)
    }

    // -------------------------------------------------------------------------
    // Shared Config
    // -------------------------------------------------------------------------

    /// Store the shared config reference after construction.
    ///
    /// Called from `run_menu()` after both App and Witch are created.
    pub fn set_shared_config(&mut self, shared: SharedConfig) {
        self.shared_config = Some(shared);
    }

    /// Replace the in-memory config with a new version (after config edit mutation).
    ///
    /// Write-locks briefly; safe because tick() and render() are sequential on main thread.
    pub fn update_shared_config(&self, new_config: Config) {
        if let Some(ref shared) = self.shared_config {
            let mut guard = shared.write().expect("SharedConfig lock poisoned");
            *guard = new_config;
        }
    }

    // -------------------------------------------------------------------------
    // Eye and Observation State
    // -------------------------------------------------------------------------

    /// Get current reasoning level for UI rendering decisions.
    pub fn reasoning_level(&self) -> ReasoningLevel {
        self.reasoning_level
    }

    /// Check if inode checking is currently in progress.
    pub fn is_checking_inodes(&self) -> bool {
        matches!(self.inode_awareness, InodeAwarenessLevel::Checking)
    }

    /// Set the UI-controlled idle rescan eligibility flag.
    ///
    /// Should be true only on lateral browsing views (Insights, CorpusBrowser,
    /// TagSearch, Inbox). False during modals, resolution flows, or transactions.
    pub fn set_idle_rescan_eligible(&mut self, eligible: bool) {
        self.idle_rescan_eligible = eligible;
    }

    /// Check if an idle rescan is currently in progress.
    pub fn idle_rescan_active(&self) -> bool {
        self.idle_rescan_active
    }

    /// Check if mutations are currently accepted.
    ///
    /// Mutations are only accepted when:
    /// - Eye state is Awake (observing and awakening complete)
    /// - Not in read-only mode (config setting)
    /// - Safety latch not triggered (runtime invariant violation)
    /// - No idle rescan in progress
    ///
    /// This is derived state - mutations are automatically blocked during
    /// re-awakening cycles after mutations drain, and during idle rescans.
    fn accepting_mutations(&self) -> bool {
        self.reasoning_level == ReasoningLevel::Full
            && !self.read_only_mode
            && self.safety_latch_reason.is_none()
            && !self.idle_rescan_active
    }

    /// Trigger the safety latch, permanently disabling mutations for this session.
    ///
    /// This is a one-way operation - once latched, cannot be unlatched.
    /// The operator must fix the underlying issue and restart MM.
    ///
    /// Called when a runtime invariant is violated (e.g., mount boundary crossed).
    pub fn latch_read_only_for_safety(&mut self, reason: String) {
        if self.safety_latch_reason.is_none() {
            crate::logging::log_error(format!(
                "[WITCH] SAFETY LATCH TRIGGERED: {}",
                reason
            ));
            let _ = self.notice_tx.send(WitchNotice::SafetyLatch(reason.clone()));
            self.safety_latch_reason = Some(reason);
        }
    }

    /// Start observing. Returns false if observing already in progress.
    ///
    /// Derives paths from the global resolver and stored config.
    /// Queues WalkCorpus computations for corpus and optional legacy library.
    pub fn start_observing(&mut self) -> bool {
        if self.is_checking_inodes() {
            return false;
        }

        self.inode_awareness = InodeAwarenessLevel::Checking;
        self.queue_observing_computations();
        true
    }

    /// Queue observing computations (internal helper).
    fn queue_observing_computations(&mut self) {
        let resolver = crate::corpus::paths::get_resolver();
        let force_check = self.force_check_all_files_at_startup;

        // Clear accumulated observation state before fresh scan
        self.observed_corpus_inodes.clear();
        self.observed_inbox_inodes.clear();
        self.observed_library_files.clear();
        self.library_reconciliation_done = false;

        // Queue corpus walk
        self.queue_computation_with_label(
            Computation::Observation(observation::Computation::WalkCorpus {
                root: resolver.corpus_dir(),
                zone: "corpus".to_string(),
                force_check,
            }),
            Some("Observing corpus".to_string()),
        );

        // Queue inbox walk if inbox directory exists
        let inbox_dir = resolver.inbox_dir();
        if inbox_dir.is_dir() {
            self.queue_computation_with_label(
                Computation::Observation(observation::Computation::WalkCorpus {
                    root: inbox_dir,
                    zone: "inbox".to_string(),
                    force_check,
                }),
                Some("Observing inbox".to_string()),
            );
        }

        // Queue legacy library walk if enabled
        if self.legacy_enabled {
            self.queue_computation_with_label(
                Computation::Observation(observation::Computation::WalkCorpus {
                    root: resolver.libraries_dir().join("legacy"),
                    zone: "legacy".to_string(),
                    force_check,
                }),
                Some("Observing legacy".to_string()),
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
    pub fn tick(&mut self) {
        // Check for mount boundary violations reported by worker threads
        if let Some(reason) = check_mount_violation() {
            self.latch_read_only_for_safety(reason.to_string());
        }

        // DEBUG: Log first tick state (only when timing enabled)
        if let Some(ref stats) = self.worker_stats_shared {
            let (_, total_qw_before, _) = stats.debug_values();
            if self.work_state.processed() == 0 && total_qw_before != 0 {
                crate::logging::log_perf(format!(
                    "[PERF BUG] tick() called with total_processed=0 but total_queue_wait_ms={}!",
                    total_qw_before
                ));
            }
        }

        // Drain completed results and collect spawned computations and mutations
        let mut spawned_computations: Vec<Computation> = Vec::new();
        let mut spawned_mutations: Vec<types::SpawnedMutation> = Vec::new();

        while let Ok(result) = self.result_rx.try_recv() {
            // Task has completed - no longer in flight
            self.work_state.dec_in_flight();
            self.work_state.inc_processed();

            // Track by task type
            *self.task_counts.entry(result.label.clone()).or_insert(0) += 1;

            // Decrement pending count for this label
            self.work_state.dec_label(&result.label);

            // Record stats via thread-safe interface (only when timing enabled)
            let current_processed = self.work_state.processed();
            if let Some(ref stats) = self.worker_stats_shared {
                stats.record_result(&result);

                // DEBUG: Log queue wait values
                let (_, total_qw_now, max_qw_now) = stats.debug_values();
                if current_processed == 1 {
                    crate::logging::log_perf(format!(
                        "[PERF FIRST] FIRST TASK: queue_wait_ms={}, total_queue_wait_ms={} (should equal queue_wait_ms!), max_queue_wait_ms={}",
                        result.queue_wait_ms, total_qw_now, max_qw_now
                    ));
                }
                if current_processed <= 50 || current_processed.is_multiple_of(500) || result.queue_wait_ms > 50000 {
                    crate::logging::log_perf(format!(
                        "[PERF DEBUG] queue_wait_ms={} for task={}, total_processed={}, total_queue_wait_ms={}, max_queue_wait_ms={}",
                        result.queue_wait_ms, result.label, current_processed, total_qw_now, max_qw_now
                    ));
                }
            }

            if !result.success {
                if let Some(err) = result.error {
                    let _ = self.notice_tx.send(WitchNotice::Error(err.clone()));
                    if self.recent_errors.len() >= 5 {
                        self.recent_errors.pop_front();
                    }
                    self.recent_errors.push_back(err);
                }
            }

            // Apply config update if present (from ApplyConfigEdits mutation)
            if let Some(new_config) = result.config_update {
                self.update_shared_config(new_config);
                let _ = self.notice_tx.send(WitchNotice::ConfigUpdated);
            }

            // Accumulate recomputation scope from mutation results
            if !result.recomputation_scope.is_empty() {
                self.session_recomputation_scope |= result.recomputation_scope;
            }

            // Accumulate observed inodes from ScanCorpusDirectory results
            self.observed_corpus_inodes.extend(result.observed_corpus_inodes);
            self.observed_inbox_inodes.extend(result.observed_inbox_inodes);

            // Accumulate observed library files from ScanLibraryDirectory results
            self.observed_library_files.extend(result.observed_library_files);

            // Collect spawned follow-up computations and mutations
            spawned_computations.extend(result.spawn);
            spawned_mutations.extend(result.spawn_mutations);
        }

        // Queue spawned follow-up computations (chaining)
        // IMPORTANT: This happens BEFORE we check in_flight for state transitions,
        // ensuring spawned tasks are counted before we decide to transition.
        for comp in spawned_computations {
            self.queue_computation_with_label(comp, None);
        }

        // Queue spawned follow-up mutations (chaining from mutations like ApplyTagOps)
        // These are pre-authorized by the parent mutation's witness chain.
        for mutation in spawned_mutations {
            self.queue_spawned_mutation(mutation);
        }

        // Cache thread handles its own periodic refreshes — no action needed here.

        // Capture current state for status before transitions
        let current_in_flight = self.work_state.in_flight();
        let current_total_processed = self.work_state.processed();
        let current_session_queued = self.work_state.queued();
        let current_pending_by_label = self.work_state.by_label().cloned().unwrap_or_default();

        // State machine transitions
        self.update_state();

        // Check if idle rescan should trigger
        self.maybe_start_idle_rescan();

        // External fetch: drain results
        self.drain_external_fetch_results();

        // Emit status update to UI
        let _ = self.notice_tx.send(WitchNotice::StatusUpdate(WorkStatus {
            state: WorkStateSnapshot::from(&self.work_state),
            pending: current_in_flight,
            total_processed: current_total_processed,
            session_queued: current_session_queued,
            pending_by_label: current_pending_by_label,
            idle_rescan_active: self.idle_rescan_active,
        }));
    }

    /// Update state machine based on in-flight tasks and timing.
    fn update_state(&mut self) {
        match self.work_state {
            WorkState::Idle => {
                // Idle → Working: handled in queue methods
            }
            WorkState::Working { processed, .. } => {
                // Working → Done: when all work finishes (no in-flight tasks AND
                // db_thread queue empty). Uses centralized has_pending() for consistency
                // with exit handlers and UI state display.
                if !self.has_pending() && processed > 0 {
                    self.transition_to_completed();
                }
            }
            WorkState::Done { finished_at, .. } => {
                // Done → Idle: after linger timeout
                if finished_at.elapsed() >= Self::LINGER_DURATION {
                    self.transition_to_idle();
                }
                // Done → Working: handled in queue methods
            }
        }
    }

    fn transition_to_completed(&mut self) {
        // Phase advancement: if there are more mutation phases from a staged
        // transaction, drain the db_thread queue, then queue the next phase.
        // Stay in Working state — more work to do.
        if let Some((stage, mutations)) = self.pending_mutation_phases.pop_front() {
            crate::logging::log_mutation(format!(
                "[TRANSACTION] Phase advancement: draining db_thread, then queueing {:?} ({} mutations). \
                 {} phase(s) remaining.",
                stage, mutations.len(), self.pending_mutation_phases.len()
            ));
            write_thread::wait_for_queue_drain();

            // Extract label from current WorkState before queueing (preserves session label)
            let label = if let WorkState::Working { ref label, .. } = self.work_state {
                label.clone()
            } else {
                None
            };
            self.queue_mutations_internal(mutations, label);
            return; // Don't transition to Done — more phases to execute
        }

        // Mutations with non-empty scope need re-awakening. Mutations with EMPTY
        // scope (AcknowledgeMtimeOnly, operational config edits) don't — their
        // post-execution pipeline already handles everything they need.
        let had_mutations = !self.session_recomputation_scope.is_empty();

        // Flag to queue awakening computations after session reset
        let mut queue_awakening_after_reset = false;
        // Flag to queue idle-rescan-only awakening (corpus+inbox signals, no second-level)
        let mut queue_idle_rescan_awakening_after_reset = false;
        // Flag to auto-queue content analysis after mutations drain
        let mut queue_content_analysis_after_reset = false;
        // Flag to queue re-observation (WalkCorpus) after mutations complete
        let mut queue_reobservation_after_reset = false;
        // Flag to queue ReconcileLibraryFiles after awakening stage 1
        let mut queue_reconcile_library_after_reset = false;
        let mut reconcile_library_observed: Option<Vec<derivation::ObservedLibraryFile>> = None;

        // Extract session counters before transitioning
        let session_processed = match &self.work_state {
            WorkState::Working { processed, .. } => *processed,
            _ => 0,
        };

        // State transition based on (is_checking_inodes, reasoning_level) tuple
        // All combinations explicitly handled; invalid states panic
        match (self.is_checking_inodes(), self.reasoning_level) {
            // Observing completed while None: begin Inodes
            (true, ReasoningLevel::None) => {
                self.inode_awareness = InodeAwarenessLevel::Done;
                crate::logging::log_general(format!(
                    "[STATE] Observing complete. Transitioning None -> Inodes. \
                     Processed {} tasks.",
                    session_processed
                ));
                self.reasoning_level = ReasoningLevel::Inodes;
                queue_awakening_after_reset = true;
            }

            // Re-observing completed while Full: sync signals via awakening
            (true, ReasoningLevel::Full) => {
                self.inode_awareness = InodeAwarenessLevel::Done;
                if self.idle_rescan_active {
                    crate::logging::log_general(format!(
                        "[STATE] Idle rescan observation complete. Queueing lightweight signal derivation. \
                         Processed {} tasks.",
                        session_processed
                    ));
                    queue_idle_rescan_awakening_after_reset = true;
                } else {
                    crate::logging::log_general(format!(
                        "[STATE] Re-observing complete while Full. Queueing awakening to sync signals. \
                         Processed {} tasks.",
                        session_processed
                    ));
                    queue_awakening_after_reset = true;
                }
            }

            // Re-walk completed during Inodes: proceed to derivations
            (true, ReasoningLevel::Inodes) => {
                self.inode_awareness = InodeAwarenessLevel::Done;
                crate::logging::log_general(format!(
                    "[STATE] Re-observation complete during Inodes. Queueing derivations. \
                     Processed {} tasks.",
                    session_processed
                ));
                queue_awakening_after_reset = true;
            }

            // Inodes completed: two-stage transition
            // Stage 1: Queue ReconcileLibraryFiles, stay in Inodes
            // Stage 2: Transition to Full and queue content analysis
            (false, ReasoningLevel::Inodes) => {
                if !self.library_reconciliation_done {
                    // Stage 1: Library reconciliation not yet done
                    self.library_reconciliation_done = true;
                    let observed = std::mem::take(&mut self.observed_library_files);
                    crate::logging::log_general(format!(
                        "[STATE] Inodes stage 1 complete. Queueing ReconcileLibraryFiles ({} observed files). \
                         Staying in Inodes. Processed {} tasks.",
                        observed.len(), session_processed
                    ));
                    queue_reconcile_library_after_reset = true;
                    reconcile_library_observed = Some(observed);
                } else {
                    // Stage 2: Library reconciliation done, NOW transition to Full
                    self.library_reconciliation_done = false;
                    crate::logging::log_general(format!(
                        "[STATE] Inodes stage 2 complete. Transitioning Inodes -> Full. \
                         Processed {} tasks.",
                        session_processed
                    ));
                    self.reasoning_level = ReasoningLevel::Full;

                    if !self.read_only_mode {
                        crate::logging::log_general("[STATE] Mutations now enabled (read-write mode).");
                    }

                    // Queue content analysis after full awakening
                    queue_content_analysis_after_reset = true;
                }
            }

            // Normal operation: work completed while Full
            (false, ReasoningLevel::Full) => {
                if self.idle_rescan_active {
                    // Idle rescan signal derivation complete
                    crate::logging::log_general(format!(
                        "[STATE] Idle rescan complete. Processed {} tasks.",
                        session_processed
                    ));
                    self.idle_rescan_active = false;
                } else if had_mutations {
                    // Mutations ran - re-validate everything via re-awakening
                    let _ = self.notice_tx.send(WitchNotice::MutationsCompleted);
                    self.cache_thread_handle.invalidate_all();
                    crate::logging::log_general(format!(
                        "[STATE] Mutations complete (scope={:?}). Transitioning Full -> Inodes for re-validation. \
                         Processed {} tasks.",
                        self.session_recomputation_scope, session_processed
                    ));
                    // Carry the accumulated scope into the pending slot for
                    // ScheduleContentAnalysis to consume after re-awakening.
                    self.pending_recomputation_scope = Some(
                        std::mem::replace(
                            &mut self.session_recomputation_scope,
                            crate::meta::recomputation::RecomputationScope::EMPTY,
                        )
                    );
                    self.reasoning_level = ReasoningLevel::Inodes;
                    self.inode_awareness = InodeAwarenessLevel::Checking;
                    queue_reobservation_after_reset = true;
                }
                // If no mutations and not idle rescan, stay Full (normal work completion)
            }

            // Migrations can complete while None - this is valid, just NOP
            (false, ReasoningLevel::None) => {
                let had_migrations = self.task_counts.keys().any(|k| k.starts_with("Migration"));
                if had_migrations {
                    crate::logging::log_general(format!(
                        "[STATE] Migrations complete while None. Staying None. \
                         Processed {} tasks.",
                        session_processed
                    ));
                } else {
                    panic!(
                        "Invalid state: non-observing, non-migration work completed while reasoning is None. \
                         The only work while None should be observing or migrations."
                    );
                }
            }
        }

        // Transition to Done state
        self.work_state = WorkState::Done {
            finished_at: Instant::now(),
            total_processed: session_processed,
        };
        self.task_counts.clear();
        self.recent_errors.clear();
        self.session_recomputation_scope = crate::meta::recomputation::RecomputationScope::EMPTY;

        // Flush all pending db_thread writes before queueing the next phase.
        // Computation tasks fire writes asynchronously via db_thread (fire-and-forget).
        // The task completes when the worker returns, NOT when db_thread commits the
        // writes. Without this barrier, the next phase's computations could read stale
        // data (e.g., DeriveDeployHealthSignals reading library files written by
        // ScanLibraryDirectory, or Awake-phase computations reading Awakening signals).
        if queue_awakening_after_reset || queue_idle_rescan_awakening_after_reset || queue_content_analysis_after_reset || queue_reobservation_after_reset || queue_reconcile_library_after_reset {
            write_thread::wait_for_queue_drain();
        }

        // Queue follow-up computations AFTER reset to fix off-by-one counting
        // (if queued before reset, the task's queue count gets wiped but it still completes)
        if queue_reobservation_after_reset {
            self.queue_reobservation_computations();
        }
        if queue_awakening_after_reset {
            self.queue_awakening_computations();
        }
        if queue_idle_rescan_awakening_after_reset {
            self.queue_idle_rescan_awakening();
        }
        if queue_reconcile_library_after_reset {
            if let Some(observed) = reconcile_library_observed {
                self.queue_computation_with_label(
                    Computation::Derivation(derivation::Computation::ReconcileLibraryFiles {
                        observed_files: observed,
                    }),
                    Some("Reconciling library files".to_string()),
                );
            }
        }
        if queue_content_analysis_after_reset {
            // Create witness here - this is the ONLY valid call site
            let witness = ContentAnalysisWitness::new();
            self.queue_content_analysis(witness);
        }
    }

    /// Queue Awakening-phase computations.
    ///
    /// Takes the accumulated observed inode maps and queues DeriveCorpusSignals
    /// and DeriveInboxSignals directly with the data. Also queues
    /// ScheduleSecondLevelDerivations for directory checks and library walks.
    fn queue_awakening_computations(&mut self) {
        let observed_corpus = std::mem::take(&mut self.observed_corpus_inodes);
        let observed_inbox = std::mem::take(&mut self.observed_inbox_inodes);

        crate::logging::log_general(format!(
            "[STATE] Queueing Awakening: DeriveCorpusSignals ({} inodes), DeriveInboxSignals ({} inodes), ScheduleSecondLevelDerivations",
            observed_corpus.len(), observed_inbox.len()
        ));

        self.queue_computation_with_label(
            Computation::Derivation(derivation::Computation::DeriveCorpusSignals {
                observed_inodes: observed_corpus,
            }),
            Some("Deriving corpus signals".to_string()),
        );

        self.queue_computation_with_label(
            Computation::Derivation(derivation::Computation::DeriveInboxSignals {
                observed_inodes: observed_inbox,
            }),
            Some("Deriving inbox signals".to_string()),
        );

        // Directory checks + library walks
        self.queue_computation_with_label(
            Computation::Derivation(derivation::Computation::ScheduleSecondLevelDerivations),
            Some("Computing directory signals".to_string()),
        );
    }

    /// Check if conditions are met to start an idle rescan.
    ///
    /// Called from `tick()` after `update_state()`. All gates:
    /// - State is Idle, eye is Awake
    /// - `idle_rescan_eligible` (UI on lateral view)
    /// - No active transaction
    /// - Config interval > 0 and timer expired
    fn maybe_start_idle_rescan(&mut self) {
        if !self.work_state.is_idle() || self.reasoning_level != ReasoningLevel::Full {
            return;
        }
        if !self.idle_rescan_eligible {
            return;
        }
        // In open-txn mode, idle rescans are always allowed (they're read-only observation).
        // In closed-txn mode, block if a transaction is open (user is actively reviewing).
        let open_txn_mode = self.shared_config.as_ref()
            .map(|sc| sc.read().expect("SharedConfig lock poisoned").opinions.leave_transactions_open)
            .unwrap_or(false);
        if !open_txn_mode && self.pending_transaction.is_some() {
            return;
        }

        let interval_secs = self.shared_config.as_ref()
            .map(|sc| sc.read().expect("SharedConfig lock poisoned").opinions.idle_rescan_interval_secs)
            .unwrap_or(0);
        if interval_secs == 0 {
            return;
        }

        let idle_since = match self.idle_since {
            Some(t) => t,
            None => return,
        };
        if idle_since.elapsed() < Duration::from_secs(interval_secs) {
            return;
        }

        // All gates passed — start idle rescan
        crate::logging::log_general(format!(
            "[STATE] Starting idle rescan (idle for {}s, interval={}s)",
            idle_since.elapsed().as_secs(), interval_secs
        ));

        self.idle_rescan_active = true;
        self.idle_since = None;
        self.inode_awareness = InodeAwarenessLevel::Checking;

        // Clear accumulated observation state before fresh scan
        self.observed_corpus_inodes.clear();
        self.observed_inbox_inodes.clear();

        let resolver = crate::corpus::paths::get_resolver();

        // Queue corpus walk (mtime-optimized)
        self.queue_computation_with_label(
            Computation::Observation(observation::Computation::WalkCorpus {
                root: resolver.corpus_dir(),
                zone: "corpus".to_string(),
                force_check: false,
            }),
            Some("Rescanning corpus".to_string()),
        );

        // Queue inbox walk if directory exists (mtime-optimized)
        let inbox_dir = resolver.inbox_dir();
        if inbox_dir.is_dir() {
            self.queue_computation_with_label(
                Computation::Observation(observation::Computation::WalkCorpus {
                    root: inbox_dir,
                    zone: "inbox".to_string(),
                    force_check: false,
                }),
                Some("Rescanning inbox".to_string()),
            );
        }
    }

    /// Queue lightweight awakening computations for idle rescan.
    ///
    /// Takes the accumulated observed inode maps and queues DeriveCorpusSignals
    /// and DeriveInboxSignals. Does NOT queue ScheduleSecondLevelDerivations
    /// (no library walks, no directory-level checks).
    fn queue_idle_rescan_awakening(&mut self) {
        let observed_corpus = std::mem::take(&mut self.observed_corpus_inodes);
        let observed_inbox = std::mem::take(&mut self.observed_inbox_inodes);

        crate::logging::log_general(format!(
            "[STATE] Queueing idle rescan awakening: DeriveCorpusSignals ({} inodes), DeriveInboxSignals ({} inodes)",
            observed_corpus.len(), observed_inbox.len()
        ));

        self.queue_computation_with_label(
            Computation::Derivation(derivation::Computation::DeriveCorpusSignals {
                observed_inodes: observed_corpus,
            }),
            Some("Deriving corpus signals".to_string()),
        );

        self.queue_computation_with_label(
            Computation::Derivation(derivation::Computation::DeriveInboxSignals {
                observed_inodes: observed_inbox,
            }),
            Some("Deriving inbox signals".to_string()),
        );
    }

    // =========================================================================
    // External Fetch Integration
    // =========================================================================

    /// Drain results from the external fetch thread and write them to the DB.
    ///
    /// Called each tick(). Non-blocking: processes whatever results are available.
    fn drain_external_fetch_results(&mut self) {
        let results = match self.external_fetch {
            Some(ref mut handle) => handle.drain_results(),
            None => return,
        };

        let sender = match write_thread::signal_sender() {
            Some(s) => s,
            None => return,
        };

        for result in results {
            match result {
                external_fetch::FetchResult::Matches {
                    inode, fingerprint, source, recordings, raw_response,
                } => {
                    let now = SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);

                    for row in &recordings {
                        sender.insert_external_match(
                            inode,
                            fingerprint.clone(),
                            source.to_key(),
                            &row.recording_id,
                            row.confidence,
                            raw_response.clone(),
                            now,
                        );
                    }
                    // Clear any retry entry for this inode
                    sender.delete_external_retry(inode, source.to_key());
                }
                external_fetch::FetchResult::NoMatch {
                    inode, fingerprint, source,
                } => {
                    let now = SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);

                    sender.insert_external_no_match(fingerprint, source.to_key(), now);
                    // Clear any retry entry for this inode
                    sender.delete_external_retry(inode, source.to_key());
                }
                external_fetch::FetchResult::NeedsRetry {
                    inode, fingerprint, source, error,
                } => {
                    sender.upsert_external_retry(
                        inode,
                        fingerprint,
                        source.to_key(),
                        &error,
                    );
                }
                external_fetch::FetchResult::Progress(p) => {
                    self.fetch_progress = Some(p);
                }
                external_fetch::FetchResult::BatchDone {
                    source, processed, matched, no_match, retries,
                } => {
                    // Store final snapshot so last-batch summary stays visible
                    self.fetch_progress = Some(external_fetch::FetchProgress {
                        total: processed,
                        processed,
                        matched,
                        no_match,
                        retries,
                    });
                    crate::logging::log_general(format!(
                        "[FETCH] {} batch done: {} processed, {} matched, {} no-match, {} retries",
                        source.name(), processed, matched, no_match, retries
                    ));
                    if matched > 0 {
                        self.session_recomputation_scope |= crate::meta::recomputation::RecomputationScope::EXTERNAL;
                    }
                }
            }
        }
    }

    /// Trigger an external fetch (operator-initiated).
    ///
    /// Requires an API key, eligible dirs, and no active batch.
    pub fn request_external_fetch(&mut self) {
        let shared_config = match self.shared_config {
            Some(ref sc) => sc.clone(),
            None => {
                crate::logging::log_general("[WITCH] External fetch: no config available");
                return;
            }
        };

        let (api_key, eligible_dirs) = {
            let config = shared_config.read().expect("SharedConfig lock poisoned");
            let key = config.opinions.external_matching.acoustid_api_key.clone();
            let dirs: Vec<std::path::PathBuf> = config.source_dirs.iter()
                .filter(|sd| sd.enable_acoustid)
                .map(|sd| sd.path.clone())
                .collect();
            (key, dirs)
        };

        if api_key.is_empty() {
            crate::logging::log_general("[WITCH] External fetch: no API key configured");
            return;
        }
        if eligible_dirs.is_empty() {
            crate::logging::log_general("[WITCH] External fetch: no eligible directories");
            return;
        }

        // Lazy-spawn the fetch thread if needed
        if self.external_fetch.is_none() {
            self.external_fetch = Some(external_fetch::ExternalFetchHandle::spawn(shared_config));
            crate::logging::log_general("[WITCH] Spawned external fetch thread");
        }

        let handle = self.external_fetch.as_mut().unwrap();

        if handle.is_batch_active() {
            crate::logging::log_general("[WITCH] External fetch: batch already active");
            return;
        }

        // Clear stale progress from last batch
        self.fetch_progress = None;

        crate::logging::log_general(format!(
            "[WITCH] Manual external fetch requested for {} eligible dirs",
            eligible_dirs.len()
        ));

        handle.request_refresh(
            crate::meta::external::ExternalSource::AcoustID,
            eligible_dirs,
        );
    }

    /// Whether an external AcoustID fetch batch is currently active.
    pub fn is_external_fetch_active(&self) -> bool {
        self.external_fetch.as_ref().map_or(false, |h| h.is_batch_active())
    }

    /// Latest progress snapshot from the external fetch thread.
    pub fn external_fetch_progress(&self) -> Option<&external_fetch::FetchProgress> {
        self.fetch_progress.as_ref()
    }

    /// Whether an AcoustID API key is configured.
    pub fn has_acoustid_api_key(&self) -> bool {
        self.shared_config.as_ref().map_or(false, |sc| {
            let config = sc.read().expect("SharedConfig lock poisoned");
            !config.opinions.external_matching.acoustid_api_key.is_empty()
        })
    }

    /// Queue re-observation computations (WalkCorpus) for re-awakening after mutations.
    ///
    /// Similar to `queue_observing_computations` but for re-awakening cycles.
    /// Uses mtime optimization (`force_check: false`) since mutations that changed
    /// files will naturally trigger verification via mtime changes.
    fn queue_reobservation_computations(&mut self) {
        let resolver = crate::corpus::paths::get_resolver();

        crate::logging::log_general(
            "[STATE] Queueing re-observation computations for post-mutation re-awakening"
        );

        // Clear accumulated observation state before fresh scan
        self.observed_corpus_inodes.clear();
        self.observed_inbox_inodes.clear();
        self.observed_library_files.clear();
        self.library_reconciliation_done = false;

        // Re-walk corpus (mtime-optimized)
        self.queue_computation_with_label(
            Computation::Observation(observation::Computation::WalkCorpus {
                root: resolver.corpus_dir(),
                zone: "corpus".to_string(),
                force_check: false,
            }),
            Some("Re-observing corpus".to_string()),
        );

        // Re-walk inbox if directory exists (mtime-optimized)
        let inbox_dir = resolver.inbox_dir();
        if inbox_dir.is_dir() {
            self.queue_computation_with_label(
                Computation::Observation(observation::Computation::WalkCorpus {
                    root: inbox_dir,
                    zone: "inbox".to_string(),
                    force_check: false,
                }),
                Some("Re-observing inbox".to_string()),
            );
        }

        // Re-walk legacy library if enabled (mtime-optimized)
        if self.legacy_enabled {
            self.queue_computation_with_label(
                Computation::Observation(observation::Computation::WalkCorpus {
                    root: resolver.libraries_dir().join("legacy"),
                    zone: "legacy".to_string(),
                    force_check: false,
                }),
                Some("Re-observing legacy".to_string()),
            );
        }
    }

    /// Queue content analysis computations (internal only).
    ///
    /// This queues `ScheduleContentAnalysis` which will spawn bulk detection
    /// computations for FingerprintOverlap, CrossSourceOverlap, MissingTag, etc.
    ///
    /// Only callable from `transition_to_completed` when mutations drain while Awake.
    /// Sealed by requiring `ContentAnalysisWitness` which can only be created in that context.
    fn queue_content_analysis(&mut self, _witness: ContentAnalysisWitness) {
        let scope = self.pending_recomputation_scope.take();

        crate::logging::log_general(format!(
            "[STATE] Queueing ScheduleContentAnalysis for content analysis (scope={:?})",
            scope
        ));

        // Queue the orchestrator computation that will spawn all detection computations
        self.queue_computation_with_label(
            Computation::Analysis(analysis::Computation::ScheduleContentAnalysis { scope }),
            Some("Analyzing metadata".to_string()),
        );
    }

    fn transition_to_idle(&mut self) {
        self.work_state = WorkState::Idle;
        if self.reasoning_level == ReasoningLevel::Full {
            self.idle_since = Some(Instant::now());
        }
    }

    fn transition_to_working(&mut self) {
        if !self.work_state.is_working() {
            self.work_state = WorkState::Working {
                queued: 0,
                in_flight: 0,
                processed: 0,
                by_label: HashMap::new(),
                label: None,
            };
        }
    }

    // -------------------------------------------------------------------------
    // Task Spawning Helper
    // -------------------------------------------------------------------------

    /// Spawn a task on the rayon thread pool with panic catching.
    ///
    /// If the task panics, we still send a failure result so the Witch's
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
                        spawn_mutations: Vec::new(),
                        duration_ms: queue_time.elapsed().as_millis() as u64,
                        queue_wait_ms: 0,
                        thread_stats: None,
                        config_update: None,
                        recomputation_scope: crate::meta::recomputation::RecomputationScope::EMPTY,
                        observed_corpus_inodes: HashMap::new(),
                        observed_inbox_inodes: HashMap::new(),
                        observed_library_files: Vec::new(),
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
            .or_else(|| {
                if let WorkState::Working { ref label, .. } = self.work_state {
                    label.clone()
                } else {
                    None
                }
            })
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

    pub(super) fn queue_mutations_internal(&mut self, mutations: impl IntoIterator<Item = Mutation>, label: Option<String>) {
        self.transition_to_working();

        let queue_time = Instant::now();
        let mutations: Vec<_> = mutations.into_iter().collect();

        crate::logging::log_general(format!(
            "[WORKER] queue_mutations_internal: queueing {} mutations (label={:?})",
            mutations.len(), label
        ));

        let count = mutations.len();
        self.work_state.inc_queued(count);
        for _ in 0..count {
            self.work_state.inc_in_flight();
        }

        // Store label in WorkState for phase advancement
        if let WorkState::Working { label: ref mut ws_label, .. } = self.work_state {
            if ws_label.is_none() {
                *ws_label = label.clone();
            }
        }

        for mutation in mutations {
            let task = Task::Mutation(mutation);
            let task_label = self.resolve_label(label.clone(), &task);
            self.work_state.inc_label(&task_label);
            self.spawn_task(task, task_label, queue_time);
        }
    }

    /// Queue a mutation spawned by another mutation (spawn chaining).
    ///
    /// This is called internally from tick() when processing spawn_mutations
    /// from completed tasks. The spawn chain is already authorized by the
    /// parent mutation's witness - no additional operator decision required.
    ///
    /// Example: ApplyTagOps spawns ApplyDbTagsToDisk after DB write succeeds.
    fn queue_spawned_mutation(&mut self, spawned: types::SpawnedMutation) {
        // Extract the inner mutation - SpawnedMutation's existence proves authorization
        let mutation = spawned.into_inner();

        // Spawned mutations inherit the working state from their parent
        // (transition_to_working already happened when parent was queued)

        let task = Task::Mutation(mutation);
        let task_label = TaskLabel::from_task(&task).0;

        self.work_state.inc_queued(1);
        self.work_state.inc_in_flight();
        self.work_state.inc_label(&task_label);
        self.spawn_task(task, task_label, Instant::now());
    }

    // -------------------------------------------------------------------------
    // Computation Queueing (internal only, no witness required)
    // -------------------------------------------------------------------------

    /// Queue a single computation with an optional label.
    ///
    /// Computations are derived facts that don't alter state - they only emit
    /// signals. They can execute without user decisions.
    fn queue_computation_with_label(&mut self, computation: Computation, label: Option<String>) {
        self.transition_to_working();

        let task = Task::Computation(computation);
        let task_label = self.resolve_label(label, &task);

        self.work_state.inc_queued(1);
        self.work_state.inc_in_flight();
        self.work_state.inc_label(&task_label);
        self.spawn_task(task, task_label, Instant::now());
    }

    // -------------------------------------------------------------------------
    // Maintenance Task Queueing (bypasses accepting_mutations gate)
    // -------------------------------------------------------------------------

    /// Queue a maintenance task for async execution on the rayon pool.
    ///
    /// Maintenance tasks bypass the `accepting_mutations` gate and can run
    /// before observing completes. Called only after operator approval.
    fn queue_maintenance(&mut self, task: crate::meta::maintenance::DbMaintenanceTask) {
        self.transition_to_working();

        let task = Task::Maintenance(task);
        let task_label = self.resolve_label(None, &task);

        self.work_state.inc_queued(1);
        self.work_state.inc_in_flight();
        self.work_state.inc_label(&task_label);
        self.spawn_task(task, task_label, Instant::now());
    }

    // -------------------------------------------------------------------------
    // Migration-Aware Startup Methods
    // -------------------------------------------------------------------------

    /// Check if migrations are needed.
    ///
    /// Returns true if the database exists and has pending schema migrations.
    pub fn needs_migrations(&self) -> bool {
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return false,
        };
        matches!(
            InitialUiState::determine(&db_path),
            InitialUiState::MigrationRequired
        )
    }

    /// Get pending migration descriptions for UI display.
    ///
    /// Returns a list of human-readable descriptions of pending migrations.
    pub fn pending_migration_descriptions(&self) -> Vec<String> {
        use crate::db::ReadOnlyDb;
        use crate::meta::mutations::MigrationRegistry;

        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        let db = match Database::open_read_only(&db_path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };
        let read_db = ReadOnlyDb::new(&db);
        MigrationRegistry::new().pending_descriptions(&read_db)
    }

    /// Queue all pending migrations for async execution.
    ///
    /// Requires a `ConfirmationGesture` from the MigrationApproval view.
    pub fn queue_pending_migrations(
        &mut self,
        _gesture: &crate::meta::decisions::ConfirmationGesture,
    ) {
        use crate::meta::maintenance::DbMaintenanceTask;
        use crate::meta::mutations::MigrationRegistry;

        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                crate::logging::log_error(format!(
                    "[WITCH] queue_pending_migrations: no db path: {}",
                    e
                ));
                return;
            }
        };
        let db = match Database::open_read_only(&db_path) {
            Ok(d) => d,
            Err(e) => {
                crate::logging::log_error(format!(
                    "[WITCH] queue_pending_migrations: failed to open db: {}",
                    e
                ));
                return;
            }
        };
        let read_db = crate::db::ReadOnlyDb::new(&db);

        let registry = MigrationRegistry::new();
        let current_version = read_db.get_schema_version().unwrap_or(1);
        let pending = registry.pending_migrations(current_version);

        if pending.is_empty() {
            crate::logging::log_general("[WITCH] No migrations to queue");
            return;
        }

        crate::logging::log_general(format!(
            "[WITCH] Queueing {} migrations (operator approved)",
            pending.len()
        ));

        for m in pending {
            self.queue_maintenance(DbMaintenanceTask::Migration {
                migration_id: m.to_version,
                description: m.description.to_string(),
            });
        }
    }

    /// Queue a VACUUM for async execution.
    ///
    /// Requires a `ConfirmationGesture` from the VacuumPrompt view.
    /// Drops the cached read-only connection first (VACUUM needs exclusive access).
    pub fn queue_vacuum(
        &mut self,
        _gesture: &crate::meta::decisions::ConfirmationGesture,
    ) {
        use crate::meta::maintenance::DbMaintenanceTask;

        self.queue_maintenance(DbMaintenanceTask::Vacuum);
    }

    // -------------------------------------------------------------------------
    // Utility Methods
    // -------------------------------------------------------------------------

    /// Get current Witch status snapshot (readonly, does not advance state).
    ///
    /// Safe to call from render code, utility functions, etc.
    /// Use `tick()` only from the main event loop to advance state.
    pub fn status(&self) -> WorkStatus {
        WorkStatus {
            state: WorkStateSnapshot::from(&self.work_state),
            pending: self.work_state.in_flight(),
            total_processed: self.work_state.processed(),
            session_queued: self.work_state.queued(),
            pending_by_label: self.work_state.by_label().cloned().unwrap_or_default(),
            idle_rescan_active: self.idle_rescan_active,
        }
    }

    /// Check if there's pending work (tasks queued or in-flight).
    pub fn has_pending(&self) -> bool {
        // Check rayon in-flight tasks
        if self.work_state.in_flight() > 0 {
            return true;
        }
        // Check db_thread queue
        if !self.db_thread_handle.queue_empty() {
            return true;
        }
        false
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
            crate::logging::log_perf(format!(
                "[PERF BUG] avg > max! tasks={}, avg={}, max={}",
                stats.tasks_completed, stats.queue_wait_avg_ms, stats.queue_wait_max_ms
            ));
        }

        Some(stats)
    }

    /// Get the decision key kinds that have been handled in the active transaction.
    ///
    /// Used by the insights view to hide entries already staged.
    pub fn handled_decision_kinds(&self) -> &std::collections::HashSet<crate::meta::decisions::DecisionKeyKind> {
        &self.handled_sources
    }

    /// Rebuild handled_sources from current transaction state.
    ///
    /// Called internally after transaction mutations (add, remove, confirm, discard).
    pub(crate) fn sync_handled_sources(&mut self) {
        self.handled_sources = self.pending_transaction
            .as_ref()
            .map(|txn| txn.decisions.keys().filter_map(|k| k.kind()).collect())
            .unwrap_or_default();
    }
}

impl Drop for Witch {
    fn drop(&mut self) {
        // Shutdown order is critical for SQLite WAL cleanup:
        // 0. Shut down external fetch thread (owns a read-only connection)
        // 1. Shut down cache thread (owns a read-only connection)
        // 2. Close thread-local read-only connections on rayon workers
        // 3. Close the Witch's cached read-only connection
        // 4. DB thread checkpoints WAL and closes write connection
        // 5. With all connections closed, SQLite cleans up -wal and -shm files

        // Step 0: Shut down external fetch thread if active.
        if let Some(ref mut handle) = self.external_fetch {
            handle.shutdown();
        }

        // Step 1: Shut down cache thread and wait for it to exit.
        // The cache thread owns a read-only DB connection that must close
        // before WAL checkpoint.
        self.cache_thread_handle.shutdown();

        // Step 2: Close thread-local read-only connections on all rayon worker threads
        // These are cached per-thread and must be explicitly closed
        rayon::broadcast(|_| {
            crate::meta::computations::close_thread_local_connection();
        });
        crate::logging::log_general("[WITCH] Closed all rayon thread-local DB connections");

        // Step 3: Shut down the DB thread (it will checkpoint and close write connection)
        crate::db::write_thread::request_shutdown();
        self.db_thread_handle.join();

        // Step 5: Shut down the logging thread last so all shutdown messages get logged
        crate::logging::request_shutdown();
        if let Some(ref mut handle) = self.log_thread_handle {
            handle.join();
        }
    }
}

// Note: Witch no longer implements Default because it requires Config for PathResolver
