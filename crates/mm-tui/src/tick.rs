//! Per-Frame Update Logic
//!
//! Tick functions run each frame for views that need continuous updates
//! (progress screen, tag search, progressive worker).

use crossterm::event;
use std::time::{Duration, Instant};

use super::eye::Eye;
use super::insights_view;
use super::App;
use mm_ui::modal_buttons::ModalButtons;
use mm_ui::resolutions::compound_split::{CompoundSplitButton, CompoundSplitButtonCtx};
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
        group: &mm_meta::signals::data::CompoundGroup,
        idx: usize,
        worker: &mut ProgressiveWorkerState,
    ) {
        use mm_meta::mutations::{tag_edit::ApplyTagOpsMutation, Mutation, TagOp};

        let is_safe_mode = worker.is_safe_mode;

        // Load compound split data from the group via cache thread
        let data = match self
            .query(mm_meta::domain_queries::GetCompoundSplitGroupData {
                group: group.clone(),
                zone: mm_meta::db_types::Zone::Corpus,
            })
        {
            Some(d) => d,
            None => {
                worker.nops_elided += 1;
                return;
            }
        };

        // Update current label for display
        worker.current_label = Some(format!(
            "Split \"{}\" in {}",
            data.compound.compound_value, data.compound.tag_name,
        ));

        // Build mutations directly from compound data
        let parts = &data.compound.split_parts;
        if parts.is_empty() {
            worker.nops_elided += 1;
            return;
        }

        let mut ops = Vec::new();
        for file in &data.files {
            // Replace compound value with first part
            if let Some(first_part) = parts.first() {
                ops.push(TagOp::replace_tag(
                    file.inode,
                    &data.compound.tag_name,
                    &data.compound.compound_value,
                    first_part,
                ));
            }
            // Add remaining parts
            for part in parts.iter().skip(1) {
                ops.push(TagOp::add_tag(
                    file.inode,
                    &data.compound.tag_name,
                    part,
                ));
            }
        }

        if ops.is_empty() {
            worker.nops_elided += 1;
            return;
        }

        let mutations = vec![Mutation::ApplyTagOps(ApplyTagOpsMutation {
            ops,
            zone: mm_meta::db_types::Zone::Corpus,
        })];

        // Stage via operator_decisions
        let description = format!(
            "Split \"{}\" in {} \u{2192} [{}]",
            data.compound.compound_value,
            data.compound.tag_name,
            parts.join(", ")
        );

        let ctx = CompoundSplitButtonCtx {
            has_files: true,
            current_group_index: idx,
            tag_name: data.compound.tag_name.clone(),
            zone: mm_meta::db_types::Zone::Corpus,
            safe_mode: is_safe_mode,
        };
        let key = CompoundSplitButton::Confirm.protocol_binding(&ctx)
            .decision_key().unwrap().clone();
        let decision = worker.gesture.decide(&description, mutations);
        let _ = super::operator_decisions::stage_decision(self, key, decision);
        worker.mutations_generated += 1;
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
