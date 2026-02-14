//! Per-Frame Update Logic
//!
//! Tick functions run each frame for views that need continuous updates
//! (progress screen, tag search, progressive worker).

use std::time::{Duration, Instant};
use crossterm::event;

use crate::ui::{
    compound_split_v2,
    progress_screen::{ProgressPhase, ProgressScreen},
    progressive_worker::{OnComplete, ProgressiveWorkerState, WorkItem, WorkSummary},
    startup,
    transaction_review,
    ActiveView,
};
use super::App;
use super::eye::Eye;
use super::insights_view;

impl App {
    /// Tick the progress screen and check for completion.
    ///
    /// Called each frame while the active view is Progress. Handles all progress phases:
    /// - Eyeballing: checks for unindexed files, proceeds to intake or content analysis
    /// - ContentAnalysis: transitions to Insights view on completion
    /// - SignalRefresh: transitions to Insights view on completion
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
        let can_animate = self.witch().eye_state() == crate::witch::EyeState::Awake;
        eye.update(can_animate);

        // Tick progress screen - it checks daemon state for completion
        let completed = screen.tick(self.witch());
        if completed {
            let status = self.witch().status();
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
                        if self.witch().has_pending() {
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
                                "[TRANSITION] check_for_unindexed_files took {}ms, no unindexed files - skipping to Insights",
                                check_duration.as_millis()
                            ));
                            // No unindexed files, no mutations - skip content analysis entirely
                            // Corpus is unchanged from last session, signals are still valid
                            self.start_insights_view();
                        }
                    }
                }
                ProgressPhase::ContentAnalysis | ProgressPhase::SignalRefresh => {
                    // Invalidate insights cache before transitioning - mutations just completed
                    if let Some(ref witch) = self.witch {
                        witch.ui_read_cache().invalidate_insights_data();
                    }
                    // Transition to Insights view
                    self.start_insights_view();
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
        let corpus_root = self.config.corpus_dir();

        let eye_state = self.witch().eye_state();
        crate::logging::log_general(format!(
            "check_for_unindexed_files: eye_state={:?}",
            eye_state
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
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => {
                    worker.nops_elided += 1;
                    return;
                }
            };

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

        if let Some(ref mut witch) = self.witch {
            let _ = super::operator_decisions::stage_decision(witch, idx, &description, mutations);
            worker.mutations_generated += 1;
        } else {
            worker.nops_elided += 1;
        }
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
