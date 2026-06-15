//! Handlers for offloaded spawn_blocking results.
//!
//! Every blocking operation that was removed from the Witch's `select!` loop
//! sends its result back through the unified offload channel. This module
//! dispatches those results and applies the appropriate state transitions.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use super::types::{AutoIndexSource, ContentAnalysisWitness, OffloadResult, PendingWatcherCommand};

impl super::Witch {
    /// Dispatch a single offloaded result from a spawn_blocking task.
    pub(super) fn handle_offload_result(&mut self, result: OffloadResult) {
        match result {
            OffloadResult::WatcherDbCache { cache } => {
                self.watcher_cache_loading = false;
                if let Some(cmd) = self.pending_watcher_command.take() {
                    match cmd {
                        PendingWatcherCommand::Start { zones } => {
                            self.fs_watcher.start(zones, cache);
                            crate::logging::log_general(
                                "[WITCH] Watcher started — initial scan in progress",
                            );
                        }
                        PendingWatcherCommand::Poll { interval_secs } => {
                            self.fs_watcher.poll(cache, interval_secs);
                            crate::logging::log_general(
                                "[WITCH] Watcher poll triggered — re-walking zones",
                            );
                        }
                    }
                }
            }

            OffloadResult::AutoIndexResult {
                mutations,
                stale_inodes,
                source,
            } => {
                // Clear signals for files whose paths no longer exist on disk.
                // Without this, moved/deleted files spin the auto-indexer forever
                // (mutation fails with ENOENT → recomputation_scope is EMPTY →
                // had_mutations=false → immediate re-check → same failures → loop).
                if !stale_inodes.is_empty() {
                    if let Some(sender) = crate::db::write_thread::signal_sender() {
                        use crate::meta::signals::data::UnindexedFileSignal;
                        // Witness: infrastructure cleanup within crate::witch,
                        // same authorization scope as execute_mutation().
                        let witness = super::types::MutationExecutionWitness::new();
                        crate::logging::log_general(format!(
                            "[AUTO-INDEX] Clearing {} stale unindexed signals (files no longer at recorded path)",
                            stale_inodes.len()
                        ));
                        for inode in &stale_inodes {
                            sender.clear_corpus_signal::<UnindexedFileSignal>(
                                *inode,
                                &witness,
                            );
                        }
                    }
                }

                if !mutations.is_empty() {
                    crate::logging::log_general(format!(
                        "[AUTO-INDEX] Queueing {} index mutations for unindexed corpus files",
                        mutations.len()
                    ));
                    self.queue_mutations_internal(
                        mutations,
                        Some("Auto-index unindexed files".to_string()),
                    );
                    self.pending_work.insert(super::types::PendingWork::FETCH);

                    // At Inodes→Full, the post-mutation content analysis pass
                    // owes the deploy/release pipeline a recompute even if
                    // fetch produces no EXTERNAL scope. Branch A (no mutations)
                    // already fires it directly; mirror that here so dirty
                    // corpus_deploy_status inodes carried across a restart
                    // get reclassified without waiting on the linger gate.
                    if matches!(source, AutoIndexSource::InodesTransition) {
                        self.pending_startup_content_analysis = true;
                    }
                } else if matches!(source, AutoIndexSource::InodesTransition) {
                    // No unindexed files at Inodes→Full — go straight to content analysis.
                    let witness = ContentAnalysisWitness::new();
                    self.queue_content_analysis(witness);
                }
            }

            OffloadResult::AutoDeployResult { soft_mutations } => {
                if !soft_mutations.is_empty() {
                    crate::logging::log_general(format!(
                        "[AUTO-DEPLOY] Queueing {} soft mutations for library deployment",
                        soft_mutations.len()
                    ));
                    self.queue_soft_mutations_internal(
                        soft_mutations,
                        Some("Auto-deploy".to_string()),
                    );
                }
            }

            OffloadResult::SidecarDeployResult { soft_mutations } => {
                if !soft_mutations.is_empty() {
                    crate::logging::log_general(format!(
                        "[SIDECAR-DEPLOY] Queueing {} sidecar deploy links",
                        soft_mutations.len()
                    ));
                    self.queue_soft_mutations_internal(
                        soft_mutations,
                        Some("Sidecar deploy".to_string()),
                    );
                }
            }

            OffloadResult::VacuumCheck { needed } => {
                self.vacuum_check_pending = false;
                if needed {
                    crate::logging::log_general(
                        "[WITCH] Vacuum threshold exceeded — entering Vacuuming",
                    );
                    self.startup_state = super::types::WitchStartupState::Vacuuming;
                    self.queue_vacuum();
                } else {
                    self.startup_state = super::types::WitchStartupState::Ready;
                }
            }

            OffloadResult::SetupComplete { result, reply } => {
                self.setup_in_progress = false;
                match result {
                    Ok(output) => {
                        use crate::config;

                        config::init_performance_config(
                            output.config.opinions.performance.clone(),
                        );

                        self.force_check_all_files_at_startup = output.force_check;
                        self.vacuum_threshold = output.vacuum_threshold;
                        self.set_shared_config(output.config.into_shared());

                        // Tell cache thread to reconnect to the new DB
                        self.cache_thread_handle.reconnect_db();

                        // Notify auth thread that DB is now available
                        if let Some(ref auth_handle) = self.auth_thread_handle {
                            auth_handle.notify_db_ready();
                        }

                        self.startup_state = super::types::WitchStartupState::Ready;
                        crate::logging::log_general(
                            "[WITCH] Setup complete — transitioning to Ready",
                        );

                        let _ = reply.send(Ok(
                            crate::meta::protocol::UnauthenticatedResponse::SetupComplete,
                        ));
                    }
                    Err(e) => {
                        crate::logging::log_error(format!(
                            "[WITCH] First-time setup failed: {}",
                            e
                        ));
                        let _ = reply.send(Err(
                            crate::meta::protocol::ProtocolError::Internal(e),
                        ));
                    }
                }
            }
        }
    }
}
