//! The Witch - Main-thread task execution and orderliness enforcement
//!
//! The Witch is named such because She enforces orderliness in her domain,
//! and provides all guarantees for data which flows properly through Her.
//!
//! ## Module Organization
//! - `types.rs` - Core types, enums, witness system
//! - `transaction.rs` - Transaction lifecycle management
//! - `execution.rs` - Task execution (mutations, computations, maintenance)
//!
//! ## Extending the Witch
//! - New task types: Add variants to `Task` enum in `types.rs`
//! - New execution logic: Add to `execution.rs`
//! - Transaction features: Modify `transaction.rs`

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};


use std::sync::mpsc;

use crate::config::{self, Config, SharedConfig};
use crate::db::write_thread::{self, DbThreadHandle};
use crate::db::Database;
use crate::meta::computations::{analysis, derivation, Computation};
use crate::meta::mutations::Mutation;

// Module declarations
pub(crate) mod auth_thread;
pub(crate) mod cache_thread;
mod execution;
pub(crate) mod external_fetch;
pub(crate) mod fs_watcher;
mod hades;
mod handle;
pub(crate) mod socket;
mod transaction;
pub(crate) mod types;
// Re-export public types
pub use types::{
    MaintenanceWitness, MutationExecutionWitness, PendingTransaction, ReasoningLevel,
    SpawnedMutation, Task, TaskLabel, TransactionSnapshot, WatcherState,
    WitchStatus, WorkState, WorkStateSnapshot, WorkStatus,
};
// Decision authority: TUI gates decisions via ConfirmationGesture (ui/action_handlers/witness.rs),
// producing Decision objects (meta/decisions/mod.rs) that cross the protocol boundary.
// See operator_decisions.rs for call sites.

// Internal imports
use types::ContentAnalysisWitness;


// ============================================================================
// Zone-Keyed Observation State
// ============================================================================

// Re-export from mm-meta
pub use mm_meta::computations::types::ObservedInodeMeta;

/// Authoritative inode→metadata maps for all watched zones.
///
/// The watcher thread populates these maps during initial scan and updates
/// them incrementally via steady-state events. Derivation computations
/// receive cloned snapshots.
struct ObservedInodes {
    corpus: HashMap<i64, ObservedInodeMeta>,
    inbox: HashMap<i64, ObservedInodeMeta>,
    library: HashMap<i64, ObservedInodeMeta>,
}

impl ObservedInodes {
    fn new() -> Self {
        Self {
            corpus: HashMap::new(),
            inbox: HashMap::new(),
            library: HashMap::new(),
        }
    }

    /// Runtime zone dispatch — returns the inode map for the given zone.
    fn for_zone_mut(&mut self, zone: crate::db::types::Zone) -> Option<&mut HashMap<i64, ObservedInodeMeta>> {
        match zone {
            crate::db::types::Zone::Corpus => Some(&mut self.corpus),
            crate::db::types::Zone::Inbox => Some(&mut self.inbox),
            crate::db::types::Zone::Library => Some(&mut self.library),
        }
    }

    fn clear(&mut self) {
        self.corpus.clear();
        self.inbox.clear();
        self.library.clear();
    }
}

/// Build the zone→root list for watcher start commands.
///
/// Always includes corpus. Includes inbox and library if their directories
/// exist on disk.
fn watched_zones() -> Vec<(crate::db::types::Zone, std::path::PathBuf)> {
    let resolver = crate::corpus::paths::get_resolver();
    let mut zones = vec![(crate::db::types::Zone::Corpus, resolver.corpus_dir())];

    let inbox_dir = resolver.inbox_dir();
    if inbox_dir.is_dir() {
        zones.push((crate::db::types::Zone::Inbox, inbox_dir));
    }

    let libraries_dir = resolver.libraries_dir();
    if libraries_dir.is_dir() {
        zones.push((crate::db::types::Zone::Library, libraries_dir));
    }

    zones
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
    /// Increments when a new task error occurs.
    error_generation: u64,
    /// Increments when config is mutated.
    config_generation: u64,

    /// Decision key kinds with staged decisions in the active transaction.
    /// Used by the insights view to hide entries already handled.
    handled_sources: std::collections::HashSet<crate::meta::decisions::DecisionKeyKind>,

    /// Authoritative inode→path maps for watched zones (corpus + inbox).
    /// Populated by watcher initial scan, updated incrementally by steady-state events.
    /// Cloned (not taken) when queuing derivation computations.
    observed_inodes: ObservedInodes,


    /// Handle to the dedicated logging thread for shutdown coordination.
    log_thread_handle: Option<crate::logging::LogThreadHandle>,

    /// Shared config reference for runtime config updates.
    /// Set after construction via `set_shared_config()`.
    shared_config: Option<SharedConfig>,

    /// Set by watcher events (FileCreated/FileRemoved) during steady state.
    /// When true and Witch is idle, queues a derivation pass to reconcile
    /// observed inodes against the index.
    watcher_derivation_needed: bool,

    /// Images observed by the watcher, pending indexing.
    /// Accumulated from ImageFileObserved messages, drained when a batch
    /// is queued as an IndexObservedImages computation.
    pending_observed_images: Vec<fs_watcher::ObservedImage>,

    /// Handle to the filesystem watcher thread.
    /// Spawned at construction time — always present.
    fs_watcher: fs_watcher::FsWatcherHandle,

    /// Handle to the auth thread for lifecycle management.
    /// Spawned in `run()`, before the UI thread.
    auth_thread_handle: Option<auth_thread::AuthThreadHandle>,

    /// Handle to the Unix domain socket listener thread.
    /// Spawned in `run()` for out-of-process client connections.
    socket_listener_handle: Option<socket::SocketListenerHandle>,

    /// Client-facing auth handle for server-side token validation.
    /// Stored on the Witch so dispatch_command can gate protocol messages.
    auth_handle: Option<auth_thread::AuthHandle>,

    /// Handle for the autonomous external fetch thread (AcoustID lookups).
    /// None when no API key is configured or shared_config not yet available.
    external_fetch: Option<external_fetch::ExternalFetchHandle>,

    /// Latest progress snapshot from the external fetch thread.
    /// Updated each tick from Progress results; cleared when a new batch starts.
    fetch_progress: Option<external_fetch::FetchProgress>,
}

/// Deferred follow-up work accumulated during `transition_to_completed()`.
///
/// Observation-phase transitions (watcher scan completing) are now handled
/// directly in `drain_watcher_messages()`. This struct handles post-derivation
/// and post-mutation work.
#[derive(Default)]
struct PostTransitionWork {
    /// Queue ScheduleContentAnalysis after full awakening completes.
    content_analysis: bool,
}

impl PostTransitionWork {
    /// Whether any follow-up work is pending.
    fn has_work(&self) -> bool {
        self.content_analysis
    }
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
        let fs_watcher_handle = fs_watcher::FsWatcherHandle::spawn();

        let she = Self {
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
            db_thread_handle: write_thread::spawn(),
            cache_thread_handle: cache_witch_handle,
            mutations_generation: 0,
            error_generation: 0,
            config_generation: 0,
            handled_sources: std::collections::HashSet::new(),
            observed_inodes: ObservedInodes::new(),
            log_thread_handle,
            shared_config: None,
            watcher_derivation_needed: false,
            pending_observed_images: Vec::new(),
            fs_watcher: fs_watcher_handle,
            auth_thread_handle: None, // Spawned in run(), not new()
            auth_handle: None,       // Set in run() alongside auth_thread_handle
            socket_listener_handle: None, // Spawned in run()
            external_fetch: None,
            fetch_progress: None,
        };

        she
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
    pub fn run(
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

        // Auto-detect and queue startup maintenance (schema reconciliation, vacuum)
        if she.startup_state == types::WitchStartupState::Ready {
            if she.needs_schema_update() {
                crate::logging::log_general("[WITCH] Schema update needed — entering Reconciling");
                she.startup_schema_descriptions = she.pending_schema_descriptions();
                she.startup_state = types::WitchStartupState::Reconciling;
                she.queue_schema_reconciliation();
            } else if she.check_vacuum_needed() {
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
        let (cmd_tx, cmd_rx) = mpsc::channel();

        // Spawn the Unix domain socket listener. Binds synchronously — the
        // socket is ready for connections when this returns.
        she.socket_listener_handle = socket::spawn_listener(cmd_tx);

        if she.socket_listener_handle.is_none() {
            eprintln!("ERROR: Failed to bind socket (is XDG_RUNTIME_DIR set?)");
            std::process::exit(1);
        }

        if let Some(path) = socket::socket_path() {
            println!("mm: listening on {}", path.display());
        }

        // Witch owns the main thread — blocks here until shutdown
        she.run_loop(cmd_rx);
    }

    /// The Witch's self-owned run loop.
    ///
    /// Processes commands, runs tick(), publishes state. Runs until Shutdown
    /// command or channel disconnection.
    ///
    /// When `startup_state == AwaitingSetup`, skips `tick()` — only drains
    /// commands and publishes status. This lets the Witch idle safely until
    /// a client delivers the setup payload.
    fn run_loop(
        &mut self,
        cmd_rx: mpsc::Receiver<handle::HandleCommand>,
    ) {
        loop {
            // Drain all pending commands (non-blocking)
            loop {
                match cmd_rx.try_recv() {
                    Ok(cmd) => {
                        if self.dispatch_command(cmd) {
                            return;
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                }
            }

            // Advance the work loop in any state except AwaitingSetup
            // (Reconciling/Vacuuming need tick() to process maintenance tasks)
            if self.startup_state != types::WitchStartupState::AwaitingSetup {
                self.tick();
            }

            // Brief sleep to avoid busy-spinning.
            // The Witch processes at ~100Hz — faster than the TUI frame rate.
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    // -------------------------------------------------------------------------
    // Auth Gate
    // -------------------------------------------------------------------------

    /// Resolve authorization level from a session token.
    ///
    /// Two system states:
    /// - No auth_handle (no users exist) → FirstTimeSetup
    /// - Auth_handle present + valid token → Authenticated
    /// - Auth_handle present + missing token → Unauthorized
    /// - Auth_handle present + invalid token → InvalidSession
    fn resolve_auth(
        &self,
        token: Option<&crate::auth::SessionToken>,
    ) -> Result<crate::meta::protocol::AuthorizationLevel, crate::meta::protocol::ProtocolError> {
        use crate::meta::protocol::{AuthorizationLevel, ProtocolError};
        match &self.auth_handle {
            None => Ok(AuthorizationLevel::FirstTimeSetup),
            Some(auth) => match token {
                Some(t) if auth.validate_token(t) => Ok(AuthorizationLevel::Authenticated),
                Some(_) => Err(ProtocolError::InvalidSession),
                None => Err(ProtocolError::Unauthorized),
            },
        }
    }

    /// Auth gate. Fail-closed exact-match semantics.
    ///
    /// Each system state accepts ONLY its own endpoints:
    /// - FirstTimeSetup → only FirstTimeSetup endpoints (CompleteSetup)
    /// - Unauthenticated → only Unauthenticated endpoints (login, when it becomes protocol)
    /// - AuthRequired → only AuthRequired endpoints (everything operational)
    ///
    /// There is no hierarchy. An authenticated client cannot call setup endpoints.
    /// A setup-state client cannot call auth endpoints. Fail closed.
    fn gate<T>(
        &mut self,
        token: Option<&crate::auth::SessionToken>,
        required: crate::meta::protocol::AuthorizationLevel,
        handler: impl FnOnce(&mut Self) -> Result<T, crate::meta::protocol::ProtocolError>,
    ) -> Result<T, crate::meta::protocol::ProtocolError> {
        use crate::meta::protocol::ProtocolError;
        let level = self.resolve_auth(token)?;
        if level == required {
            handler(self)
        } else {
            Err(ProtocolError::Unauthorized)
        }
    }

    // -------------------------------------------------------------------------
    // Protocol Dispatch
    // -------------------------------------------------------------------------

    /// Dispatch a single command from the handle.
    /// Returns `true` if the Witch should shut down.
    fn dispatch_command(&mut self, cmd: handle::HandleCommand) -> bool {
        use crate::meta::protocol::{
            AuthResponse, AuthenticatedBody, AuthenticatedResponse, AuthorizationLevel,
            CommandPayload, CommandResponse, DecisionDetail, ProtocolError, QueryPayload,
            QueryResponse, TransactionPayload, TransactionResponse, UnauthenticatedBody,
            UnauthenticatedResponse,
        };
        use handle::HandleCommand;

        match cmd {
            HandleCommand::Authenticated { token, body, reply } => {
                // Domain queries bypass synchronous dispatch — they forward
                // the reply channel to the read thread so it replies directly.
                if let AuthenticatedBody::Query(QueryPayload::Domain(domain_payload)) = body {
                    // Validate auth before forwarding.
                    match self.resolve_auth(Some(&token)) {
                        Ok(AuthorizationLevel::Authenticated) => {
                            self.cache_thread_handle
                                .forward_domain_query(domain_payload, reply);
                            // Read thread owns the reply now — don't reply here.
                        }
                        Ok(_) | Err(_) => {
                            let _ = reply.send(Err(ProtocolError::Unauthorized));
                        }
                    }
                    return false;
                }

                let result = self.gate(Some(&token), AuthorizationLevel::Authenticated, |w| {
                    match body {
                        // -- Query area --
                        AuthenticatedBody::Query(payload) => {
                            let response = match payload {
                                QueryPayload::Status => QueryResponse::Status(w.publish_status()),
                                QueryPayload::Config => {
                                    let config = w.read_config(|c| c.clone())
                                        .ok_or(ProtocolError::NotReady)?;
                                    QueryResponse::Config(config)
                                }
                                QueryPayload::Domain(_) => {
                                    unreachable!("domain queries handled above")
                                }
                            };
                            Ok(AuthenticatedResponse::Query(response))
                        }

                        // -- Transaction area --
                        AuthenticatedBody::Transaction(payload) => {
                            let response = match payload {
                                TransactionPayload::Start { label } => {
                                    match w.start_transaction(&label) {
                                        Ok(()) => TransactionResponse::Ok,
                                        Err(e) => TransactionResponse::Error(e),
                                    }
                                }
                                TransactionPayload::AddDecision { key, decision } => {
                                    match w.add_decision(key, decision) {
                                        Ok(()) => TransactionResponse::Ok,
                                        Err(e) => TransactionResponse::Error(e),
                                    }
                                }
                                TransactionPayload::RemoveDecision { key } => {
                                    match w.remove_decision(&key) {
                                        Ok(()) => TransactionResponse::Ok,
                                        Err(e) => TransactionResponse::Error(e),
                                    }
                                }
                                TransactionPayload::Confirm => {
                                    match w.confirm_transaction() {
                                        Ok(()) => TransactionResponse::Ok,
                                        Err(e) => TransactionResponse::Error(e),
                                    }
                                }
                                TransactionPayload::Discard => {
                                    match w.discard_transaction() {
                                        Ok(summary) => TransactionResponse::Discarded(summary),
                                        Err(e) => TransactionResponse::Error(e),
                                    }
                                }
                                TransactionPayload::GetDetails => {
                                    let details = w
                                        .pending_transaction
                                        .as_ref()
                                        .map(|txn| {
                                            txn.decisions
                                                .iter()
                                                .map(|(k, d)| DecisionDetail {
                                                    key: k.clone(),
                                                    label: d.label.clone(),
                                                    mutations: d.mutations.clone(),
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    TransactionResponse::Details(details)
                                }
                            };
                            Ok(AuthenticatedResponse::Transaction(response))
                        }

                        // -- Command area --
                        AuthenticatedBody::Command(payload) => {
                            let response = match payload {
                                CommandPayload::RequestExternalFetch => {
                                    w.request_external_fetch();
                                    CommandResponse::Ok
                                }
                                CommandPayload::RequestReleasePacking => {
                                    w.request_release_packing();
                                    CommandResponse::Ok
                                }
                                CommandPayload::ValidateConfig { config } => {
                                    crate::config::validate_config(&config).map_err(|e| {
                                        ProtocolError::Internal(format!("{:#}", e))
                                    })?;
                                    CommandResponse::Ok
                                }
                                CommandPayload::SetSharedConfig { config } => {
                                    w.set_shared_config(config.into_shared());
                                    CommandResponse::Ok
                                }
                                CommandPayload::UpdatePerformance { opinions } => {
                                    w.update_performance_impl(opinions);
                                    CommandResponse::Ok
                                }
                                CommandPayload::QueueSchemaReconciliation => {
                                    w.queue_schema_reconciliation();
                                    CommandResponse::Ok
                                }
                                CommandPayload::QueueVacuum => {
                                    w.queue_vacuum();
                                    CommandResponse::Ok
                                }
                                CommandPayload::JettisonEditHistorySession { session_id } => {
                                    let now = chrono::Local::now().to_rfc3339();
                                    let mutation = Mutation::JettisonEditHistory(
                                        mm_meta::mutations::jettison::JettisonEditHistoryMutation {
                                            session_id: Some(session_id.clone()),
                                            timestamp: now,
                                        },
                                    );
                                    let label = format!("Jettison edit history: session {}", session_id);
                                    w.queue_mutations_internal(vec![mutation], Some(label));
                                    CommandResponse::Ok
                                }
                                CommandPayload::JettisonEditHistoryAll => {
                                    let now = chrono::Local::now().to_rfc3339();
                                    let mutation = Mutation::JettisonEditHistory(
                                        mm_meta::mutations::jettison::JettisonEditHistoryMutation {
                                            session_id: None,
                                            timestamp: now,
                                        },
                                    );
                                    w.queue_mutations_internal(
                                        vec![mutation],
                                        Some("Jettison edit history: all sessions".to_string()),
                                    );
                                    CommandResponse::Ok
                                }
                                CommandPayload::Shutdown => {
                                    CommandResponse::Goodbye
                                }
                            };
                            Ok(AuthenticatedResponse::Command(response))
                        }
                    }
                });
                let is_shutdown = matches!(
                    result,
                    Ok(AuthenticatedResponse::Command(CommandResponse::Goodbye))
                );
                let _ = reply.send(result);
                if is_shutdown {
                    return true;
                }
            }

            HandleCommand::Unauthenticated { body, reply } => {
                let result = match body {
                    UnauthenticatedBody::Login { username, password } => {
                        let response = match &self.auth_handle {
                            Some(auth) => {
                                match auth.login(
                                    &username,
                                    &password,
                                    crate::auth::SessionLifetime::CloseOnExit,
                                ) {
                                    Ok(token) => AuthResponse::Token(token),
                                    Err(msg) => AuthResponse::Failed(msg),
                                }
                            }
                            None => AuthResponse::Failed("Auth not available".to_string()),
                        };
                        Ok(UnauthenticatedResponse::Auth(response))
                    }
                    UnauthenticatedBody::SetupQuery => {
                        let needs_setup = self.startup_state == types::WitchStartupState::AwaitingSetup;
                        Ok(UnauthenticatedResponse::SetupStatus { needs_setup })
                    }
                    UnauthenticatedBody::CompleteSetup { root, first_user } => {
                        self.gate(
                            None,
                            AuthorizationLevel::FirstTimeSetup,
                            |w| {
                                w.complete_setup_impl(root, first_user)
                                    .map_err(ProtocolError::Internal)?;
                                Ok(UnauthenticatedResponse::SetupComplete)
                            },
                        )
                    }
                };
                let _ = reply.send(result);
            }

            HandleCommand::NotifyDbReady => {
                if let Some(ref auth_handle) = self.auth_thread_handle {
                    auth_handle.notify_db_ready();
                }
            }

            HandleCommand::Shutdown => {
                return true;
            }
        }

        false
    }

    // -------------------------------------------------------------------------
    // First-Time Setup
    // -------------------------------------------------------------------------

    /// Complete first-time setup: write config, create dirs + DB, create first user, transition to Ready.
    ///
    /// Called when a client sends CompleteSetup after the operator picks an archive root
    /// and provides first-user credentials. `first_user` is `(username, password_hash)`.
    fn complete_setup_impl(
        &mut self,
        root: std::path::PathBuf,
        first_user: Option<(String, String)>,
    ) -> Result<(), String> {
        use mm_meta::auth::FirstTimeSetupToken;

        if !config::config_exists() {
            // Fresh install — write initial config.kdl with root
            config::write_initial_config(&root).map_err(|e| format!("Failed to write config: {}", e))?;
        }

        // Load the config we just wrote
        let cfg = config::load_config().map_err(|e| format!("Failed to load config: {}", e))?;

        // Init performance globals
        config::init_performance_config(cfg.opinions.performance.clone());

        // Create subdirectories
        std::fs::create_dir_all(root.join("corpus"))
            .map_err(|e| format!("Failed to create corpus/: {}", e))?;
        std::fs::create_dir_all(root.join("libraries"))
            .map_err(|e| format!("Failed to create libraries/: {}", e))?;
        std::fs::create_dir_all(root.join("stash"))
            .map_err(|e| format!("Failed to create stash/: {}", e))?;
        std::fs::create_dir_all(root.join("inbox"))
            .map_err(|e| format!("Failed to create inbox/: {}", e))?;

        // Create database
        let db_path = config::get_db_path().map_err(|e| format!("Failed to get DB path: {}", e))?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create DB parent dir: {}", e))?;
        }
        let token = FirstTimeSetupToken::new();
        let db = crate::db::create_database(&db_path, &token)
            .map_err(|e| format!("Failed to create database: {}", e))?;

        // Create the first user if credentials were provided
        if let Some((username, plaintext_password)) = first_user {
            let password_hash = crate::auth::hash_password(&plaintext_password)
                .map_err(|e| format!("Failed to hash password: {}", e))?;
            db.create_user(&username, &password_hash)
                .map_err(|e| format!("Failed to create first user: {}", e))?;
            crate::logging::log_general(format!(
                "[WITCH] First user '{}' created",
                username
            ));
        }

        drop(db);

        // Tell cache thread to reconnect to the new DB
        self.cache_thread_handle.reconnect_db();

        // Notify auth thread that DB is now available
        if let Some(ref auth_handle) = self.auth_thread_handle {
            auth_handle.notify_db_ready();
        }

        // Apply opinions from the loaded config
        self.force_check_all_files_at_startup =
            cfg.opinions.startup.force_check_all_files_at_startup;

        // Transition to Ready
        self.startup_state = types::WitchStartupState::Ready;
        crate::logging::log_general("[WITCH] Setup complete — transitioning to Ready");

        Ok(())
    }

    // -------------------------------------------------------------------------
    // Shared Config
    // -------------------------------------------------------------------------

    /// Store the shared config reference after construction.
    ///
    /// Called from `run_tui()` after both App and Witch are created.
    pub fn set_shared_config(&mut self, shared: SharedConfig) {
        self.shared_config = Some(shared);
    }

    /// Update performance config at runtime: propagate to Hades (pool resize),
    /// db_thread (cache_size), and cache_thread (cache_size).
    fn update_performance_impl(&mut self, opinions: crate::config::PerformanceOpinions) {
        let new_cache_kb = -(opinions.db_cache_mb as i64 * 1024);

        // Read current config for Hades snapshot update
        let current_config = self.shared_config.as_ref()
            .map(|sc| sc.read().expect("SharedConfig lock poisoned").clone());

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

    /// Start watching. Returns false if already scanning.
    ///
    /// Seeds the watcher with DB-cached mtime+tags so the initial scan
    /// can skip tag reads for files with matching mtimes.
    ///
    /// In polling mode, sends a Poll command (immediate re-walk with fresh
    /// cache) instead of Start (which would retry inotify).
    pub fn start_watching(&mut self) -> bool {
        if self.is_initial_scanning() {
            return false;
        }

        // Clear accumulated observation state before fresh scan
        self.observed_inodes.clear();

        // Seed DB cache for watcher: mtime + tags per inode.
        // Watcher compares disk mtime → if match, uses cached tags (skips disk read).
        let db_cache = self.build_watcher_db_cache();

        if self.watcher_state == WatcherState::Polling {
            // In polling mode: send Poll for immediate re-walk without
            // retrying inotify. Poll interval comes from config.
            let interval = self.poll_interval_secs();
            self.fs_watcher.poll(db_cache, interval);
            crate::logging::log_general("[WITCH] Watcher poll triggered — re-walking zones");
        } else {
            self.watcher_state = WatcherState::InitialScan;
            self.fs_watcher.start(watched_zones(), db_cache);
            crate::logging::log_general("[WITCH] Watcher started — initial scan in progress");
        }

        true
    }

    /// Get the configured poll interval (seconds) from shared config.
    fn poll_interval_secs(&self) -> u64 {
        self.shared_config
            .as_ref()
            .map(|sc| {
                config::read_shared_config(sc)
                    .opinions
                    .watcher_poll_interval_secs
            })
            .unwrap_or(900)
    }

    /// Build a DB cache for the watcher's initial scan.
    ///
    /// Queries all corpus+inbox file mtimes and tags, producing a map
    /// the watcher can use to skip tag reads for files with matching mtimes.
    fn build_watcher_db_cache(&self) -> HashMap<i64, fs_watcher::CachedInodeState> {
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return HashMap::new(),
        };
        let db = match Database::open_read_only(&db_path) {
            Ok(d) => d,
            Err(_) => return HashMap::new(),
        };

        // Get mtimes for corpus and inbox files
        let corpus_mtimes = db.get_all_file_mtimes(crate::db::types::Zone::Corpus)
            .unwrap_or_default();
        let inbox_mtimes = db.get_all_file_mtimes(crate::db::types::Zone::Inbox)
            .unwrap_or_default();

        // Get all corpus tags, grouped by inode
        let all_tags = db.get_all_tags_ordered().unwrap_or_default();
        let mut tags_by_inode: HashMap<i64, Vec<(String, String)>> = HashMap::new();
        for (inode, tag_name, tag_value) in all_tags {
            tags_by_inode.entry(inode).or_default().push((tag_name, tag_value));
        }

        let mut cache = HashMap::new();

        // Build cache entries for corpus files
        for (inode, (mtime_secs, mtime_nanos)) in &corpus_mtimes {
            let tags = tags_by_inode.remove(inode)
                .map(|pairs| crate::corpus::tags::TagSet::new(pairs))
                .unwrap_or_else(crate::corpus::tags::TagSet::empty);
            cache.insert(*inode, fs_watcher::CachedInodeState {
                mtime_secs: *mtime_secs,
                mtime_nanos: *mtime_nanos,
                tags,
            });
        }

        // Build cache entries for inbox files (no tags — inbox tags are in inbox_tags table)
        for (inode, (mtime_secs, mtime_nanos)) in &inbox_mtimes {
            cache.entry(*inode).or_insert(fs_watcher::CachedInodeState {
                mtime_secs: *mtime_secs,
                mtime_nanos: *mtime_nanos,
                tags: crate::corpus::tags::TagSet::empty(),
            });
        }

        crate::logging::log_general(format!(
            "[WITCH] DB cache seeded: {} entries ({} corpus, {} inbox)",
            cache.len(), corpus_mtimes.len(), inbox_mtimes.len()
        ));

        cache
    }

    // queue_walk_computations() removed — watcher thread handles directory walking.

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
        // Drain completed results and collect spawned computations and mutations
        let mut spawned_computations: Vec<Computation> = Vec::new();
        let mut spawned_mutations: Vec<types::SpawnedMutation> = Vec::new();
        // Collect fetch outcomes to send to scheduler after we're done borrowing self
        let mut fetch_outcomes: Vec<external_fetch::FetchOutcome> = Vec::new();

        for result in self.hades.drain_results() {
            // ExternalFetch results bypass work_state — they're independent of the
            // Witch's task lifecycle. Process them separately.
            if result.kind == types::TaskKind::ExternalFetch {
                if let Some(fetch_data) = result.fetch_result {
                    fetch_outcomes.push(fetch_data);
                }
                continue;
            }

            // Task has completed - no longer in flight
            self.work_state.dec_in_flight();
            self.work_state.inc_processed();

            // Track by task type
            *self.task_counts.entry(result.label.clone()).or_insert(0) += 1;
            *self.kind_counts.entry(result.kind).or_insert(0) += 1;

            // Decrement pending count for this label
            self.work_state.dec_label(&result.label);

            if !result.success {
                if let Some(err) = result.error {
                    self.error_generation += 1;
                    if self.recent_errors.len() >= 5 {
                        self.recent_errors.pop_front();
                    }
                    self.recent_errors.push_back(err);
                }
            }

            // Apply config update if present (from ApplyConfigEdits mutation)
            if let Some(new_config) = result.config_update {
                // If in polling mode, send updated interval to the watcher thread
                if self.watcher_state == WatcherState::Polling {
                    let new_interval = new_config.opinions.watcher_poll_interval_secs;
                    let db_cache = self.build_watcher_db_cache();
                    self.fs_watcher.poll(db_cache, new_interval);
                }
                self.update_shared_config(new_config);
                self.config_generation += 1;
            }

            // Accumulate recomputation scope from mutation results
            if !result.recomputation_scope.is_empty() {
                self.session_recomputation_scope |= result.recomputation_scope;
            }

            // Collect spawned follow-up computations and mutations
            spawned_computations.extend(result.spawn);
            spawned_mutations.extend(result.spawn_mutations);

            // Collect deferred computation phases (pipeline orchestrators)
            if !result.deferred_phases.is_empty() {
                self.pending_computation_phases
                    .extend(result.deferred_phases);
            }
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

        // Send fetch outcomes to scheduler for chain-emit decisions
        if let Some(ref handle) = self.external_fetch {
            for outcome in fetch_outcomes {
                handle.send_outcome(outcome);
            }
        }

        // Cache thread handles its own periodic refreshes — no action needed here.

        // State machine transitions
        self.update_state();

        // FS watcher: drain initial scan results and steady-state events
        self.drain_watcher_messages();

        // If watcher events changed the observed inode maps, queue derivation
        // once current work drains. This batches rapid events naturally.
        if self.watcher_derivation_needed
            && self.work_state.is_idle()
            && self.reasoning_level == ReasoningLevel::Full
        {
            self.watcher_derivation_needed = false;
            crate::logging::log_general(
                "[WITCH] Steady-state derivation triggered by watcher events"
            );
            self.queue_awakening_computations(false);
        }

        // External fetch: drain scheduler messages (task requests + status)
        self.drain_scheduler_messages();

        // Startup state transitions (Reconciling → Vacuuming → Ready)
        match self.startup_state {
            types::WitchStartupState::Reconciling if !self.has_pending() => {
                crate::logging::log_general("[WITCH] Schema reconciliation complete");
                // Reconnect cache thread's DB so it picks up new schema
                self.cache_thread_handle.reconnect_db();
                self.startup_schema_descriptions.clear();
                if self.check_vacuum_needed() {
                    crate::logging::log_general("[WITCH] Vacuum threshold exceeded — entering Vacuuming");
                    self.startup_state = types::WitchStartupState::Vacuuming;
                    self.queue_vacuum();
                } else {
                    self.startup_state = types::WitchStartupState::Ready;
                }
            }
            types::WitchStartupState::Vacuuming if !self.has_pending() => {
                crate::logging::log_general("[WITCH] Vacuum complete — entering Ready");
                self.startup_state = types::WitchStartupState::Ready;
            }
            _ => {}
        }

        // Auto-start watcher when Ready and not yet scanning.
        // Scanning is the Witch's own operational concern — not gated on client login.
        if self.startup_state == types::WitchStartupState::Ready
            && self.watcher_state == WatcherState::NotStarted
        {
            self.start_watching();
        }

    }

    /// Update state machine based on in-flight tasks and timing.
    fn update_state(&mut self) {
        match self.work_state {
            WorkState::Idle => {
                // Idle → Working: handled in queue methods
            }
            WorkState::Working { processed, .. } => {
                // Phase advancement: if all in-flight tasks have landed and the
                // db_thread has drained, but pending pipeline phases are waiting,
                // pop and spawn the next phase.  This must happen *before* the
                // has_pending() gate because has_pending() includes these phases
                // in its check, which would otherwise deadlock: has_pending()
                // returns true → transition_to_completed() never called → phases
                // never drained.
                //
                // Mutation phases (from staged transactions) take priority over
                // computation phases (from pipeline orchestrators).
                if self.work_state.in_flight() == 0
                    && self.db_thread_handle.queue_empty()
                    && !self.pending_mutation_phases.is_empty()
                {
                    let (stage, mutations) = self.pending_mutation_phases.pop_front().unwrap();
                    crate::logging::log_mutation(format!(
                        "[TRANSACTION] Phase advancement: draining db_thread, then queueing {:?} ({} mutations). \
                         {} phase(s) remaining.",
                        stage, mutations.len(), self.pending_mutation_phases.len()
                    ));
                    write_thread::wait_for_queue_drain();

                    // Extract label from current WorkState (preserves session label)
                    let label = if let WorkState::Working { ref label, .. } = self.work_state {
                        label.clone()
                    } else {
                        None
                    };
                    self.queue_mutations_internal(mutations, label);
                    return; // Stay in Working — more work queued
                }

                if self.work_state.in_flight() == 0
                    && self.db_thread_handle.queue_empty()
                    && !self.pending_computation_phases.is_empty()
                {
                    let (stage, computations) =
                        self.pending_computation_phases.pop_front().unwrap();
                    crate::logging::log_general(format!(
                        "[PIPELINE] Phase advancement: draining db_thread, then queueing {} ({} computations). \
                         {} phase(s) remaining.",
                        stage.label(), computations.len(), self.pending_computation_phases.len()
                    ));
                    write_thread::wait_for_queue_drain();

                    for comp in computations {
                        self.queue_computation_with_label(comp, None);
                    }
                    return; // Stay in Working — more work queued
                }

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
        // NOTE: Both pending_mutation_phases and pending_computation_phases are
        // now drained in update_state() before the has_pending() gate.  By the
        // time we reach transition_to_completed(), they are guaranteed empty.

        // Mutations with non-empty scope need re-awakening. Mutations with EMPTY
        // scope (AcknowledgeMtimeOnly, operational config edits) don't — their
        // post-execution pipeline already handles everything they need.
        let had_mutations = !self.session_recomputation_scope.is_empty();

        let mut work = PostTransitionWork::default();

        // Extract session counters before transitioning
        let session_processed = match &self.work_state {
            WorkState::Working { processed, .. } => *processed,
            _ => 0,
        };

        // State transition based on reasoning_level.
        //
        // Observation-phase transitions (watcher scan completing) are now handled
        // directly in drain_watcher_messages() when AllInitialScansComplete arrives.
        // This function only sees non-observation work draining (derivation,
        // mutations, content analysis, maintenance).
        match self.reasoning_level {
            // Inodes completed: derivation + ReconcileLibraryFiles all drained.
            // Transition directly to Full.
            ReasoningLevel::Inodes => {
                crate::logging::log_general(format!(
                    "[STATE] Inodes complete. Transitioning Inodes -> Full. \
                     Processed {} tasks.",
                    session_processed
                ));
                self.reasoning_level = ReasoningLevel::Full;

                crate::logging::log_general(
                    "[STATE] Mutations now enabled.",
                );

                // Queue content analysis after full awakening
                work.content_analysis = true;
            }

            // Normal operation: work completed while Full
            ReasoningLevel::Full => {
                if had_mutations {
                    // Mutations ran — notify UI and invalidate caches.
                    // inotify detects FS changes from mutations → watcher_derivation_needed
                    // triggers derivation when idle. No watcher restart needed.
                    self.mutations_generation += 1;
                    self.cache_thread_handle.invalidate_scope(self.session_recomputation_scope);
                    crate::logging::log_general(format!(
                        "[STATE] Mutations complete (scope={:?}). Staying Full, inotify handles re-derivation. \
                         Processed {} tasks.",
                        self.session_recomputation_scope, session_processed
                    ));
                    // Carry the accumulated scope into the pending slot for
                    // ScheduleContentAnalysis to consume after re-derivation.
                    self.pending_recomputation_scope = Some(std::mem::replace(
                        &mut self.session_recomputation_scope,
                        crate::meta::recomputation::RecomputationScope::EMPTY,
                    ));
                    // Queue derivation immediately since mutations may have changed file state
                    self.watcher_derivation_needed = true;
                }
                // If no mutations, stay Full (normal work completion)
            }

            // Maintenance can complete while None - this is valid, just NOP
            ReasoningLevel::None => {
                let only_maintenance = self
                    .kind_counts
                    .keys()
                    .all(|k| *k == types::TaskKind::Maintenance);
                if only_maintenance {
                    crate::logging::log_general(format!(
                        "[STATE] Maintenance complete while None. Staying None. \
                         Processed {} tasks.",
                        session_processed
                    ));
                } else {
                    panic!(
                        "Invalid state: non-maintenance work completed while reasoning is None. \
                         The only work while None should be maintenance."
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
        self.kind_counts.clear();
        self.recent_errors.clear();
        self.session_recomputation_scope = crate::meta::recomputation::RecomputationScope::EMPTY;

        self.dispatch_post_transition_work(work);
    }

    /// Dispatch deferred work after a session transition to Done.
    ///
    /// Called at the end of `transition_to_completed()` after the work state
    /// has been reset. Flushes the db_thread write queue if any follow-up work
    /// is pending, then queues the appropriate computations.
    fn dispatch_post_transition_work(&mut self, work: PostTransitionWork) {
        if !work.has_work() {
            return;
        }

        // Flush all pending db_thread writes before queueing the next phase.
        // Computation tasks fire writes asynchronously via db_thread (fire-and-forget).
        // The task completes when the worker returns, NOT when db_thread commits the
        // writes. Without this barrier, the next phase's computations could read stale
        // data (e.g., DeriveDeployHealthSignals reading library files written by
        // ScanLibraryDirectory, or Awake-phase computations reading Awakening signals).
        write_thread::wait_for_queue_drain();

        // Queue follow-up computations AFTER reset to fix off-by-one counting
        // (if queued before reset, the task's queue count gets wiped but it still completes)
        if work.content_analysis {
            // Create witness here - this is the ONLY valid call site
            let witness = ContentAnalysisWitness::new();
            self.queue_content_analysis(witness);
        }
    }

    /// Queue Awakening-phase computations.
    ///
    /// Takes the accumulated observed inode maps and queues DeriveCorpusSignals
    /// and DeriveInboxSignals. When `include_second_level` is true, also queues
    /// ScheduleSecondLevelDerivations for directory checks and library walks.
    fn queue_awakening_computations(&mut self, include_second_level: bool) {
        // Flush pending image observations before derivation runs.
        // Images must be in the `files` table before DeriveCorpusSignals,
        // otherwise derivation sees them as disk-only ghosts.
        if !self.pending_observed_images.is_empty() {
            let images = std::mem::take(&mut self.pending_observed_images);
            let count = images.len();
            crate::logging::log_general(format!(
                "[WITCH] Queueing IndexObservedImages for {} images", count
            ));
            self.queue_computation_with_label(
                Computation::Analysis(
                    analysis::Computation::IndexObservedImages { images }
                ),
                Some(format!("Index {} observed images", count)),
            );
        }

        // Clone maps rather than take — the observed inode sets must persist for
        // steady-state watcher events to incrementally update them. Derivation
        // gets a snapshot; the Witch keeps the authoritative live set.
        let observed_corpus = self.observed_inodes.corpus.clone();
        let observed_inbox = self.observed_inodes.inbox.clone();

        let label = if include_second_level { "Awakening" } else { "Steady-state derivation" };
        crate::logging::log_general(format!(
            "[STATE] Queueing {}: DeriveCorpusSignals ({} inodes), DeriveInboxSignals ({} inodes){}",
            label, observed_corpus.len(), observed_inbox.len(),
            if include_second_level { ", ScheduleSecondLevelDerivations" } else { "" },
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

        if include_second_level {
            self.queue_computation_with_label(
                Computation::Derivation(derivation::Computation::ScheduleSecondLevelDerivations),
                Some("Computing directory signals".to_string()),
            );

            // Convert watcher library observations to ObservedLibraryFile format
            // and queue ReconcileLibraryFiles directly (no WalkLibrary/ScanLibraryDirectory needed).
            if !self.observed_inodes.library.is_empty() {
                let observed_files: Vec<derivation::ObservedLibraryFile> = self
                    .observed_inodes
                    .library
                    .iter()
                    .map(|(inode, meta)| derivation::ObservedLibraryFile {
                        stored_path: meta.path.clone(),
                        inode: *inode,
                        mtime_secs: meta.mtime_secs,
                        mtime_nanos: meta.mtime_nanos,
                        file_size: meta.file_size,
                    })
                    .collect();

                crate::logging::log_general(format!(
                    "[STATE] Queueing ReconcileLibraryFiles from watcher data ({} files)",
                    observed_files.len()
                ));
                self.queue_computation_with_label(
                    Computation::Derivation(derivation::Computation::ReconcileLibraryFiles {
                        observed_files,
                    }),
                    Some("Reconciling library files".to_string()),
                );
            }
        }
    }

    // =========================================================================
    // External Fetch Integration
    // =========================================================================

    /// Drain messages from the external fetch scheduler and act on them.
    ///
    /// Task requests are spawned on rayon. Status updates (progress, source
    /// completion) are tracked locally. Called each tick().
    fn drain_scheduler_messages(&mut self) {
        let messages = match self.external_fetch {
            Some(ref mut handle) => handle.drain_messages(),
            None => return,
        };

        for msg in messages {
            match msg {
                external_fetch::SchedulerMessage::TaskRequest { task, label } => {
                    // Spawn on rayon — bypasses work_state tracking entirely
                    let t = Task::ExternalFetch(task);
                    self.spawn_task(t, label);
                }
                external_fetch::SchedulerMessage::Progress(p) => {
                    self.fetch_progress = Some(p);
                }
                external_fetch::SchedulerMessage::SourceDone { source, stats } => {
                    crate::logging::log_general(format!(
                        "[FETCH] {} done: {} processed, {} matched, {} no-match, {} retries",
                        source.name(),
                        stats.processed,
                        stats.matched,
                        stats.no_match,
                        stats.retries
                    ));
                    if stats.matched > 0 {
                        self.session_recomputation_scope |=
                            crate::meta::recomputation::RecomputationScope::EXTERNAL;
                    }
                }
                external_fetch::SchedulerMessage::AllDone => {
                    // batch_active already cleared by drain_messages()
                }
            }
        }
    }

    // =========================================================================
    // FS Watcher Integration
    // =========================================================================

    /// Drain messages from the filesystem watcher and act on them.
    ///
    /// InitialScanComplete results are accumulated into observed inode maps.
    /// AllInitialScansComplete signals observation is done (equivalent to
    /// the old WalkCorpus + ScanCorpusDirectory flow).
    ///
    /// Called each tick().
    fn drain_watcher_messages(&mut self) {
        let messages = self.fs_watcher.drain_messages();

        for msg in messages {
            match msg {
                fs_watcher::WatcherMessage::InitialScanComplete { zone, inodes } => {
                    crate::logging::log_general(format!(
                        "[WITCH] Watcher initial scan complete for {}: {} inodes",
                        zone,
                        inodes.len()
                    ));

                    // Accumulate inodes into observed maps.
                    // Watcher paths are zone-relative.
                    // For corpus/inbox: prepend zone name to match DB convention
                    //   (e.g. "digital/releases/..." → "corpus/digital/releases/...")
                    // For library: paths are already in stored_path format
                    //   (e.g. "music/Artist/track.opus" = library_name/relative)
                    if let Some(map) = self.observed_inodes.for_zone_mut(zone) {
                        let use_zone_prefix = zone != crate::db::types::Zone::Library;
                        let zone_prefix = zone.as_str();
                        for (inode, observed) in inodes {
                            let path = if use_zone_prefix {
                                format!("{}/{}", zone_prefix, observed.path)
                            } else {
                                observed.path.clone()
                            };
                            map.insert(inode, ObservedInodeMeta {
                                path,
                                mtime_secs: observed.mtime_secs,
                                mtime_nanos: observed.mtime_nanos,
                                file_size: observed.file_size,
                            });
                        }
                    }
                }
                fs_watcher::WatcherMessage::AllInitialScansComplete => {
                    crate::logging::log_general(format!(
                        "[WITCH] All watcher initial scans complete. \
                         Corpus: {} inodes, Inbox: {} inodes, Library: {} inodes",
                        self.observed_inodes.corpus.len(),
                        self.observed_inodes.inbox.len(),
                        self.observed_inodes.library.len(),
                    ));
                    // Drive the state machine forward based on current reasoning level.
                    // Note: watcher_state transitions to Watching only when
                    // MonitoringActive arrives (after inotify watches are established).
                    // This replaces the observation→awakening transition that previously
                    // happened in transition_to_completed() when the old observation
                    // computations drained.
                    match self.reasoning_level {
                        ReasoningLevel::None => {
                            crate::logging::log_general(
                                "[STATE] Watcher scan complete. Transitioning None -> Inodes."
                            );
                            self.reasoning_level = ReasoningLevel::Inodes;
                            self.queue_awakening_computations(true);
                        }
                        ReasoningLevel::Inodes => {
                            crate::logging::log_general(
                                "[STATE] Re-observation complete during Inodes. Queueing derivations."
                            );
                            self.queue_awakening_computations(true);
                        }
                        ReasoningLevel::Full => {
                            crate::logging::log_general(
                                "[STATE] Re-observing complete while Full. Queueing awakening to sync signals."
                            );
                            self.queue_awakening_computations(true);
                        }
                    }
                }
                // Steady-state events: incremental inode map updates + computation queueing
                fs_watcher::WatcherMessage::FileChanged {
                    zone, inode, path,
                    mtime_secs, mtime_nanos, file_size, disk_tags,
                } => {
                    crate::logging::log_general(format!(
                        "[WITCH] Watcher: file changed — zone={} inode={} path={:?}",
                        zone, inode, path
                    ));

                    // Update observed map with current path and metadata
                    if let Some(map) = self.observed_inodes.for_zone_mut(zone) {
                        let rel_path = crate::corpus::paths::get_resolver()
                            .to_relative(&path)
                            .unwrap_or_else(|| path.clone())
                            .to_string_lossy()
                            .to_string();
                        map.insert(inode, ObservedInodeMeta {
                            path: rel_path,
                            mtime_secs,
                            mtime_nanos,
                            file_size,
                        });
                    }

                    // Queue per-inode tag verification (compares disk tags vs DB).
                    // Only corpus files have tag verification — inbox files are
                    // reconciled entirely through derivation.
                    if zone == crate::db::types::Zone::Corpus {
                        self.queue_computation_with_label(
                            Computation::Observation(
                                crate::meta::computations::observation::Computation::VerifyTags {
                                    inode, path,
                                    mtime_secs, mtime_nanos, file_size,
                                    disk_tags,
                                }
                            ),
                            Some("Verify tags (watcher)".to_string()),
                        );
                    }
                }
                fs_watcher::WatcherMessage::FileCreated {
                    zone, inode, path,
                    mtime_secs, mtime_nanos, file_size,
                } => {
                    crate::logging::log_general(format!(
                        "[WITCH] Watcher: file created — zone={} inode={} path={:?}",
                        zone, inode, path
                    ));

                    if let Some(map) = self.observed_inodes.for_zone_mut(zone) {
                        let rel_path = crate::corpus::paths::get_resolver()
                            .to_relative(&path)
                            .unwrap_or_else(|| path.clone())
                            .to_string_lossy()
                            .to_string();
                        map.insert(inode, ObservedInodeMeta {
                            path: rel_path,
                            mtime_secs,
                            mtime_nanos,
                            file_size,
                        });
                    }

                    // Derivation will detect this as disk-only and emit UnindexedFileSignal
                    self.watcher_derivation_needed = true;
                }
                fs_watcher::WatcherMessage::FileRemoved { zone, inode, path } => {
                    crate::logging::log_general(format!(
                        "[WITCH] Watcher: file removed — zone={} inode={} path={:?}",
                        zone, inode, path
                    ));

                    if let Some(map) = self.observed_inodes.for_zone_mut(zone) {
                        map.remove(&inode);
                    }

                    // Derivation will detect this as index-only and emit MissingFileSignal
                    // (or cascade-drop for inbox)
                    self.watcher_derivation_needed = true;
                }
                fs_watcher::WatcherMessage::ImageFileObserved(mut img) => {
                    // Watcher paths are zone-relative; DB expects archive-root-relative
                    img.path = format!("{}/{}", img.zone.as_str(), img.path);
                    self.pending_observed_images.push(img);
                }
                fs_watcher::WatcherMessage::MonitoringActive => {
                    if self.watcher_state != WatcherState::Polling {
                        self.watcher_state = WatcherState::Watching;
                    }
                    crate::logging::log_general(
                        "[WITCH] Watcher monitoring active (inotify established)"
                    );
                }
                fs_watcher::WatcherMessage::InotifyFailed => {
                    crate::logging::log_error(
                        "[WITCH] Watcher fell back to polling mode (inotify unavailable). \
                         Filesystem changes will be detected periodically, not in real-time."
                    );
                    self.observed_inodes.clear();
                    self.watcher_state = WatcherState::Polling;
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
            let dirs: Vec<std::path::PathBuf> = config
                .source_dirs
                .iter()
                .filter(|sd| sd.enable_acoustid.unwrap_or(true))
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

        handle.request_fetch(eligible_dirs);
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

    /// Queue release bin-packing analysis (operator-initiated).
    ///
    /// Analyzes cached MusicBrainz data and assigns corpus files to releases
    /// using greedy bin-packing. Does not require a ConfirmationGesture.
    pub fn request_release_packing(&mut self) {
        crate::logging::log_general("[WITCH] Release packing analysis requested");
        self.queue_computation_with_label(
            Computation::Analysis(analysis::Computation::PackReleases),
            Some("Release packing".to_string()),
        );
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
    fn spawn_task(&self, task: Task, label: String) {
        self.hades.dispatch(task, label);
    }

    // -------------------------------------------------------------------------
    // Enqueue Helper
    // -------------------------------------------------------------------------

    /// Resolve label, bump counters, and spawn a single task on the rayon pool.
    ///
    /// Consolidates the 4-step sequence (inc_queued / inc_in_flight / inc_label /
    /// spawn_task) used by every queue method.
    fn enqueue_one(&mut self, task: Task, label: Option<String>) {
        let task_label = self.resolve_label(label, &task);
        self.work_state.inc_queued(1);
        self.work_state.inc_in_flight();
        self.work_state.inc_label(&task_label);
        self.spawn_task(task, task_label);
    }

    // -------------------------------------------------------------------------
    // Config Access Helper
    // -------------------------------------------------------------------------

    /// Read from shared config, returning None if config isn't set yet.
    fn read_config<T>(&self, f: impl FnOnce(&Config) -> T) -> Option<T> {
        self.shared_config.as_ref().map(|sc| {
            let config = sc.read().expect("SharedConfig lock poisoned");
            f(&config)
        })
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

    pub(super) fn queue_mutations_internal(
        &mut self,
        mutations: impl IntoIterator<Item = Mutation>,
        label: Option<String>,
    ) {
        self.transition_to_working();

        let mutations: Vec<_> = mutations.into_iter().collect();

        crate::logging::log_general(format!(
            "[WORKER] queue_mutations_internal: queueing {} mutations (label={:?})",
            mutations.len(),
            label
        ));

        // Store label in WorkState for phase advancement
        if let WorkState::Working {
            label: ref mut ws_label,
            ..
        } = self.work_state
        {
            if ws_label.is_none() {
                *ws_label = label.clone();
            }
        }

        for mutation in mutations {
            self.enqueue_one(Task::Mutation(Box::new(mutation)), label.clone());
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
        //
        // Pass None for label so resolve_label falls through to TaskLabel::from_task
        self.enqueue_one(Task::Mutation(Box::new(mutation)), None);
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
        self.enqueue_one(Task::Computation(computation), label);
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
        self.enqueue_one(Task::Maintenance(task), None);
    }

    // -------------------------------------------------------------------------
    // Migration-Aware Startup Methods
    // -------------------------------------------------------------------------

    /// Check if schema reconciliation is needed.
    ///
    /// Returns true if the database exists and has pending schema changes.
    pub fn needs_schema_update(&self) -> bool {
        use crate::db::reconciler;

        if self.startup_state != types::WitchStartupState::Ready {
            return false;
        }

        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return false,
        };
        if !db_path.exists() {
            return false;
        }

        let db = match Database::open_read_only(&db_path) {
            Ok(d) => d,
            Err(_) => return false,
        };

        let schema_dirty = !reconciler::fingerprint_matches(&db);
        let has_data_migrations = reconciler::has_pending_data_migrations(&db);

        if !schema_dirty && !has_data_migrations {
            return false;
        }

        if schema_dirty {
            match reconciler::ReconciliationPlan::compute(db.conn()) {
                Ok(plan) => !plan.is_empty() || has_data_migrations,
                _ => true,
            }
        } else {
            true
        }
    }

    /// Get pending schema update descriptions for UI display.
    ///
    /// Returns a list of human-readable descriptions of planned changes.
    pub fn pending_schema_descriptions(&self) -> Vec<String> {
        use crate::db::reconciler::ReconciliationPlan;

        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        let db = match Database::open_read_only(&db_path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };

        match ReconciliationPlan::compute_full(&db) {
            Ok(plan) => plan.descriptions(),
            Err(_) => vec!["Schema update needed (could not compute details)".to_string()],
        }
    }

    /// Queue schema reconciliation for async execution.
    ///
    /// Called internally by the Witch during startup when schema update is needed.
    fn queue_schema_reconciliation(&mut self) {
        use crate::meta::maintenance::DbMaintenanceTask;

        crate::logging::log_general("[WITCH] Queueing schema reconciliation");
        self.queue_maintenance(DbMaintenanceTask::SchemaReconciliation);
    }

    /// Queue a VACUUM for async execution.
    ///
    /// Called internally by the Witch during startup when vacuum threshold is exceeded.
    fn queue_vacuum(&mut self) {
        use crate::meta::maintenance::DbMaintenanceTask;

        self.queue_maintenance(DbMaintenanceTask::Vacuum);
    }

    /// Check if the database needs vacuuming based on freelist ratio.
    ///
    /// Returns true if the freelist ratio exceeds `self.vacuum_threshold`.
    fn check_vacuum_needed(&self) -> bool {
        if self.vacuum_threshold <= 0.0 {
            return false;
        }

        let db_path = match config::get_db_path() {
            Ok(p) if p.exists() => p,
            _ => return false,
        };

        let conn = match rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) {
            Ok(c) => c,
            Err(_) => return false,
        };

        let page_count: u64 = conn
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .unwrap_or(0);
        let freelist_count: u64 = conn
            .pragma_query_value(None, "freelist_count", |row| row.get(0))
            .unwrap_or(0);

        if page_count == 0 {
            return false;
        }

        let ratio = freelist_count as f64 / page_count as f64;
        ratio > self.vacuum_threshold
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
        }
    }

    /// Build the comprehensive state machine snapshot.
    ///
    /// This captures all observable Witch state in a single struct.
    /// Returned synchronously when a client sends `StatusQuery`.
    pub fn publish_status(&self) -> WitchStatus {
        let transaction = self.pending_transaction.as_ref().map(|txn| {
            TransactionSnapshot {
                label: txn.label.clone(),
                decision_count: txn.decision_count(),
                mutation_count: txn.mutation_count(),
                decision_keys: txn.keys(),
                decision_labels: txn
                    .decisions
                    .iter()
                    .map(|(k, d)| (k.clone(), d.label.clone()))
                    .collect(),
            }
        });

        WitchStatus {
            startup_state: self.startup_state,
            work: self.status(),
            reasoning_level: self.reasoning_level(),
            has_pending: self.has_pending(),
            is_initial_scanning: self.is_initial_scanning(),
            db_queue_depth: self.db_queue_depth(),
            transaction,
            handled_decision_kinds: self.handled_sources.clone(),
            is_external_fetch_active: self.is_external_fetch_active(),
            external_fetch_progress: self.external_fetch_progress().cloned(),
            has_acoustid_api_key: self.has_acoustid_api_key(),
            mutations_generation: self.mutations_generation,
            last_error: self.recent_errors.back().cloned(),
            error_generation: self.error_generation,
            config_generation: self.config_generation,
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
        // Check pending mutation/computation pipeline phases
        if !self.pending_mutation_phases.is_empty() || !self.pending_computation_phases.is_empty() {
            return true;
        }
        false
    }

    /// Get pending DB write queue depth.
    pub fn db_queue_depth(&self) -> u64 {
        self.db_thread_handle.queue_depth()
    }

    /// Rebuild handled_sources from current transaction state.
    ///
    /// Called internally after transaction mutations (add, remove, confirm, discard).
    pub(crate) fn sync_handled_sources(&mut self) {
        self.handled_sources = self
            .pending_transaction
            .as_ref()
            .map(|txn| txn.decisions.keys().filter_map(|k| k.kind()).collect())
            .unwrap_or_default();
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
