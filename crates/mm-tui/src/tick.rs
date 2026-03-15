//! Per-Frame Update Logic
//!
//! Tick functions run each frame for views that need continuous updates
//! (progress screen, tag search, progressive worker).

use crossterm::event;
use std::time::{Duration, Instant};

use super::eye::Eye;
use super::insights_view;
use super::App;
use crate::{
    progress_screen::{ProgressPhase, ProgressScreen},
    progressive_worker::{OnComplete, ProgressiveWorkerState, WorkItem, WorkSummary},
    startup, transaction_review, ActiveView,
};

impl App {
    /// Tick the progress screen and check for completion.
    ///
    /// Called each frame while the active view is Progress. Handles all progress phases:
    /// - Eyeballing: checks for unindexed files, proceeds to intake or content analysis
    /// - ContentAnalysis: transitions to Health view on completion
    /// - SignalRefresh: transitions to Health view on completion
    ///
    /// Also drives the eye animation (moved here from run_app, scoped to Progress).
    pub(super) fn tick_progress_screen(&mut self) {
        if !matches!(self.view, ActiveView::Progress { .. }) {
            return;
        }

        // Take the progress view out temporarily via replace with a throwaway Insights.
        let old = std::mem::replace(
            &mut self.view,
            ActiveView::Insights {
                data: insights_view::InsightsViewData::new(),
                interaction: insights_view::HealthInteraction::new(),
            },
        );
        let ActiveView::Progress {
            mut screen,
            mut eye,
        } = old
        else {
            unreachable!()
        };

        let phase = screen.phase();

        // Update eye animation (scoped to Progress view)
        let can_animate = self.witch_status().reasoning_level == mm_meta::witch_types::ReasoningLevel::Full;
        eye.update(can_animate);

        // Tick progress screen - it checks daemon state for completion
        let completed = screen.tick(self.witch_status());
        if completed {
            let status = self.witch_status().work.clone();
            mm_meta::logging::log_general(format!(
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
                    if let Some(intake_state) = self.check_for_unindexed_files() {
                        self.view = ActiveView::IntakeConfirmation(intake_state);
                    } else if self.witch_status().has_pending {
                        // Witch has pending work (e.g., freshen latch triggered content analysis)
                        self.view = ActiveView::Progress {
                            screen: ProgressScreen::new_content_analysis(),
                            eye: Eye::default(),
                        };
                    } else {
                        // No unindexed files, no mutations - skip content analysis entirely
                        // Corpus is unchanged from last session, signals are still valid
                        self.start_default_view();
                    }
                }
                ProgressPhase::ContentAnalysis | ProgressPhase::SignalRefresh => {
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
        if let Some(inodes) = pending {
            let files = self.query(mm_meta::domain_queries::GetAudioFilesByInodes {
                inodes,
                zone: mm_meta::db_types::Zone::Corpus,
            });
            if !files.is_empty() {
                self.push_current_view();
                self.start_unified_tag_editor_for_audio_files(files);
            }
        }
    }

    /// Check for unindexed files after Awakening completes.
    ///
    /// Queries UnindexedFile signals (emitted by DeriveZoneSignals / UpdateCorpusFileSignals).
    /// Returns Some if there are unindexed files to confirm, None otherwise.
    pub(super) fn check_for_unindexed_files(&mut self) -> Option<startup::IntakeConfirmationState> {
        let reasoning = self.witch_status().reasoning_level;
        mm_meta::logging::log_general(format!(
            "check_for_unindexed_files: reasoning_level={:?}",
            reasoning
        ));

        self.query(mm_meta::domain_queries::GetIntakeConfirmation {
                source: startup::IntakeSource::Startup,
                zone: None,
            })
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
        if !matches!(self.view, ActiveView::ProgressiveWork(_)) {
            return;
        }

        // Take the progressive work view out temporarily
        let old = std::mem::replace(
            &mut self.view,
            ActiveView::Insights {
                data: insights_view::InsightsViewData::new(),
                interaction: insights_view::HealthInteraction::new(),
            },
        );
        let ActiveView::ProgressiveWork(mut worker) = old else {
            unreachable!()
        };

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
    fn process_work_item(&mut self, _item: &WorkItem, _worker: &mut ProgressiveWorkerState) {
        // No active work item types — the compound split progressive worker
        // was replaced by the V3 interactive modal.
    }

    /// Handle completion of progressive work.
    ///
    /// The view stack already holds the suspended view from the earlier
    /// push_and_switch, so we just set the active view to TransactionReview.
    fn handle_progressive_complete(&mut self, on_complete: OnComplete, summary: WorkSummary) {
        match on_complete {
            OnComplete::CompoundSplitStaging => {
                if summary.nops_elided > 0 {
                    self.status_message = Some(format!(
                        "Staged {} compound tag splits ({} skipped)",
                        summary.mutations_generated, summary.nops_elided,
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
                let decisions = transaction_review::fetch_decision_summaries(self);
                let mut review = transaction_review::TransactionReviewState::new();
                review.set_decisions(decisions);
                self.view = ActiveView::TransactionReview(review);
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
