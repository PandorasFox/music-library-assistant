//! The Witch - Main-thread task execution and orderliness enforcement
//!
//! The Witch is named such because She enforces orderliness in her domain,
//! and provides all guarantees for data which flows properly through Her.
//!
//! ## Module Organization
//! - `types.rs` - Core types, enums, witness system, zone observation structs
//! - `state_machine.rs` - Work/Done/Idle transitions, reasoning level advancement
//! - `dispatch.rs` - Protocol routing, auth gate, first-time setup
//! - `event_handlers.rs` - Task result, watcher, and scheduler message processing
//! - `queue.rs` - Task/mutation/computation/maintenance queueing pipeline
//! - `status.rs` - Status snapshots and utility queries
//! - `startup.rs` - Schema reconciliation, vacuum, auto-indexing
//! - `transaction.rs` - Transaction lifecycle management
//! - `execution.rs` - Task execution (mutations, computations, maintenance)
//!
//! ## Extending the Witch
//! - New task types: Add variants to `Task` enum in `types.rs`
//! - New execution logic: Add to `execution.rs`
//! - State transitions: Modify `state_machine.rs`
//! - Transaction features: Modify `transaction.rs`
//! - Protocol commands: Add to `dispatch.rs`
//! - New event sources: Add handlers in `event_handlers.rs`

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::config::{self, Config, SharedConfig};
use crate::db::write_thread::{self, DbThreadHandle};
use crate::db::Database;
use crate::meta::computations::{analysis, Computation};
use crate::meta::mutations::Mutation;

// Module declarations
pub(crate) mod auth_thread;
pub(crate) mod cache_thread;
mod dispatch;
mod event_handlers;
mod execution;
mod offload_handlers;
pub(crate) mod external_fetch;
pub(crate) mod fs_thread;
mod hades;
mod handle;
mod pipeline_triggers;
mod queue;
pub(crate) mod socket;
mod auto_deploy;
mod startup;
mod state_machine;
mod status;
mod transaction;
pub(crate) mod types;
// Re-export public types
pub use types::{
    MaintenanceWitness, MutationExecutionWitness, ObservedInodeMeta, PendingTransaction,
    ReasoningLevel, SpawnedMutation, TransactionSnapshot, WatcherState,
    WitchStatus, WorkState, WorkStateSnapshot, WorkStatus,
};
// Decision authority: TUI gates decisions via ConfirmationGesture (ui/action_handlers/witness.rs),
// producing Decision objects (meta/decisions/mod.rs) that cross the protocol boundary.
// See operator_decisions.rs for call sites.

// Internal imports
use types::ObservedInodes;

/// Build a DB cache for the watcher's initial scan (blocking, off main thread).
///
/// Opens its own read-only connection and queries all corpus file mtimes + tags.
/// The watcher compares disk mtime → if match, uses cached tags (skips disk read).
fn build_watcher_db_cache_blocking() -> HashMap<i64, fs_thread::CachedInodeState> {
    let db_path = match config::get_db_path() {
        Ok(p) => p,
        Err(_) => return HashMap::new(),
    };
    let db = match Database::open_read_only(&db_path) {
        Ok(d) => d,
        Err(_) => return HashMap::new(),
    };

    let corpus_mtimes = db
        .get_all_file_mtimes(crate::db::types::Zone::Corpus)
        .unwrap_or_default();

    let all_tags = db.get_all_tags_ordered().unwrap_or_default();
    let mut tags_by_inode: HashMap<i64, Vec<(String, String)>> = HashMap::new();
    for (inode, tag_name, tag_value) in all_tags {
        tags_by_inode
            .entry(inode)
            .or_default()
            .push((tag_name, tag_value));
    }

    let mut cache = HashMap::new();
    for (inode, (mtime_secs, mtime_nanos)) in &corpus_mtimes {
        let tags = tags_by_inode
            .remove(inode)
            .map(crate::corpus::tags::TagSet::new)
            .unwrap_or_else(crate::corpus::tags::TagSet::empty);
        cache.insert(
            *inode,
            fs_thread::CachedInodeState {
                mtime_secs: *mtime_secs,
                mtime_nanos: *mtime_nanos,
                tags,
            },
        );
    }

    crate::logging::log_general(format!(
        "[WITCH] DB cache seeded: {} entries ({} corpus)",
        cache.len(),
        corpus_mtimes.len()
    ));

    cache
}

/// Async recv on an `Option<UnboundedReceiver>`. Returns `None` (pending forever)
/// if the option is `None`, avoiding borrow-checker issues in `select!`.
async fn recv_optional<T>(
    rx: &mut Option<tokio::sync::mpsc::UnboundedReceiver<T>>,
) -> Option<T> {
    match rx.as_mut() {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

// ============================================================================
// The Witch
// ============================================================================

/// The Witch - enforcer of orderliness, guarantor of data integrity.
///
/// She provides a parallel task queue with state machine semantics,
/// ensuring all mutations flow through proper witness channels.
pub struct Witch {
    // Startup state — lifecycle from AwaitingSetup through maintenance to Ready
    startup_state: types::WitchStartupState,

    /// Vacuum freelist ratio threshold from config (0.0 = disabled).
    vacuum_threshold: f64,

    /// Human-readable descriptions of pending schema changes (for progress display).
    /// Populated once at startup if Reconciling, cleared on transition to Ready.
    startup_schema_descriptions: Vec<String>,

    // Pipeline overseer thread — owns the rayon pool and result channel
    hades: hades::HadesHandle,

    // Work state machine (carries session counters inline)
    work_state: WorkState,

    // Reasoning and inode awareness state
    reasoning_level: ReasoningLevel,
    watcher_state: WatcherState,

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
    kind_counts: HashMap<types::TaskKind, usize>,

    // Transaction state
    pending_transaction: Option<PendingTransaction>,

    /// Remaining mutation phases from a staged transaction.
    /// Populated by `confirm_transaction()`, drained by `update_state()`.
    pending_mutation_phases: VecDeque<(
        crate::meta::mutations::MutationExecutionStage,
        Vec<Mutation>,
    )>,

    /// Remaining computation phases from a staged pipeline (e.g., release packing).
    /// Populated by `tick()` from computation result `deferred_phases`, drained by
    /// `transition_to_completed()` after mutation phases.
    pending_computation_phases:
        VecDeque<(crate::meta::computations::PipelineStage, Vec<Computation>)>,

    /// Computations waiting for external fetch to complete before being queued.
    /// Populated by `handle_fetch_requests()`, drained when fetch scheduler reports done.
    post_fetch_computations: Vec<Computation>,

    /// Pipeline actions pending after work completes.
    ///
    /// FETCH, PACKING, AUDIO_DEPLOY are consumed at idle (30s linger + priority chain).
    /// SIDECAR_DEPLOY fires eagerly from `transition_to_completed`.
    pending_work: types::PendingWork,

    /// Remaining soft mutation phases from auto-deploy. Drained by `update_state()`
    /// after the current phase completes (same pattern as `pending_mutation_phases`).
    pending_soft_mutation_phases:
        std::collections::VecDeque<(mm_meta::soft_mutations::SoftMutationPhase, Vec<mm_meta::soft_mutations::SoftMutation>)>,

    /// Handle to the dedicated DB write thread.
    /// Provides stats access and shutdown coordination.
    /// Spawned at construction time — always present.
    db_thread_handle: DbThreadHandle,

    // -------------------------------------------------------------------------
    // Worker Performance Stats (Thread-Safe, Isolated)
    // -------------------------------------------------------------------------
    /// Thread-safe worker stats in separate heap allocation.
    /// None when timing instrumentation is disabled.

    /// Handle for the dedicated cache thread (periodic refreshes + one-shot queries).
    /// Spawned in new(), shut down in Drop.
    cache_thread_handle: cache_thread::CacheThreadHandle,

    // -- Generation counters (monotonically increasing, published in WitchStatus) --
    /// Increments when a mutation batch completes.
    mutations_generation: u64,
    /// Increments when a computation batch completes.
    computations_generation: u64,
    /// Increments when a new task error occurs.
    error_generation: u64,
    /// Increments when config is mutated.
    config_generation: u64,

    /// Broadcast channel for pushing status events to all connected clients.
    /// Per-connection handlers subscribe via the socket listener.
    event_tx: tokio::sync::broadcast::Sender<mm_meta::witch_types::WitchEvent>,

    /// Decision key kinds with staged decisions in the active transaction.
    /// Used by the insights view to hide entries already handled.
    handled_sources: std::collections::HashSet<crate::meta::decisions::DecisionKeyKind>,

    /// Authoritative inode→path maps for watched zones (corpus + library).
    /// Populated by watcher initial scan, updated incrementally by steady-state events.
    /// Cloned (not taken) when queuing derivation computations.
    observed_inodes: ObservedInodes,


    /// Handle to the dedicated logging thread for shutdown coordination.
    log_thread_handle: Option<crate::logging::LogThreadHandle>,

    /// Shared config reference for runtime config updates.
    /// Set after construction via `set_shared_config()`.
    shared_config: Option<SharedConfig>,

    /// Lock-free config snapshot for hot-path reads (publish_status, etc.).
    /// Updated alongside shared_config. Reads use arc_swap::Guard — no locks.
    config_snapshot: arc_swap::ArcSwap<Option<std::sync::Arc<Config>>>,

    /// Set by watcher events (FileCreated/FileRemoved) during steady state.
    /// When true and Witch is idle, queues a derivation pass to reconcile
    /// observed inodes against the index.
    watcher_derivation_needed: bool,

    /// Images observed by the watcher, pending indexing.
    /// Accumulated from ImageFileObserved messages, drained when a batch
    /// is queued as an IndexObservedImages computation.
    pending_observed_images: Vec<fs_thread::ObservedImage>,

    /// Handle to the filesystem watcher thread.
    /// Spawned at construction time — always present.
    fs_watcher: fs_thread::FsThreadHandle,

    /// Handle to the auth thread for lifecycle management.
    /// Spawned in `run()`, before the UI thread.
    auth_thread_handle: Option<auth_thread::AuthThreadHandle>,

    /// Handle to the Unix domain socket listener thread.
    /// Spawned in `run()` for out-of-process client connections.
    socket_listener_handle: Option<socket::SocketListenerHandle>,

    /// Scheduler message receiver, stored separately from ExternalFetchHandle
    /// so the select! loop can borrow it independently.
    scheduler_message_rx: Option<tokio::sync::mpsc::UnboundedReceiver<external_fetch::SchedulerMessage>>,

    /// Client-facing auth handle for server-side token validation.
    /// Stored on the Witch so dispatch_command can gate protocol messages.
    auth_handle: Option<auth_thread::AuthHandle>,

    /// Handle for the autonomous external fetch thread (AcoustID lookups).
    /// None when no API key is configured or shared_config not yet available.
    external_fetch: Option<external_fetch::ExternalFetchHandle>,

    /// Latest progress snapshot from the external fetch thread.
    /// Updated each tick from Progress results; cleared when a new batch starts.
    fetch_progress: Option<external_fetch::FetchProgress>,

    /// Latest progress snapshot from the cover art fetch.
    cover_art_progress: Option<external_fetch::CoverArtProgress>,

    /// Latest progress snapshot from the Deezer ISRC-keyed fetch.
    deezer_progress: Option<mm_meta::witch_types::DeezerProgress>,

    // -------------------------------------------------------------------------
    // Offload Infrastructure (async spawn_blocking results)
    // -------------------------------------------------------------------------

    /// Sender for spawn_blocking tasks to return results to the main loop.
    offload_tx: tokio::sync::mpsc::UnboundedSender<types::OffloadResult>,
    /// Receiver polled in the select! loop for offloaded results.
    offload_rx: tokio::sync::mpsc::UnboundedReceiver<types::OffloadResult>,

    /// True when watcher DB cache is being built asynchronously.
    watcher_cache_loading: bool,
    /// Deferred watcher command waiting for the cache build to complete.
    pending_watcher_command: Option<types::PendingWatcherCommand>,

    /// True while first-time setup is running in a blocking task.
    setup_in_progress: bool,

    /// True while vacuum check is running in a blocking task.
    vacuum_check_pending: bool,

    // -------------------------------------------------------------------------
    // Idle Full-Repack Promotion
    // -------------------------------------------------------------------------
    /// True if at least one incremental release-packing pass has been queued
    /// since the last full repack. Cleared when a full repack is queued.
    /// Drives the idle-timer auto-promotion in `decide_idle_action`.
    incremental_since_full_repack: bool,

    /// Set when the work state transitions into Idle, cleared when it leaves
    /// Idle. Drives the elapsed-time guard for idle-timer auto-promotion.
    last_idle_entry_at: Option<Instant>,
}

impl Witch {
    /// Linger duration for completed session display.
    const LINGER_DURATION: Duration = Duration::from_secs(30);

    fn new(
        startup_state: types::WitchStartupState,
        initial_config: Option<Config>,
        log_rx: Option<std::sync::mpsc::Receiver<crate::logging::LogOp>>,
    ) -> Self {
        // Spawn the logging thread if we have the receiver
        let log_thread_handle = log_rx.map(crate::logging::spawn_log_thread);

        // Spawn Hades — the pipeline overseer thread that owns the rayon pool.
        // Uses a dedicated ThreadPool instance (not the global pool).
        // Config is None during AwaitingSetup (no config on disk yet).
        let hades = hades::HadesHandle::spawn(initial_config);

        // Spawn db_thread early. Migrations coexist with an idle db_thread — they open
        // their own write connections on rayon threads, and in WAL mode concurrent
        // connections work with busy_timeout. The db_thread sits idle on recv() during migrations.
        crate::logging::log_general("[WITCH] Spawning db_thread");

        // Spawn the DB read thread
        let cache_witch_handle = cache_thread::spawn();

        // Spawn the filesystem watcher thread
        let fs_watcher_handle = fs_thread::FsThreadHandle::spawn();

        let (offload_tx, offload_rx) = tokio::sync::mpsc::unbounded_channel();

        Self {
            startup_state,
            vacuum_threshold: 0.0,
            startup_schema_descriptions: Vec::new(),
            hades,
            work_state: WorkState::Idle,
            reasoning_level: ReasoningLevel::None,
            watcher_state: WatcherState::NotStarted,
            session_recomputation_scope: crate::meta::recomputation::RecomputationScope::EMPTY,
            pending_recomputation_scope: None,
            force_check_all_files_at_startup: false, // Set via with_opinions()
            recent_errors: VecDeque::with_capacity(5),
            task_counts: HashMap::new(),
            kind_counts: HashMap::new(),
            pending_transaction: None,
            pending_mutation_phases: VecDeque::new(),
            pending_computation_phases: VecDeque::new(),
            post_fetch_computations: Vec::new(),
            pending_work: types::PendingWork::EMPTY,
            pending_soft_mutation_phases: std::collections::VecDeque::new(),
            db_thread_handle: write_thread::spawn(),
            cache_thread_handle: cache_witch_handle,
            mutations_generation: 0,
            computations_generation: 0,
            error_generation: 0,
            config_generation: 0,
            event_tx: tokio::sync::broadcast::channel(16).0,
            handled_sources: std::collections::HashSet::new(),
            observed_inodes: ObservedInodes::new(),
            log_thread_handle,
            shared_config: None,
            config_snapshot: arc_swap::ArcSwap::from_pointee(None),
            watcher_derivation_needed: false,
            pending_observed_images: Vec::new(),
            fs_watcher: fs_watcher_handle,
            auth_thread_handle: None, // Spawned in run(), not new()
            auth_handle: None,       // Set in run() alongside auth_thread_handle
            socket_listener_handle: None, // Spawned in run()
            scheduler_message_rx: None,
            external_fetch: None,
            fetch_progress: None,
            cover_art_progress: None,
            deezer_progress: None,
            offload_tx,
            offload_rx,
            watcher_cache_loading: false,
            pending_watcher_command: None,
            setup_in_progress: false,
            vacuum_check_pending: false,
            incremental_since_full_repack: false,
            last_idle_entry_at: None,
        }
    }

    /// Create a new Witch with opinions applied (Ready state).
    fn with_opinions(
        cfg: &Config,
        log_rx: Option<std::sync::mpsc::Receiver<crate::logging::LogOp>>,
    ) -> Self {
        let mut she = Self::new(types::WitchStartupState::Ready, Some(cfg.clone()), log_rx);
        she.force_check_all_files_at_startup =
            cfg.opinions.startup.force_check_all_files_at_startup;
        she.vacuum_threshold = cfg.opinions.startup.vacuum_threshold;
        if she.force_check_all_files_at_startup {
            crate::logging::log_general(
                "[WITCH] force_check_all_files_at_startup=true: will verify all indexed files at startup"
            );
        }
        she
    }

    /// Run the Witch on the current (main) thread.
    ///
    /// Detects startup state from filesystem: if no DB exists, boots in
    /// `AwaitingSetup` and idles until a client sends `CompleteSetup`.
    /// If both config and DB exist, loads config + performance globals and
    /// runs normally.
    ///
    /// Binds the Unix domain socket and enters the run loop on the calling
    /// thread. Clients (TUI, web, tooling) connect over the socket.
    /// Does not return until a client sends Shutdown or all senders disconnect.
    pub async fn run(
        log_rx: Option<std::sync::mpsc::Receiver<crate::logging::LogOp>>,
    ) {
        // Detect startup state
        let db_path = config::get_db_path().expect("XDG data dir");
        let has_db = db_path.exists();

        let mut she = if has_db {
            // Normal startup: load config, init performance globals
            let cfg = match config::load_config() {
                Ok(cfg) => {
                    crate::logging::log_general("Config loaded successfully");
                    config::init_performance_config(cfg.opinions.performance.clone());
                    cfg
                }
                Err(e) => {
                    eprintln!("ERROR: Failed to load config\n");
                    eprintln!("{:#}", e);
                    std::process::exit(1);
                }
            };
            let mut w = Self::with_opinions(&cfg, log_rx);
            w.set_shared_config(cfg.into_shared());
            w
        } else {
            // No DB — Witch boots in AwaitingSetup
            crate::logging::log_general("[WITCH] No database found — entering AwaitingSetup");
            Self::new(types::WitchStartupState::AwaitingSetup, None, log_rx)
        };

        // Auto-detect and queue startup maintenance (schema reconciliation, vacuum).
        // Runs before the socket listener is spawned, so no clients are connected —
        // spawn_blocking().await is fine here (doesn't block the select loop).
        if she.startup_state == types::WitchStartupState::Ready {
            let vacuum_threshold = she.vacuum_threshold;
            let startup_check = tokio::task::spawn_blocking(move || {
                startup::startup_maintenance_check(vacuum_threshold)
            })
            .await
            .unwrap_or_default();

            if startup_check.needs_schema_update {
                crate::logging::log_general("[WITCH] Schema update needed — entering Reconciling");
                she.startup_schema_descriptions = startup_check.schema_descriptions;
                she.startup_state = types::WitchStartupState::Reconciling;
                she.queue_schema_reconciliation();
            } else if startup_check.needs_vacuum {
                crate::logging::log_general("[WITCH] Vacuum threshold exceeded — entering Vacuuming");
                she.startup_state = types::WitchStartupState::Vacuuming;
                she.queue_vacuum();
            }
        }

        // Spawn auth thread BEFORE socket — auth must be available for login flow.
        // If DB exists, auth thread opens a read-only connection immediately.
        // If no DB (AwaitingSetup), auth thread starts in no-DB mode and opens
        // its connection when notified via DbReady after first-time setup.
        let auth_db_path = if has_db { Some(db_path) } else { None };
        let (auth_handle, auth_witch_handle) = auth_thread::spawn(auth_db_path);
        she.auth_thread_handle = Some(auth_witch_handle);
        she.auth_handle = Some(auth_handle);

        // Command channel: socket handler threads send, Witch receives
        let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();

        // Spawn the Unix domain socket listener. Binds synchronously — the
        // socket is ready for connections when this returns.
        she.socket_listener_handle = socket::spawn_listener(cmd_tx, she.event_tx.clone());

        if she.socket_listener_handle.is_none() {
            eprintln!("ERROR: Failed to bind socket (is XDG_RUNTIME_DIR set?)");
            std::process::exit(1);
        }

        if let Some(path) = socket::socket_path() {
            println!("mm: listening on {}", path.display());
        }

        // Witch owns the main thread — blocks here until shutdown
        she.run_loop(cmd_rx).await;
    }

    /// The Witch's self-owned run loop.
    ///
    /// Processes commands, runs tick(), publishes state. Runs until Shutdown
    /// command or channel disconnection.
    ///
    /// When `startup_state == AwaitingSetup`, skips `tick()` — only drains
    /// commands and publishes status. This lets the Witch idle safely until
    /// a client delivers the setup payload.
    async fn run_loop(
        &mut self,
        mut cmd_rx: tokio::sync::mpsc::UnboundedReceiver<handle::HandleCommand>,
    ) {
        let mut housekeeping = tokio::time::interval(Duration::from_millis(100));

        loop {
            let mut broadcast_status = false;

            tokio::select! {
                Some(cmd) = cmd_rx.recv() => {
                    broadcast_status = true;
                    if self.dispatch_command(cmd) { return; }
                    while let Ok(cmd) = cmd_rx.try_recv() {
                        if self.dispatch_command(cmd) { return; }
                    }
                }
                Some(hades::HadesMessage::Result(result)) = self.hades.message_rx.recv() => {
                    broadcast_status = true;
                    self.process_task_result(result);
                    while let Ok(hades::HadesMessage::Result(r)) = self.hades.message_rx.try_recv() {
                        self.process_task_result(r);
                    }
                    self.post_drain_bookkeeping();
                }
                Some(msg) = self.fs_watcher.message_rx.recv() => {
                    broadcast_status = true;
                    self.process_watcher_message(msg);
                    while let Ok(msg) = self.fs_watcher.message_rx.try_recv() {
                        self.process_watcher_message(msg);
                    }
                }
                msg = recv_optional(&mut self.scheduler_message_rx) => {
                    broadcast_status = true;
                    let mut msgs = Vec::new();
                    if let Some(msg) = msg {
                        msgs.push(msg);
                    }
                    if let Some(ref mut rx) = self.scheduler_message_rx {
                        while let Ok(msg) = rx.try_recv() {
                            msgs.push(msg);
                        }
                    }
                    for msg in msgs {
                        self.process_scheduler_message(msg);
                    }
                }
                Some(result) = self.offload_rx.recv() => {
                    broadcast_status = true;
                    self.handle_offload_result(result);
                    while let Ok(r) = self.offload_rx.try_recv() {
                        self.handle_offload_result(r);
                    }
                }
                _ = housekeeping.tick() => {}
            }

            // After any event: run bookkeeping if not awaiting setup
            if self.startup_state != types::WitchStartupState::AwaitingSetup {
                self.update_state();
                self.check_startup_transitions();
                self.maybe_trigger_derivation();
            }

            // Push status snapshot to all connected clients after any real activity
            if broadcast_status {
                use mm_meta::witch_types::WitchEvent;
                let _ = self.event_tx.send(WitchEvent::StatusChanged(self.publish_status()));
            }
        }
    }

    // -------------------------------------------------------------------------
    // Shared Config
    // -------------------------------------------------------------------------

    /// Store the shared config reference after construction.
    ///
    /// Called from `run_tui()` after both App and Witch are created.
    pub fn set_shared_config(&mut self, shared: SharedConfig) {
        // Update lock-free snapshot for hot-path reads
        let snapshot = shared.read().expect("SharedConfig lock poisoned").clone();
        self.config_snapshot
            .store(std::sync::Arc::new(Some(std::sync::Arc::new(snapshot))));
        self.cache_thread_handle.set_config(shared.clone());
        self.shared_config = Some(shared);
    }

    /// Update performance config at runtime: propagate to Hades (pool resize),
    /// db_thread (cache_size), and cache_thread (cache_size).
    pub(super) fn update_performance_impl(&mut self, opinions: crate::config::PerformanceOpinions) {
        let new_cache_kb = -(opinions.db_cache_mb as i64 * 1024);

        // Read current config for Hades snapshot update (lock-free via ArcSwap)
        let guard = self.config_snapshot.load();
        let current_config = guard.as_deref().cloned();

        // Hades handles rayon pool rebuild + thread-local cache_size (lazy propagation)
        if let Some(ref cfg) = current_config {
            self.hades.update_config(cfg, &opinions);
        }

        // Explicit cache_size update for long-lived thread connections
        crate::db::write_thread::set_cache_size(new_cache_kb);
        self.cache_thread_handle.set_cache_size(new_cache_kb);

        crate::logging::log_general(format!(
            "[WITCH] Performance config updated (cache_kb={}, threads={:?})",
            new_cache_kb, opinions.worker_threads
        ));
    }

    /// Replace the in-memory config with a new version (after config edit mutation).
    ///
    /// Updates both the SharedConfig (for threads that hold it) and the lock-free
    /// ArcSwap snapshot (for hot-path reads on the main loop).
    pub fn update_shared_config(&self, new_config: Config) {
        // Update lock-free snapshot first (hot-path reads see it immediately)
        self.config_snapshot
            .store(std::sync::Arc::new(Some(std::sync::Arc::new(new_config.clone()))));
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

    /// Check if the watcher is performing its initial scan.
    pub fn is_initial_scanning(&self) -> bool {
        matches!(self.watcher_state, WatcherState::InitialScan)
    }

    /// Check if mutations are currently accepted.
    ///
    /// Mutations are only accepted when reasoning level is Full
    /// (observing and awakening complete). This is derived state —
    /// mutations are automatically blocked during re-awakening cycles
    /// after mutations drain.
    fn accepting_mutations(&self) -> bool {
        self.reasoning_level == ReasoningLevel::Full
    }

    /// Start watching. Returns false if already scanning or cache loading.
    ///
    /// Offloads the DB cache build to a blocking task. When the cache arrives
    /// via the offload channel, the watcher is actually started/polled.
    ///
    /// In polling mode, sends a Poll command (immediate re-walk with fresh
    /// cache) instead of Start (which would retry inotify).
    pub fn start_watching(&mut self) -> bool {
        if self.is_initial_scanning() || self.watcher_cache_loading {
            return false;
        }

        // Clear accumulated observation state before fresh scan
        self.observed_inodes.clear();

        // Determine the watcher command synchronously (cheap config reads).
        let cmd = if self.watcher_state == WatcherState::Polling {
            let interval = self.poll_interval_secs();
            types::PendingWatcherCommand::Poll { interval_secs: interval }
        } else {
            self.watcher_state = WatcherState::InitialScan;
            let guard = self.config_snapshot.load();
            let config_ref = guard.as_deref();
            types::PendingWatcherCommand::Start {
                zones: types::watched_zones(config_ref),
            }
        };

        self.watcher_cache_loading = true;
        self.pending_watcher_command = Some(cmd);

        // Offload the bulk DB queries to a blocking task.
        let tx = self.offload_tx.clone();
        tokio::task::spawn_blocking(move || {
            let cache = build_watcher_db_cache_blocking();
            let _ = tx.send(types::OffloadResult::WatcherDbCache { cache });
        });

        true
    }

    /// Get the configured poll interval (seconds) from config snapshot.
    fn poll_interval_secs(&self) -> u64 {
        self.read_config(|c| c.opinions.watcher_poll_interval_secs)
            .unwrap_or(900)
    }

    /// Trigger an async watcher cache rebuild (for config update → poll re-walk).
    ///
    /// If a cache build is already in progress, this is a no-op — the pending
    /// command will be updated when the result arrives.
    pub(super) fn request_watcher_poll_with_cache(&mut self, interval_secs: u64) {
        if self.watcher_cache_loading {
            // Already loading — update the pending command to use the new interval
            self.pending_watcher_command = Some(types::PendingWatcherCommand::Poll { interval_secs });
            return;
        }
        self.watcher_cache_loading = true;
        self.pending_watcher_command = Some(types::PendingWatcherCommand::Poll { interval_secs });
        let tx = self.offload_tx.clone();
        tokio::task::spawn_blocking(move || {
            let cache = build_watcher_db_cache_blocking();
            let _ = tx.send(types::OffloadResult::WatcherDbCache { cache });
        });
    }


    /// Trigger an external fetch (operator-initiated).
    ///
    /// Requires an API key and no active batch. Directory eligibility is
    /// resolved by the scheduler thread from config (exclusion-based).
    /// Returns `Err(reason)` if the fetch cannot start.
    pub fn request_external_fetch(&mut self) -> Result<(), String> {
        let shared_config = match self.shared_config {
            Some(ref sc) => sc.clone(),
            None => {
                return Err("Server config not yet available".to_string());
            }
        };

        {
            let config = shared_config.read().expect("SharedConfig lock poisoned");
            if config.opinions.external_matching.acoustid_api_key.is_empty() {
                return Err("No AcoustID API key configured".to_string());
            }
        }

        // Lazy-spawn the fetch thread if needed
        if self.external_fetch.is_none() {
            let (handle, rx) = external_fetch::ExternalFetchHandle::spawn(shared_config);
            self.scheduler_message_rx = Some(rx);
            self.external_fetch = Some(handle);
            crate::logging::log_general("[WITCH] Spawned external fetch thread");
        }

        let handle = self.external_fetch.as_mut().unwrap();

        if handle.is_batch_active() {
            return Err("Fetch already in progress".to_string());
        }

        // Clear stale progress from last batch
        self.fetch_progress = None;

        crate::logging::log_general("[WITCH] Manual external fetch requested");
        handle.request_fetch();
        Ok(())
    }

    /// Whether an external AcoustID fetch batch is currently active.
    pub fn is_external_fetch_active(&self) -> bool {
        self.external_fetch
            .as_ref()
            .is_some_and(|h| h.is_batch_active())
    }

    /// Latest progress snapshot from the external fetch thread.
    pub fn external_fetch_progress(&self) -> Option<&external_fetch::FetchProgress> {
        self.fetch_progress.as_ref()
    }

    /// Whether an AcoustID API key is configured.
    pub fn has_acoustid_api_key(&self) -> bool {
        self.read_config(|c| !c.opinions.external_matching.acoustid_api_key.is_empty())
            .unwrap_or(false)
    }

    /// Request a cover art fetch from the Cover Art Archive.
    ///
    /// No API key needed (CAA is public). Lazy-spawns the external fetch
    /// thread if needed. Returns `Err(reason)` if the fetch cannot start.
    pub fn request_cover_art_fetch(&mut self) -> Result<(), String> {
        let shared_config = match self.shared_config {
            Some(ref sc) => sc.clone(),
            None => return Err("Server config not yet available".to_string()),
        };
        // Lazy-spawn the fetch thread if needed
        if self.external_fetch.is_none() {
            let (handle, rx) = external_fetch::ExternalFetchHandle::spawn(shared_config);
            self.scheduler_message_rx = Some(rx);
            self.external_fetch = Some(handle);
            crate::logging::log_general("[WITCH] Spawned external fetch thread");
        }
        let handle = self.external_fetch.as_mut().unwrap();
        if handle.is_cover_art_active() {
            return Err("Cover art fetch already in progress".to_string());
        }
        self.cover_art_progress = None;
        crate::logging::log_general("[WITCH] Cover art fetch requested");
        handle.request_cover_art();
        Ok(())
    }

    /// Whether a cover art fetch is currently active.
    pub fn is_cover_art_fetch_active(&self) -> bool {
        self.external_fetch
            .as_ref()
            .is_some_and(|h| h.is_cover_art_active())
    }

    /// Request a Deezer ISRC-keyed cover art fetch.
    ///
    /// Operator-triggered (no auto-add). Lazy-spawns the external fetch
    /// thread if needed.
    pub fn request_deezer_art_fetch(&mut self) -> Result<(), String> {
        let shared_config = match self.shared_config {
            Some(ref sc) => sc.clone(),
            None => return Err("Server config not yet available".to_string()),
        };
        if self.external_fetch.is_none() {
            let (handle, rx) = external_fetch::ExternalFetchHandle::spawn(shared_config);
            self.scheduler_message_rx = Some(rx);
            self.external_fetch = Some(handle);
            crate::logging::log_general("[WITCH] Spawned external fetch thread");
        }
        let handle = self.external_fetch.as_mut().unwrap();
        if handle.is_deezer_active() {
            return Err("Deezer fetch already in progress".to_string());
        }
        self.deezer_progress = None;
        crate::logging::log_general("[WITCH] Deezer fetch requested");
        handle.request_deezer_art();
        Ok(())
    }

    /// Whether a Deezer fetch is currently active.
    pub fn is_deezer_fetch_active(&self) -> bool {
        self.external_fetch
            .as_ref()
            .is_some_and(|h| h.is_deezer_active())
    }

    /// Queue release bin-packing analysis.
    ///
    /// `incremental=false` runs a full pipeline: truncate intermediate tables,
    /// rebuild candidates and scores for every release, run mapping/MIS, emit
    /// fresh PackedRelease / ReleasePacking signals.
    ///
    /// `incremental=true` is the live-ingestion path triggered after AcoustID
    /// fetches: it re-scores only the releases reachable from inodes marked
    /// dirty for `release_packing` (plus pinned releases). It deletes and
    /// rewrites the manifest/candidates/scores rows for those releases only,
    /// leaves all other scoring data and existing signals intact, and does
    /// **not** run mapping/MIS — the next full repack (operator-initiated or
    /// idle-timer auto-promoted) reconciles the global packing decision
    /// against the updated scores.
    pub fn request_release_packing(&mut self, incremental: bool) {
        let mode = if incremental { "incremental" } else { "full" };
        crate::logging::log_general(format!(
            "[WITCH] Release packing analysis requested ({})", mode
        ));

        // Track whether incremental scoring data has accumulated since the
        // last full repack. Drives the idle-timer auto-promotion in
        // decide_idle_action: incremental sets the flag, full clears it.
        self.incremental_since_full_repack = incremental;

        self.queue_computation_with_label(
            Computation::Analysis(analysis::Computation::PackReleases { incremental }),
            Some("Release packing".to_string()),
        );
    }



}

impl Drop for Witch {
    fn drop(&mut self) {
        use types::ManagedThread;

        // Shutdown order is critical for SQLite WAL cleanup:
        // 0. Shut down external fetch thread (owns a read-only connection)
        // 1. Shut down cache thread (owns a read-only connection)
        // 2. Shut down Hades (closes thread-local connections on rayon workers, drops pool)
        // 3. DB thread checkpoints WAL and closes write connection
        // 4. Logging thread last so all shutdown messages get logged

        // Step 0: Shut down socket listener (stops accepting new connections).
        if let Some(ref mut handle) = self.socket_listener_handle {
            handle.shutdown();
        }

        // Step 0a: Shut down filesystem watcher thread.
        self.fs_watcher.shutdown();

        // Step 0b: Shut down external fetch thread if active.
        if let Some(ref mut handle) = self.external_fetch {
            handle.shutdown();
        }

        // Step 1: Shut down cache thread and wait for it to exit.
        // The cache thread owns a read-only DB connection that must close
        // before WAL checkpoint.
        self.cache_thread_handle.shutdown();

        // Step 2: Shut down Hades — closes thread-local DB connections on all
        // rayon worker threads and drops the pool.
        self.hades.shutdown();

        // Step 3: Shut down the DB thread (it will checkpoint and close write connection)
        self.db_thread_handle.shutdown();

        // Step 4: Shut down the logging thread last so all shutdown messages get logged
        if let Some(ref mut handle) = self.log_thread_handle {
            handle.shutdown();
        }
    }
}

// Note: Witch no longer implements Default because it requires Config for PathResolver
