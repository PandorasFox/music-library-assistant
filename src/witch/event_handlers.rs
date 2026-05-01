//! Event processing for the Witch run loop.
//!
//! Handles incoming messages from Hades (task results), the filesystem watcher,
//! and the external fetch scheduler. Each handler updates Witch state and
//! queues follow-up work as needed.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use crate::meta::computations::Computation;

use super::external_fetch;
use super::fs_thread;
use super::pipeline_triggers;
use super::types::{ObservedInodeMeta, ReasoningLevel, WatcherState};

impl super::Witch {
    /// Process a single completed task result from Hades.
    pub(super) fn process_task_result(&mut self, result: super::types::TaskResult) {
        self.work_state.dec_in_flight();
        self.work_state.inc_processed();

        *self.task_counts.entry(result.label.clone()).or_insert(0) += 1;
        *self.kind_counts.entry(result.kind).or_insert(0) += 1;
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

        if let Some(new_config) = result.config_update {
            if self.watcher_state == WatcherState::Polling {
                let new_interval = new_config.opinions.watcher_poll_interval_secs;
                self.request_watcher_poll_with_cache(new_interval);
            }
            self.update_performance_impl(new_config.opinions.performance.clone());
            self.update_shared_config(new_config);
            self.config_generation += 1;
        }

        if !result.recomputation_scope.is_empty() {
            self.session_recomputation_scope |= result.recomputation_scope;
        }

        for comp in result.spawn {
            self.queue_computation_with_label(comp, None);
        }
        for mutation in result.spawn_mutations {
            self.queue_spawned_mutation(mutation);
        }
        if !result.deferred_phases.is_empty() {
            self.pending_computation_phases
                .extend(result.deferred_phases);
        }
        if !result.fetch_requests.is_empty() {
            self.handle_fetch_requests(result.fetch_requests);
        }
    }

    /// Post-drain bookkeeping: state transitions after processing task results.
    pub(super) fn post_drain_bookkeeping(&mut self) {
        self.update_state();
        self.check_startup_transitions();
        self.maybe_trigger_derivation();
    }

    /// Process a single watcher message.
    pub(super) fn process_watcher_message(&mut self, msg: fs_thread::WatcherMessage) {
        match msg {
            fs_thread::WatcherMessage::InitialScanComplete { zone, inodes } => {
                crate::logging::log_general(format!(
                    "[WITCH] Watcher initial scan complete for {}: {} inodes",
                    zone, inodes.len()
                ));
                if let Some(map) = self.observed_inodes.for_zone_mut(zone) {
                    for (inode, observed) in inodes {
                        map.insert(inode, ObservedInodeMeta {
                            path: observed.path,
                            mtime_secs: observed.mtime_secs,
                            mtime_nanos: observed.mtime_nanos,
                            file_size: observed.file_size,
                        });
                    }
                }
            }
            fs_thread::WatcherMessage::AllInitialScansComplete => {
                crate::logging::log_general(format!(
                    "[WITCH] All watcher initial scans complete. \
                     Corpus: {} inodes, Library: {} inodes",
                    self.observed_inodes.corpus.len(),
                    self.observed_inodes.library.len(),
                ));
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

                // Recover any pending_write markers + silent-stuck
                // needs_tag_flush=1 inodes that the watcher's mtime diff would
                // have skipped. These are inodes MM wrote tags to where the
                // post-write FileChanged event never reached VerifyTags
                // (container restart, mtime equality, etc.).
                self.queue_stale_flush_recovery();
            }
            fs_thread::WatcherMessage::FileChanged {
                zone, inode, path,
                mtime_secs, mtime_nanos, file_size, disk_tags,
            } => {
                crate::logging::log_general(format!(
                    "[WITCH] Watcher: file changed — zone={} inode={} path={:?}",
                    zone, inode, path
                ));
                let rel_path = crate::corpus::paths::get_resolver()
                    .to_zone_relative(&path, zone)
                    .unwrap_or_else(|| path.clone())
                    .to_string_lossy()
                    .to_string();
                if let Some(map) = self.observed_inodes.for_zone_mut(zone) {
                    map.insert(inode, ObservedInodeMeta {
                        path: rel_path.clone(),
                        mtime_secs, mtime_nanos, file_size,
                    });
                }
                if zone == crate::db::types::Zone::Corpus
                    && !crate::meta::computations::helpers::is_image_file(&path)
                {
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
                } else if zone == crate::db::types::Zone::Library {
                    self.queue_computation_with_label(
                        Computation::Derivation(
                            crate::meta::computations::derivation::Computation::WatcherUpsertLibraryFile {
                                file: crate::meta::computations::derivation::ObservedLibraryFile {
                                    stored_path: rel_path,
                                    inode,
                                    mtime_secs, mtime_nanos, file_size,
                                },
                            }
                        ),
                        Some("Library upsert (watcher)".to_string()),
                    );
                }
            }
            fs_thread::WatcherMessage::FileCreated {
                zone, inode, path,
                mtime_secs, mtime_nanos, file_size,
            } => {
                crate::logging::log_general(format!(
                    "[WITCH] Watcher: file created — zone={} inode={} path={:?}",
                    zone, inode, path
                ));
                let rel_path = crate::corpus::paths::get_resolver()
                    .to_zone_relative(&path, zone)
                    .unwrap_or_else(|| path.clone())
                    .to_string_lossy()
                    .to_string();
                if let Some(map) = self.observed_inodes.for_zone_mut(zone) {
                    map.insert(inode, ObservedInodeMeta {
                        path: rel_path.clone(),
                        mtime_secs, mtime_nanos, file_size,
                    });
                }
                if zone == crate::db::types::Zone::Library {
                    // Drive the DB row creation directly from the watcher event
                    // so steady-state library state stays in sync without
                    // waiting for the next ReconcileLibraryFiles tick.
                    self.queue_computation_with_label(
                        Computation::Derivation(
                            crate::meta::computations::derivation::Computation::WatcherUpsertLibraryFile {
                                file: crate::meta::computations::derivation::ObservedLibraryFile {
                                    stored_path: rel_path,
                                    inode,
                                    mtime_secs, mtime_nanos, file_size,
                                },
                            }
                        ),
                        Some("Library upsert (watcher)".to_string()),
                    );
                }
                self.watcher_derivation_needed = true;
            }
            fs_thread::WatcherMessage::FileRemoved { zone, inode, path } => {
                crate::logging::log_general(format!(
                    "[WITCH] Watcher: file removed — zone={} inode={} path={:?}",
                    zone, inode, path
                ));
                let rel_path = crate::corpus::paths::get_resolver()
                    .to_zone_relative(&path, zone)
                    .unwrap_or_else(|| path.clone())
                    .to_string_lossy()
                    .to_string();
                if let Some(map) = self.observed_inodes.for_zone_mut(zone) {
                    map.remove(&inode);
                }
                if zone == crate::db::types::Zone::Library {
                    self.queue_computation_with_label(
                        Computation::Derivation(
                            crate::meta::computations::derivation::Computation::WatcherDeleteLibraryFile {
                                stored_path: rel_path,
                            }
                        ),
                        Some("Library delete (watcher)".to_string()),
                    );
                }
                self.watcher_derivation_needed = true;
            }
            fs_thread::WatcherMessage::ImageFileObserved(img) => {
                // img.path is already zone-relative from the fs_thread
                self.pending_observed_images.push(img);
            }
            fs_thread::WatcherMessage::MonitoringActive => {
                if self.watcher_state != WatcherState::Polling {
                    self.watcher_state = WatcherState::Watching;
                }
                crate::logging::log_general(
                    "[WITCH] Watcher monitoring active (inotify established)"
                );
            }
            fs_thread::WatcherMessage::InotifyFailed => {
                crate::logging::log_error(
                    "[WITCH] Watcher fell back to polling mode (inotify unavailable). \
                     Filesystem changes will be detected periodically, not in real-time."
                );
                self.observed_inodes.clear();
                self.watcher_state = WatcherState::Polling;
            }
        }
    }

    /// Queue a `VerifyPendingWrite` Observation computation for every corpus
    /// inode that's flagged as needing reconciliation but is invisible to the
    /// watcher's mtime-diff path. Two populations:
    ///
    /// - `pending_write` dirty markers: MM wrote tags, the post-write
    ///   FileChanged event never reached VerifyTags (container restart,
    ///   mtime equality, etc.).
    /// - `audio_info.needs_tag_flush=1` with no other surface: ApplyTagOps
    ///   raised the flag and the disk-flush failed silently or never ran.
    ///   No consumer was clearing this until VerifyTags learned to.
    ///
    /// Runs once per `AllInitialScansComplete` and on each awakening
    /// quiescence transition.
    pub(super) fn queue_stale_flush_recovery(&mut self) {
        let db_path = match crate::config::get_db_path() {
            Ok(p) => p,
            Err(_) => return,
        };
        let db = match crate::db::Database::open_read_only(&db_path) {
            Ok(d) => d,
            Err(_) => return,
        };
        let read_db = crate::db::queries::ReadOnlyDb::new(&db);
        let pending_write_inodes: Vec<i64> = read_db
            .get_dirty_inodes("pending_write")
            .unwrap_or_default();
        let needs_flush_inodes: Vec<i64> = read_db
            .get_corpus_inodes_needing_tag_flush()
            .unwrap_or_default();
        let pw_count = pending_write_inodes.len();
        let nf_count = needs_flush_inodes.len();
        if pw_count == 0 && nf_count == 0 {
            return;
        }

        let mut union: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
        union.extend(pending_write_inodes);
        union.extend(needs_flush_inodes);

        let resolver = crate::corpus::paths::get_resolver();
        let mut queued = 0usize;
        let mut skipped_no_path = 0usize;
        for inode in &union {
            let rel_path = match self.observed_inodes.corpus.get(inode) {
                Some(meta) => meta.path.clone(),
                None => {
                    skipped_no_path += 1;
                    continue;
                }
            };
            let abs_path = resolver.resolve_for_zone(
                crate::db::types::Zone::Corpus,
                std::path::Path::new(&rel_path),
            );
            self.queue_computation_with_label(
                Computation::Observation(
                    crate::meta::computations::observation::Computation::VerifyPendingWrite {
                        inode: *inode,
                        path: abs_path,
                    },
                ),
                Some("Stale-flush recovery".to_string()),
            );
            queued += 1;
        }
        crate::logging::log_general(format!(
            "[WITCH] Queued {} VerifyPendingWrite stale-flush recovery computation(s) \
             ({} pending_write, {} needs_tag_flush=1, {} unioned; \
              {} skipped — inode not in corpus observed set)",
            queued, pw_count, nf_count, union.len(), skipped_no_path,
        ));
    }

    /// Process a single scheduler message from the external fetch thread.
    pub(super) fn process_scheduler_message(&mut self, msg: external_fetch::SchedulerMessage) {
        match msg {
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
                if let Some(ref mut handle) = self.external_fetch {
                    handle.mark_batch_done();
                }
                // Drain post-fetch computations (queued by pinned release resolution etc.)
                if !self.post_fetch_computations.is_empty() {
                    let comps = std::mem::take(&mut self.post_fetch_computations);
                    crate::logging::log_general(format!(
                        "[WITCH] Fetch done, queuing {} post-fetch computations",
                        comps.len()
                    ));
                    for comp in comps {
                        self.queue_computation_with_label(comp, None);
                    }
                }

                // Check if external data arrived and decide what to do.
                let post_fetch = pipeline_triggers::decide_post_fetch_actions(
                    self.session_recomputation_scope,
                );

                if let Some(scope) = post_fetch.scope_for_content_analysis {
                    // Consume the scope so it doesn't double-fire through
                    // transition_to_completed's had_mutations check.
                    self.session_recomputation_scope =
                        crate::meta::recomputation::RecomputationScope::EMPTY;

                    crate::logging::log_general(
                        "[WITCH] External data arrived — queuing post-fetch content analysis",
                    );
                    self.queue_computation_with_label(
                        Computation::Analysis(
                            crate::meta::computations::analysis::Computation::ScheduleContentAnalysis {
                                scope: Some(scope),
                            },
                        ),
                        Some("Post-fetch content analysis".to_string()),
                    );
                }
                if post_fetch.set_packing_needed {
                    self.pending_work.insert(super::types::PendingWork::PACKING);
                }
            }
            external_fetch::SchedulerMessage::CoverArtProgress(progress) => {
                self.cover_art_progress = Some(progress);
            }
            external_fetch::SchedulerMessage::CoverArtDone(progress) => {
                crate::logging::log_general(format!(
                    "[WITCH] Cover art fetch done: {} releases, {} written, {} skipped, {} upgraded",
                    progress.processed, progress.images_written, progress.images_skipped, progress.images_upgraded,
                ));
                self.cover_art_progress = Some(progress);
                if let Some(ref mut handle) = self.external_fetch {
                    handle.mark_cover_art_done();
                }
            }
            external_fetch::SchedulerMessage::SidecarReplacements(replacements) => {
                crate::logging::log_general(format!(
                    "[WITCH] Queuing stash-and-replace for {} sidecar(s)",
                    replacements.len(),
                ));
                self.queue_computation_with_label(
                    Computation::Derivation(
                        crate::meta::computations::derivation::Computation::StashAndReplaceSidecars {
                            replacements,
                        },
                    ),
                    Some("Stash and replace sidecars".to_string()),
                );
            }
        }
    }

    /// Handle fetch requests from computation results.
    ///
    /// Seeds the MB known entity table with the requested release IDs so the
    /// fetch scheduler will pick them up, then triggers a fetch cycle. The
    /// `then` computations are stashed in `post_fetch_computations` and queued
    /// when the scheduler reports AllDone.
    fn handle_fetch_requests(&mut self, requests: Vec<crate::meta::computations::FetchRequest>) {
        let mut need_fetch = false;
        for req in requests {
            if !req.mb_release_ids.is_empty() {
                // Seed the known entities table so the scheduler discovers these releases.
                if let Some(sender) = crate::db::write_thread::signal_sender() {
                    let now = chrono::Utc::now().timestamp();
                    for release_id in &req.mb_release_ids {
                        sender.insert_mb_known_entity(
                            release_id,
                            "release",
                            Some("pinned_release"),
                            now,
                        );
                    }
                }
                need_fetch = true;
            }
            self.post_fetch_computations.extend(req.then);
        }
        if need_fetch {
            if let Some(ref mut handle) = self.external_fetch {
                crate::logging::log_general(
                    "[WITCH] Triggering fetch for pinned release data"
                );
                handle.request_fetch();
            }
        }
    }
}
