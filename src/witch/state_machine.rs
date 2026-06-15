//! Work lifecycle state machine for the Witch.
//!
//! Governs Work/Done/Idle transitions, reasoning level advancement
//! (None → Inodes → Full), startup state progression (Reconciling →
//! Vacuuming → Ready), and post-transition follow-up work scheduling.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::meta::computations::{analysis, derivation, Computation};

use super::pipeline_triggers::{self, IdlePromotionState};
use super::types::{ContentAnalysisWitness, ReasoningLevel, TaskKind, WatcherState, WorkState};

// ============================================================================
// PostTransitionWork
// ============================================================================

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

// ============================================================================
// State Machine impl
// ============================================================================

impl super::Witch {
    /// Check startup state transitions (Reconciling → Vacuuming → Ready).
    pub(super) fn check_startup_transitions(&mut self) {
        match self.startup_state {
            super::types::WitchStartupState::Reconciling
                if !self.has_pending() && !self.vacuum_check_pending =>
            {
                crate::logging::log_general("[WITCH] Schema reconciliation complete");
                self.cache_thread_handle.reconnect_db();
                self.startup_schema_descriptions.clear();
                // Vacuum check is async — offloaded to a blocking task.
                // The offload handler will transition to Vacuuming or Ready.
                self.request_vacuum_check();
            }
            super::types::WitchStartupState::Vacuuming if !self.has_pending() => {
                crate::logging::log_general("[WITCH] Vacuum complete — entering Ready");
                self.startup_state = super::types::WitchStartupState::Ready;
            }
            _ => {}
        }

        // Auto-start watcher when Ready and not yet scanning.
        if self.startup_state == super::types::WitchStartupState::Ready
            && self.watcher_state == WatcherState::NotStarted
        {
            self.start_watching();
        }
    }

    /// If watcher events changed the observed inode maps, queue derivation.
    pub(super) fn maybe_trigger_derivation(&mut self) {
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
    }

    /// Update state machine based on in-flight tasks and timing.
    pub(super) fn update_state(&mut self) {
        match self.work_state {
            WorkState::Idle => {
                // Idle → Working: handled in queue methods.
                // Idle → Idle: poll for idle-timer auto-promotion (full repack
                // catch-up after accumulated incremental scoring passes).
                self.maybe_run_idle_promotion();
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
                        "[TRANSACTION] Phase advancement: queueing {:?} ({} mutations). \
                         {} phase(s) remaining.",
                        stage, mutations.len(), self.pending_mutation_phases.len()
                    ));
                    // Guard above already confirmed in_flight==0 && queue_empty.
                    // No workers running ⇒ no new writes can arrive ⇒ queue stays empty.
                    debug_assert!(
                        self.db_thread_handle.queue_empty(),
                        "Phase advancement: guard confirmed queue_empty but it changed"
                    );

                    // Extract label from current WorkState (preserves session label)
                    let label = if let WorkState::Working { ref label, .. } = self.work_state {
                        label.clone()
                    } else {
                        None
                    };
                    self.queue_mutations_internal(mutations, label);
                    return; // Stay in Working — more work queued
                }

                // Soft mutation phases (from auto-deploy) — between mutation
                // and computation phases.
                if self.work_state.in_flight() == 0
                    && self.db_thread_handle.queue_empty()
                    && !self.pending_soft_mutation_phases.is_empty()
                {
                    let (_phase, soft_mutations) =
                        self.pending_soft_mutation_phases.pop_front().unwrap();
                    crate::logging::log_general(format!(
                        "[AUTO-DEPLOY] Phase advancement: queueing {} soft mutations. \
                         {} phase(s) remaining.",
                        soft_mutations.len(),
                        self.pending_soft_mutation_phases.len()
                    ));
                    debug_assert!(
                        self.db_thread_handle.queue_empty(),
                        "Phase advancement: guard confirmed queue_empty but it changed"
                    );

                    let label = if let WorkState::Working { ref label, .. } = self.work_state {
                        label.clone()
                    } else {
                        None
                    };
                    for sm in soft_mutations {
                        self.enqueue_one(
                            super::types::Task::SoftMutation(sm),
                            label.clone(),
                        );
                    }
                    return; // Stay in Working — more work queued
                }

                if self.work_state.in_flight() == 0
                    && self.db_thread_handle.queue_empty()
                    && !self.pending_computation_phases.is_empty()
                {
                    let (stage, computations) =
                        self.pending_computation_phases.pop_front().unwrap();
                    crate::logging::log_general(format!(
                        "[PIPELINE] Phase advancement: queueing {} ({} computations). \
                         {} phase(s) remaining.",
                        stage.label(), computations.len(), self.pending_computation_phases.len()
                    ));
                    debug_assert!(
                        self.db_thread_handle.queue_empty(),
                        "Phase advancement: guard confirmed queue_empty but it changed"
                    );

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

                // Auto-index check is async — offloaded to a blocking task.
                // The offload handler will either queue index mutations or
                // trigger content analysis when the result arrives.
                self.request_auto_index_check(
                    super::types::AutoIndexSource::InodesTransition,
                );
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

                    // Startup obligation: the Inodes→Full auto-index queued
                    // mutations and parked them behind the linger gate. Fire
                    // content analysis now so the deploy/release pipeline
                    // reclassifies dirty inodes carried across the restart,
                    // without waiting on the linger → fetch chain. One-shot —
                    // only fires after the initial post-Inodes auto-index pass.
                    if self.pending_startup_content_analysis {
                        self.pending_startup_content_analysis = false;
                        work.content_analysis = true;
                    }
                } else if self.pending_recomputation_scope.is_some() {
                    // Re-derivation completed after a mutation session.
                    // The pending scope was stored when mutations drained — now
                    // that derivation has reconciled signals, schedule content
                    // analysis to recompute deploy status, overlaps, etc.
                    crate::logging::log_general(format!(
                        "[STATE] Post-mutation re-derivation complete (scope={:?}). \
                         Scheduling content analysis. Processed {} tasks.",
                        self.pending_recomputation_scope, session_processed
                    ));
                    work.content_analysis = true;
                }
                // Steady-state: derivation completed (no prior mutations).
                // Check for newly unindexed files to auto-index.
                if !had_mutations && self.pending_recomputation_scope.is_none() {
                    self.request_auto_index_check(
                        super::types::AutoIndexSource::SteadyState,
                    );
                }
            }

            // Maintenance can complete while None - this is valid, just NOP
            ReasoningLevel::None => {
                let only_maintenance = self
                    .kind_counts
                    .keys()
                    .all(|k| *k == TaskKind::Maintenance);
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

        // Bump computations_generation if any computations ran this session
        if self.kind_counts.get(&TaskKind::Computation).is_some_and(|&c| c > 0) {
            self.computations_generation += 1;
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

        // Sidecar deploy fires eagerly — no idle linger, no idle priority chain.
        // Content analysis emitted SidecarDeployReady signals; deploy them now.
        if self.pending_work.contains(super::types::PendingWork::SIDECAR_DEPLOY) {
            let auto_deploy_enabled = self
                .read_config(|c| c.opinions.auto_deploy)
                .unwrap_or(false);
            if auto_deploy_enabled {
                self.pending_work.remove(super::types::PendingWork::SIDECAR_DEPLOY);
                self.request_sidecar_deploy_check();
            }
        }
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

        // transition_to_completed() is only reached via has_pending()==false, which
        // includes db_thread_handle.queue_empty(). With in_flight==0, no workers can
        // enqueue new writes, so the queue stays empty.
        debug_assert!(
            self.db_thread_handle.queue_empty(),
            "dispatch_post_transition_work: has_pending() confirmed queue_empty but it changed"
        );

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
    /// Takes the accumulated observed inode maps and queues DeriveCorpusSignals.
    /// When `include_second_level` is true, also queues
    /// ScheduleSecondLevelDerivations for directory checks and library walks.
    pub(super) fn queue_awakening_computations(&mut self, include_second_level: bool) {
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

        let label = if include_second_level { "Awakening" } else { "Steady-state derivation" };
        let extras = if include_second_level {
            ", ScheduleSecondLevelDerivations, ReconcileLibraryFiles"
        } else {
            ", ReconcileLibraryFiles"
        };
        crate::logging::log_general(format!(
            "[STATE] Queueing {}: DeriveCorpusSignals ({} inodes){}",
            label, observed_corpus.len(), extras,
        ));

        self.queue_computation_with_label(
            Computation::Derivation(derivation::Computation::DeriveCorpusSignals {
                observed_inodes: observed_corpus,
            }),
            Some("Deriving corpus signals".to_string()),
        );

        if include_second_level {
            self.queue_computation_with_label(
                Computation::Derivation(derivation::Computation::ScheduleSecondLevelDerivations),
                Some("Computing directory signals".to_string()),
            );
        }

        // Reconcile the watcher's authoritative live library set against the DB
        // every tick — both at awakening and in steady state. Without this on
        // steady-state ticks, file removals observed by the watcher post-startup
        // never propagate to the `files` table; stale rows then drive a
        // perpetual LibraryLeftover loop where stash mutations fail with ENOENT
        // and the signal never clears.
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

        // Drain any silent-stuck pending_write / needs_tag_flush=1 inodes.
        // Awakening is a natural quiescence point (post-mutation, idle), so
        // VerifyPendingWrite reads can't race in-flight FlushTagsToDisk writes.
        self.queue_stale_flush_recovery();
    }

    /// Queue content analysis computations (internal only).
    ///
    /// This queues `ScheduleContentAnalysis` which will spawn bulk detection
    /// computations for FingerprintOverlap, CrossSourceOverlap, MissingTag, etc.
    ///
    /// Only callable from `transition_to_completed` when mutations drain while Awake.
    /// Sealed by requiring `ContentAnalysisWitness` which can only be created in that context.
    pub(super) fn queue_content_analysis(&mut self, _witness: ContentAnalysisWitness) {
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

        // Content analysis updates deploy signals.
        // AUDIO_DEPLOY waits for idle; SIDECAR_DEPLOY fires eagerly.
        self.pending_work.insert(
            super::types::PendingWork::AUDIO_DEPLOY
            | super::types::PendingWork::SIDECAR_DEPLOY
        );
    }

    fn transition_to_idle(&mut self) {
        let action = self.compute_idle_action();

        // Always clear FETCH flag — don't retry on every linger expiry if fetch can't start
        self.pending_work.remove(super::types::PendingWork::FETCH);

        match action {
            pipeline_triggers::IdleAction::TriggerFetch => {
                crate::logging::log_general(
                    "[WITCH] Auto-triggering AcoustID fetch after indexing",
                );
                let _ = self.request_external_fetch();
                // Stay in Done — fetch will produce scheduler messages
            }
            pipeline_triggers::IdleAction::TriggerPacking => {
                self.pending_work.remove(super::types::PendingWork::PACKING);
                crate::logging::log_general(
                    "[WITCH] Auto-triggering release packing after external fetch",
                );
                self.request_release_packing(true);
                // Stay in Done — packing work transitions to Working
            }
            pipeline_triggers::IdleAction::TriggerAutoDeploy => {
                self.pending_work.remove(super::types::PendingWork::AUDIO_DEPLOY);
                crate::logging::log_general(
                    "[WITCH] Auto-triggering deploy after content analysis",
                );
                self.request_auto_deploy_check();
                // Stay in Done — offload result will transition to Working
            }
            pipeline_triggers::IdleAction::TriggerFullRepack => {
                // Only reachable while polling Idle — Done→Idle has
                // last_idle_entry_at=None so the elapsed-time guard fails.
                // Defensive: handle the same way as the polling path.
                self.fire_idle_full_repack();
            }
            pipeline_triggers::IdleAction::GoIdle => {
                self.work_state = WorkState::Idle;
                self.last_idle_entry_at = Some(Instant::now());
            }
        }
    }

    /// Poll for idle-timer auto-promotion while the Witch is Idle.
    ///
    /// Called from `update_state()` on every housekeeping tick. Re-runs the
    /// idle decision with the current promotion state; if the timer has
    /// elapsed and we're sitting on accumulated incremental scoring data,
    /// kicks off a full repack.
    fn maybe_run_idle_promotion(&mut self) {
        // Cheap exits before reading config / running the decision logic.
        if !self.incremental_since_full_repack {
            return;
        }
        if self.last_idle_entry_at.is_none() {
            return;
        }

        let action = self.compute_idle_action();
        if matches!(action, pipeline_triggers::IdleAction::TriggerFullRepack) {
            self.fire_idle_full_repack();
        }
    }

    /// Run `decide_idle_action` against the Witch's current config and state.
    fn compute_idle_action(&self) -> pipeline_triggers::IdleAction {
        let has_api_key = self
            .read_config(|c| !c.opinions.external_matching.acoustid_api_key.is_empty())
            .unwrap_or(false);
        let auto_deploy_enabled = self
            .read_config(|c| c.opinions.auto_deploy)
            .unwrap_or(false);
        let idle_full_repack_after = self
            .read_config(|c| {
                Duration::from_secs(c.opinions.release_packing.idle_full_repack_after_secs)
            })
            .unwrap_or(Duration::ZERO);

        pipeline_triggers::decide_idle_action(
            self.pending_work,
            has_api_key,
            self.is_external_fetch_active(),
            auto_deploy_enabled,
            IdlePromotionState {
                incremental_since_full_repack: self.incremental_since_full_repack,
                last_idle_entry_at: self.last_idle_entry_at,
                idle_full_repack_after,
            },
        )
    }

    /// Apply the IdleAction::TriggerFullRepack effect: queue a full repack
    /// and reset the idle-promotion bookkeeping. The work-state transition
    /// to Working is handled inside `request_release_packing` via the
    /// computation queue path.
    fn fire_idle_full_repack(&mut self) {
        crate::logging::log_general(
            "[WITCH] Idle-timer promotion: triggering full release repack \
             after accumulated incremental passes",
        );
        // request_release_packing(false) clears incremental_since_full_repack.
        self.request_release_packing(false);
        // Leaving Idle → Working; clear the idle-entry timestamp so we don't
        // immediately re-trigger after the repack finishes.
        self.last_idle_entry_at = None;
    }

    pub(super) fn transition_to_working(&mut self) {
        if !self.work_state.is_working() {
            self.work_state = WorkState::Working {
                queued: 0,
                in_flight: 0,
                processed: 0,
                by_label: HashMap::new(),
                label: None,
            };
            // Leaving Idle (or Done): clear the idle-entry timestamp so the
            // promotion timer doesn't measure across a working stretch.
            self.last_idle_entry_at = None;
        }
    }
}
