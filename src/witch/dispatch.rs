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
                // ConfigKdl queries bypass synchronous dispatch — the file read
                // is offloaded to a blocking task that replies directly.
                if let AuthenticatedBody::Query(QueryPayload::ConfigKdl) = &body {
                    match self.resolve_auth(Some(&token)) {
                        Ok(AuthorizationLevel::Authenticated) => {
                            tokio::task::spawn_blocking(move || {
                                let kdl = mm_utils::get_config_dir()
                                    .ok()
                                    .map(|d| d.join("config.kdl"))
                                    .and_then(|p| std::fs::read_to_string(p).ok())
                                    .unwrap_or_default();
                                let response = Ok(AuthenticatedResponse::Query(Box::new(
                                    QueryResponse::ConfigKdl(kdl),
                                )));
                                let _ = reply.send(response);
                            });
                        }
                        Ok(_) | Err(_) => {
                            let _ = reply.send(Err(ProtocolError::Unauthorized));
                        }
                    }
                    return false;
                }

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
                                    unreachable!("ConfigKdl queries handled above")
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
                                TransactionPayload::BatchApproveReleases { release_ids } => {
                                    match w.batch_approve_releases(release_ids) {
                                        Ok(summary) => {
                                            TransactionResponse::BatchApprovalStaged(summary)
                                        }
                                        Err(e) => TransactionResponse::Error(e),
                                    }
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
                        if self.startup_state != super::types::WitchStartupState::AwaitingSetup
                            || self.setup_in_progress
                        {
                            let _ = reply.send(Err(ProtocolError::Unauthorized));
                            return false;
                        }
                        // Offload the heavy setup work (FS, DB, bcrypt) to a blocking task.
                        // The reply channel is forwarded — the offload handler sends the response.
                        self.setup_in_progress = true;
                        let tx = self.offload_tx.clone();
                        tokio::task::spawn_blocking(move || {
                            let result = complete_setup_blocking(root, first_user);
                            let _ = tx.send(super::types::OffloadResult::SetupComplete {
                                result,
                                reply,
                            });
                        });
                        return false;
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

}

// ============================================================================
// Free Functions (blocking, run off main thread)
// ============================================================================

/// Complete first-time setup: write config, create dirs + DB, create first user.
///
/// Pure blocking work — no Witch state access. Returns a `SetupOutput` with
/// the data the Witch needs to update its own state.
fn complete_setup_blocking(
    root: std::path::PathBuf,
    first_user: Option<(String, String)>,
) -> Result<super::types::SetupOutput, String> {
    use mm_meta::auth::FirstTimeSetupToken;

    if !config::config_exists() {
        // Try to persist config to disk; tolerate failure (e.g. read-only FS)
        // because load_config() can fall back to env-var-only defaults.
        if let Err(e) = config::write_initial_config(&root) {
            crate::logging::log_general(format!(
                "[WITCH] Could not write config.kdl ({}), proceeding with env-only config", e
            ));
        }
    }

    let cfg = config::load_config().map_err(|e| format!("Failed to load config: {}", e))?;

    std::fs::create_dir_all(&cfg.storage_root)
        .map_err(|e| format!("Failed to create storage-root {}: {}", cfg.storage_root.display(), e))?;
    std::fs::create_dir_all(cfg.libraries_dir())
        .map_err(|e| format!("Failed to create libraries dir: {}", e))?;
    std::fs::create_dir_all(cfg.stash_dir())
        .map_err(|e| format!("Failed to create stash dir: {}", e))?;

    cfg.validate_path_nesting()
        .map_err(|e| format!("Path nesting validation failed: {}", e))?;
    crate::corpus::paths::validate_same_filesystem(&cfg)
        .map_err(|e| format!("Filesystem validation failed: {}", e))?;

    let db_path = config::get_db_path().map_err(|e| format!("Failed to get DB path: {}", e))?;
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create DB parent dir: {}", e))?;
    }
    let token = FirstTimeSetupToken::new();
    let db = crate::db::create_database(&db_path, &token)
        .map_err(|e| format!("Failed to create database: {}", e))?;

    if let Some((username, plaintext_password)) = first_user {
        let password_hash = crate::auth::hash_password(&plaintext_password)
            .map_err(|e| format!("Failed to hash password: {}", e))?;
        db.create_user(&username, &password_hash)
            .map_err(|e| format!("Failed to create first user: {}", e))?;
        crate::logging::log_general(format!("[WITCH] First user '{}' created", username));
    }

    drop(db);

    let force_check = cfg.opinions.startup.force_check_all_files_at_startup;
    let vacuum_threshold = cfg.opinions.startup.vacuum_threshold;

    Ok(super::types::SetupOutput {
        config: cfg,
        force_check,
        vacuum_threshold,
    })
}
