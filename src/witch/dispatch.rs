//! Protocol dispatch for the Witch.
//!
//! Routes incoming protocol messages (authenticated, unauthenticated) to the
//! appropriate Witch methods. Handles authentication gating, protocol-level
//! command routing, and first-time setup flow.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use crate::config;
use crate::meta::protocol::{
    AuthResponse, AuthenticatedBody, AuthenticatedResponse, AuthorizationLevel,
    CommandPayload, CommandResponse, DecisionDetail, ProtocolError, QueryPayload,
    QueryResponse, TransactionPayload, TransactionResponse, UnauthenticatedBody,
    UnauthenticatedResponse,
};

use super::handle::HandleCommand;

impl super::Witch {
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
    ) -> Result<AuthorizationLevel, ProtocolError> {
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
        required: AuthorizationLevel,
        handler: impl FnOnce(&mut Self) -> Result<T, ProtocolError>,
    ) -> Result<T, ProtocolError> {
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
    pub(super) fn dispatch_command(&mut self, cmd: HandleCommand) -> bool {
        match cmd {
            HandleCommand::Authenticated { token, body, reply } => {
                let body = *body;
                crate::logging::log_general(format!(
                    "[DISPATCH] Authenticated request received, body variant: {}",
                    match &body {
                        AuthenticatedBody::Query(_) => "Query",
                        AuthenticatedBody::Transaction(_) => "Transaction",
                        AuthenticatedBody::Command(_) => "Command",
                    }
                ));
                // Domain queries bypass synchronous dispatch — they forward
                // the reply channel to the read thread so it replies directly.
                if let AuthenticatedBody::Query(QueryPayload::Domain(domain_payload)) = body {
                    let domain_payload = *domain_payload;
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
                                    QueryResponse::Config(Box::new(config))
                                }
                                QueryPayload::ConfigKdl => {
                                    let config_dir = mm_utils::get_config_dir()
                                        .map_err(|e| ProtocolError::Internal(e.to_string()))?;
                                    let kdl = std::fs::read_to_string(config_dir.join("config.kdl"))
                                        .unwrap_or_default();
                                    QueryResponse::ConfigKdl(kdl)
                                }
                                QueryPayload::GetDirConfig(path) => {
                                    let sd = w.read_config(|c| c.get_raw_source_dir(&path).cloned())
                                        .ok_or(ProtocolError::NotReady)?;
                                    QueryResponse::DirConfig(sd)
                                }
                                QueryPayload::Domain(_) => {
                                    unreachable!("domain queries handled above")
                                }
                            };
                            Ok(AuthenticatedResponse::Query(Box::new(response)))
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
                            use crate::meta::protocol::BackgroundTask;
                            crate::logging::log_general(format!(
                                "[COMMAND] Received command: {:?}", *payload
                            ));
                            let response = match *payload {
                                CommandPayload::QueueTask(task) => {
                                    crate::logging::log_general(format!(
                                        "[COMMAND] QueueTask variant: {:?}, work_state before: {:?}",
                                        task, w.work_state
                                    ));
                                    match task {
                                        BackgroundTask::ExternalFetch => {
                                            match w.request_external_fetch() {
                                                Ok(()) => CommandResponse::Ok,
                                                Err(reason) => CommandResponse::Failed(reason),
                                            }
                                        }
                                        BackgroundTask::CoverArtFetch => {
                                            match w.request_cover_art_fetch() {
                                                Ok(()) => CommandResponse::Ok,
                                                Err(reason) => CommandResponse::Failed(reason),
                                            }
                                        }
                                        BackgroundTask::ReleasePacking => {
                                            w.request_release_packing(true);
                                            crate::logging::log_general(format!(
                                                "[COMMAND] After request_release_packing (incremental), work_state: {:?}",
                                                w.work_state
                                            ));
                                            CommandResponse::Ok
                                        }
                                        BackgroundTask::ReleasePackingFull => {
                                            w.request_release_packing(false);
                                            crate::logging::log_general(format!(
                                                "[COMMAND] After request_release_packing (full), work_state: {:?}",
                                                w.work_state
                                            ));
                                            CommandResponse::Ok
                                        }
                                        BackgroundTask::SchemaReconciliation => {
                                            w.queue_schema_reconciliation();
                                            CommandResponse::Ok
                                        }
                                        BackgroundTask::Vacuum => {
                                            w.queue_vacuum();
                                            CommandResponse::Ok
                                        }
                                    }
                                }
                                CommandPayload::Shutdown => {
                                    CommandResponse::Goodbye
                                }
                            };
                            Ok(AuthenticatedResponse::Command(response))
                        }
                    }
                });
                crate::logging::log_general(format!(
                    "[DISPATCH] Gate result: {}",
                    match &result {
                        Ok(_) => "Ok".to_string(),
                        Err(e) => format!("Err({:?})", e),
                    }
                ));
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
                        match &self.auth_handle {
                            Some(auth) => {
                                // Forward the reply channel to the auth thread —
                                // it does the work and responds directly.
                                auth.forward_login(
                                    username,
                                    password,
                                    crate::auth::SessionLifetime::CloseOnExit,
                                    reply,
                                );
                                return false;
                            }
                            None => {
                                Ok(UnauthenticatedResponse::Auth(
                                    AuthResponse::Failed("Auth not available".to_string()),
                                ))
                            }
                        }
                    }
                    UnauthenticatedBody::SetupQuery => {
                        let needs_setup = self.startup_state == super::types::WitchStartupState::AwaitingSetup;
                        let suggested_root = std::env::var("MM_ROOT").ok().map(std::path::PathBuf::from);
                        Ok(UnauthenticatedResponse::SetupStatus { needs_setup, suggested_root })
                    }
                    UnauthenticatedBody::CompleteSetup { root, first_user } => {
                        // Gate on startup state, not auth level — the auth
                        // thread may already exist (prior DB with users) while
                        // the Witch still needs an archive root.
                        if self.startup_state != super::types::WitchStartupState::AwaitingSetup {
                            Err(ProtocolError::Unauthorized)
                        } else {
                            match self.complete_setup_impl(root, first_user) {
                                Ok(()) => Ok(UnauthenticatedResponse::SetupComplete),
                                Err(e) => Err(ProtocolError::Internal(e)),
                            }
                        }
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

        // Create zone directories (storage_root IS corpus, no subdirectory needed)
        std::fs::create_dir_all(&cfg.storage_root)
            .map_err(|e| format!("Failed to create storage-root {}: {}", cfg.storage_root.display(), e))?;
        std::fs::create_dir_all(cfg.libraries_dir())
            .map_err(|e| format!("Failed to create libraries dir: {}", e))?;
        std::fs::create_dir_all(cfg.stash_dir())
            .map_err(|e| format!("Failed to create stash dir: {}", e))?;

        // Validate path root nesting invariants (no I/O needed)
        cfg.validate_path_nesting()
            .map_err(|e| format!("Path nesting validation failed: {}", e))?;

        // Validate all roots are on the same filesystem (hard links require it)
        crate::corpus::paths::validate_same_filesystem(&cfg)
            .map_err(|e| format!("Filesystem validation failed: {}", e))?;

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
        self.startup_state = super::types::WitchStartupState::Ready;
        crate::logging::log_general("[WITCH] Setup complete — transitioning to Ready");

        Ok(())
    }
}
