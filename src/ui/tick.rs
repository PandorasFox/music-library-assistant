//! Per-Frame Update Logic
//!
//! Tick functions run each frame for views that need continuous updates
//! (progress screen, tag search, progressive worker).

use std::time::{Duration, Instant};
use crossterm::event;

use crate::meta::decisions::{DecisionKey, DecisionSource};
use crate::ui::{
    compound_split_v2,
    progress_screen::{ProgressPhase, ProgressScreen},
    progressive_worker::{OnComplete, ProgressiveWorkerState, WorkItem, WorkSummary},
    startup,
    transaction_review,
    ActiveView,
    MigrationPhase, VacuumPhase,
};
use super::App;
use super::eye::Eye;
use super::insights_view;

impl App {
    /// Tick the migration approval view.
    ///
    /// When phase is Running: check if all migrations have completed.
    /// When complete: show completion briefly, then advance.
    pub(super) fn tick_migration_approval(&mut self) {
        let phase = match self.view {
            ActiveView::MigrationApproval(ref state) => state.phase,
            _ => return,
        };

        match phase {
            MigrationPhase::Running => {
                // Tick the Witch to process migration tasks
                self.witch.tick();
                if !self.witch.has_pending() {
                    // All migrations complete
                    if let ActiveView::MigrationApproval(ref mut state) = self.view {
                        state.phase = MigrationPhase::Complete;
                    }
                }
            }
            MigrationPhase::Complete => {
                // Invalidate read-only connection so it picks up new schema
                self.witch.invalidate_read_only_conn();

                // Advance past migrations
                let db_path = self.db_path.clone();
                let threshold = self.vacuum_threshold;
                self.advance_past_migrations(&db_path, threshold);
            }
            MigrationPhase::Approval => {
                // Waiting for user input, nothing to tick
            }
        }
    }

    /// Tick the vacuum prompt view.
    ///
    /// When phase is Compacting: execute VACUUM synchronously, then show completion.
    /// When Complete: advance to complete_startup after a brief delay.
    pub(super) fn tick_vacuum_prompt(&mut self) {
        let phase = match self.view {
            ActiveView::VacuumPrompt(ref state) => state.phase,
            _ => return,
        };

        match phase {
            VacuumPhase::Compacting => {
                // First tick in Compacting: mark as rendered so the UI shows
                // "Compacting database..." for at least one frame before we block.
                if let ActiveView::VacuumPrompt(ref mut state) = self.view {
                    if !state.compacting_rendered {
                        state.compacting_rendered = true;
                        return;
                    }
                }

                // Execute VACUUM via db_thread
                let db_path = match self.view {
                    ActiveView::VacuumPrompt(ref state) => state.db_path.clone(),
                    _ => return,
                };

                let vacuum_result = self.witch.execute_vacuum();

                match vacuum_result {
                    Ok(()) => {
                        // Re-query to show reclaimed amount
                        let new_size_mb = Self::query_db_size_mb(&db_path).unwrap_or(0.0);
                        if let ActiveView::VacuumPrompt(ref mut state) = self.view {
                            state.phase = VacuumPhase::Complete { new_size_mb };
                        }
                    }
                    Err(e) => {
                        crate::logging::log_error(format!("Vacuum failed: {}", e));
                        // Skip vacuum and complete startup
                        self.complete_startup();
                    }
                }
            }
            VacuumPhase::Complete { .. } => {
                // Advance to normal startup
                self.complete_startup();
            }
            VacuumPhase::Prompt => {
                // Waiting for user input
            }
        }
    }

    /// Query the current database size in MB.
    fn query_db_size_mb(db_path: &std::path::Path) -> Option<f64> {
        let conn = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ).ok()?;
        let page_count: u64 = conn.pragma_query_value(None, "page_count", |row| row.get(0)).ok()?;
        let page_size: u64 = conn.pragma_query_value(None, "page_size", |row| row.get(0)).ok()?;
        Some((page_count * page_size) as f64 / (1024.0 * 1024.0))
    }

    /// Tick the progress screen and check for completion.
    ///
    /// Called each frame while the active view is Progress. Handles all progress phases:
    /// - Eyeballing: checks for unindexed files, proceeds to intake or content analysis
    /// - ContentAnalysis: transitions to Health view on completion
    /// - SignalRefresh: transitions to Health view on completion
    ///
    /// Also drives the eye animation (moved here from run_app, scoped to Progress).
    pub(super) fn tick_progress_screen(&mut self) {
        if !matches!(self.view, ActiveView::Progress { .. }) { return; }

        // Take the progress view out temporarily via replace with a throwaway Insights.
        let old = std::mem::replace(
            &mut self.view,
            ActiveView::Insights(insights_view::InsightsViewState::new()),
        );
        let ActiveView::Progress { mut screen, mut eye } = old else { unreachable!() };

        let phase = screen.phase();

        // Update eye animation (scoped to Progress view)
        let can_animate = self.witch.reasoning_level() == crate::witch::ReasoningLevel::Full;
        eye.update(can_animate);

        // Tick progress screen - it checks daemon state for completion
        let completed = screen.tick(&mut self.witch);
        if completed {
            let status = self.witch.status();
            crate::logging::log_general(format!(
                "{:?} phase complete: {} processed",
                phase, status.total_processed
            ));
        }

        // Update stats on progress screen (for optional display)
        self.update_progress_stats(&mut screen);

        // Handle completion or put the view back
        if screen.is_complete() {
            match phase {
                ProgressPhase::Eyeballing => {
                    // Check for unindexed files before deciding next phase
                    let transition_start = std::time::Instant::now();
                    if let Some(intake_state) = self.check_for_unindexed_files() {
                        let check_duration = transition_start.elapsed();
                        crate::logging::log_general(format!(
                            "[TRANSITION] check_for_unindexed_files took {}ms, found {} files",
                            check_duration.as_millis(),
                            intake_state.file_count
                        ));
                        self.view = ActiveView::IntakeConfirmation(intake_state);
                    } else {
                        let check_duration = transition_start.elapsed();
                        // Check if the Witch has pending work (e.g., freshen latch triggered content analysis)
                        if self.witch.has_pending() {
                            crate::logging::log_general(format!(
                                "[TRANSITION] check_for_unindexed_files took {}ms, no unindexed files but Witch has pending work - showing content analysis progress",
                                check_duration.as_millis()
                            ));
                            // Show content analysis progress screen for the pending work
                            self.view = ActiveView::Progress {
                                screen: ProgressScreen::new_content_analysis(),
                                eye: Eye::default(),
                            };
                        } else {
                            crate::logging::log_general(format!(
                                "[TRANSITION] check_for_unindexed_files took {}ms, no unindexed files - skipping to default view",
                                check_duration.as_millis()
                            ));
                            // No unindexed files, no mutations - skip content analysis entirely
                            // Corpus is unchanged from last session, signals are still valid
                            self.start_default_view();
                        }
                    }
                }
                ProgressPhase::ContentAnalysis | ProgressPhase::SignalRefresh => {
                    // Invalidate caches before transitioning - mutations just completed
                    self.witch.ui_read_cache().invalidate_insights_data();
                    self.witch.ui_read_cache().invalidate_inbox_overview();
                    self.witch.ui_read_cache().invalidate_deploy_status();
                    // Transition to configured default view
                    self.start_default_view();
                }
            }
        } else {
            // Not complete yet - put the view back for next frame
            self.view = ActiveView::Progress { screen, eye };
        }
    }

    /// Tick tag search - checks for pending bulk edit after modal has rendered.
    pub(super) fn tick_tag_search(&mut self) {
        let pending = if let ActiveView::TagSearch(ref mut search) = self.view {
            search.take_pending_bulk_edit()
        } else {
            None
        };
        if let Some(audio_files) = pending {
            self.push_current_view();
            self.start_unified_tag_editor_for_audio_files(audio_files);
        }
    }

    /// Check for unindexed files after Awakening completes.
    ///
    /// Queries UnindexedFile signals (computed during second-level derivation).
    /// Returns Some if there are unindexed files to confirm, None otherwise.
    pub(super) fn check_for_unindexed_files(&mut self) -> Option<startup::IntakeConfirmationState> {
        // Clone corpus_root to avoid borrow conflict with daemon's db reference
        let corpus_root = self.config().corpus_dir();

        let reasoning = self.witch.reasoning_level();
        crate::logging::log_general(format!(
            "check_for_unindexed_files: reasoning_level={:?}",
            reasoning
        ));

        let read_db = self.read_db();

        // Query signal count from typed tables
        let signals_start = std::time::Instant::now();
        let signal_count = read_db.count_all_signals();
        crate::logging::log_general(format!(
            "[TRANSITION] count_all_signals took {}ms, {} signals",
            signals_start.elapsed().as_millis(),
            signal_count
        ));

        let gather_start = std::time::Instant::now();
        let result = startup::IntakeConfirmationState::gather(&read_db, &corpus_root, "corpus");
        crate::logging::log_general(format!(
            "[TRANSITION] IntakeConfirmationState::gather took {}ms",
            gather_start.elapsed().as_millis()
        ));

        result
    }

    // =========================================================================
    // Progressive Worker Tick
    // =========================================================================

    /// Tick the progressive worker - process items in timed chunks.
    ///
    /// Called each frame while the active view is ProgressiveWork. Processes work items
    /// until the time budget (~50ms) is exhausted, then returns to allow render.
    /// On completion, drains input buffer and invokes the completion handler.
    pub(super) fn tick_progressive_worker(&mut self) {
        if !matches!(self.view, ActiveView::ProgressiveWork(_)) { return; }

        // Take the progressive work view out temporarily
        let old = std::mem::replace(
            &mut self.view,
            ActiveView::Insights(insights_view::InsightsViewState::new()),
        );
        let ActiveView::ProgressiveWork(mut worker) = old else { unreachable!() };

        let start = Instant::now();
        let time_budget = Duration::from_millis(50);

        // Process items until time budget exhausted or queue empty
        while start.elapsed() < time_budget {
            let Some(item) = worker.work_queue.pop_front() else {
                // Done! Drain input buffer, build summary, invoke callback
                drain_input_buffer();

                let summary = worker.build_summary();
                let on_complete = worker.on_complete.clone();

                self.handle_progressive_complete(on_complete, summary);
                return;
            };

            // Process single item
            self.process_work_item(&item, &mut worker);
            worker.processed += 1;
        }

        // Time budget exhausted - put worker back for next frame
        self.view = ActiveView::ProgressiveWork(worker);
    }

    /// Process a single work item.
    fn process_work_item(&mut self, item: &WorkItem, worker: &mut ProgressiveWorkerState) {
        match item {
            WorkItem::StageCompoundSplit { group, idx } => {
                self.process_compound_split_item(group, *idx, worker);
            }
        }
    }

    /// Process a single compound split work item (group-based).
    fn process_compound_split_item(
        &mut self,
        group: &crate::meta::signals::data::CompoundGroup,
        idx: usize,
        worker: &mut ProgressiveWorkerState,
    ) {
        let is_safe_mode = worker.is_safe_mode;
        let total = worker.total;

        // Load compound split data from the group (scoped borrow)
        let data = {
            let read_db = self.witch.read_db();

            match compound_split_v2::CompoundSplitDataV2::from_compound_group(group, &read_db) {
                Some(d) => d,
                None => {
                    worker.nops_elided += 1;
                    return;
                }
            }
        }; // db borrow ends here

        // Update current label for display
        worker.current_label = Some(format!(
            "Split \"{}\" in {}",
            data.compound.compound_value,
            data.compound.tag_name,
        ));

        // Create temporary state to generate mutations
        let state = compound_split_v2::CompoundSplitStateV2::new(
            data.clone(),
            is_safe_mode,
            idx,
            total,
        );
        let mutations = state.mutations();

        if mutations.is_empty() {
            worker.nops_elided += 1;
            return;
        }

        // Stage via operator_decisions
        let description = format!(
            "Split \"{}\" in {} \u{2192} [{}]",
            data.compound.compound_value,
            data.compound.tag_name,
            data.compound.split_parts.join(", ")
        );

        let _ = super::operator_decisions::stage_decision(&mut self.witch, DecisionKey::new(DecisionSource::CompoundSplit, idx.to_string()), &description, mutations, &worker.gesture);
        worker.mutations_generated += 1;
    }

    /// Handle completion of progressive work.
    ///
    /// The view stack already holds the suspended view from the earlier
    /// push_and_switch, so we just set the active view to TransactionReview.
    fn handle_progressive_complete(
        &mut self,
        on_complete: OnComplete,
        summary: WorkSummary,
    ) {
        match on_complete {
            OnComplete::CompoundSplitStaging => {
                if summary.nops_elided > 0 {
                    self.status_message = Some(format!(
                        "Staged {} compound tag splits ({} skipped)",
                        summary.mutations_generated,
                        summary.nops_elided,
                    ));
                } else {
                    self.status_message = Some(format!(
                        "Staged {} compound tag splits",
                        summary.mutations_generated,
                    ));
                }

                // Stack already holds compound split from the earlier push_and_switch.
                // Push this (now-completed) progressive worker position so the
                // TransactionReview Cancel pops back through it.
                self.view = ActiveView::TransactionReview(
                    transaction_review::TransactionReviewState::new(),
                );
            }
        }
    }

}

/// Drain any pending input events from the terminal buffer.
///
/// Call this after slow operations to prevent buffered keypresses from
/// being processed as if they were intentional input.
fn drain_input_buffer() {
    while event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        let _ = event::read();
    }
}
