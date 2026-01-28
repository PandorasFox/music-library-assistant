//! Per-Frame Update Logic
//!
//! Tick functions run each frame for views that need continuous updates
//! (progress screen, tag search).

use crate::ui::{progress_screen::{ProgressPhase, ProgressScreen}, startup, types::UiMode};
use super::App;

impl App {
    /// Tick the progress screen and check for completion.
    ///
    /// Called each frame while progress_screen is Some. Handles all progress phases:
    /// - Eyeballing: checks for unindexed files, proceeds to intake or content analysis
    /// - ContentAnalysis: transitions to Insights view on completion
    /// - SignalRefresh: transitions to Insights view on completion
    pub(super) fn tick_progress_screen(&mut self) {
        // Take progress_screen temporarily to avoid borrow conflicts
        let mut progress = match self.progress_screen.take() {
            Some(p) => p,
            None => return,
        };

        let phase = progress.phase();

        // Tick progress screen - it checks daemon state for completion
        let completed = progress.tick(self.witch());
        if completed {
            let status = self.witch().status();
            crate::logging::log_general(format!(
                "{:?} phase complete: {} processed",
                phase, status.total_processed
            ));
        }

        // Update stats on progress screen (for optional display)
        self.update_progress_stats(&mut progress);

        // Put it back or transition based on phase
        if progress.is_complete() {
            // Don't put it back - handle transition based on phase
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
                        self.intake_confirmation = Some(intake_state);
                        self.mode = UiMode::IntakeConfirmation;
                    } else {
                        let check_duration = transition_start.elapsed();
                        // Check if the Witch has pending work (e.g., freshen latch triggered content analysis)
                        if self.witch().has_pending() {
                            crate::logging::log_general(format!(
                                "[TRANSITION] check_for_unindexed_files took {}ms, no unindexed files but Witch has pending work - showing content analysis progress",
                                check_duration.as_millis()
                            ));
                            // Show content analysis progress screen for the pending work
                            self.progress_screen = Some(ProgressScreen::new_content_analysis());
                            self.mode = UiMode::Progress;
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
                    // Transition to Insights view
                    self.start_insights_view();
                }
            }
        } else {
            self.progress_screen = Some(progress);
        }
    }

    /// Tick tag search - checks for pending bulk edit after modal has rendered.
    pub(super) fn tick_tag_search(&mut self) {
        if let Some(ref mut search) = self.tag_search {
            if let Some(tracks) = search.take_pending_bulk_edit() {
                self.tag_search = None;
                self.start_unified_tag_editor_for_tracks(tracks);
            }
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

        // Query signal count - this can be slow with many signals
        let signals_start = std::time::Instant::now();
        let signal_count = read_db.inner().get_signals(None)
            .map(|s| s.len())
            .unwrap_or(0);
        crate::logging::log_general(format!(
            "[TRANSITION] get_signals(None) took {}ms, {} signals",
            signals_start.elapsed().as_millis(),
            signal_count
        ));

        let gather_start = std::time::Instant::now();
        let result = startup::IntakeConfirmationState::gather(read_db.inner(), &corpus_root, "corpus");
        crate::logging::log_general(format!(
            "[TRANSITION] IntakeConfirmationState::gather took {}ms",
            gather_start.elapsed().as_millis()
        ));

        result
    }
}
