//! Per-Frame Update Logic
//!
//! Tick functions run each frame for views that need continuous updates
//! (splash screen, content analysis, intake confirmation, tag search).

use crate::config;
use crate::ui::{startup, types::UiMode};
use super::App;

impl App {
    /// Tick the splash screen and check for completion.
    ///
    /// Called each frame while splash_screen is Some. When daemon eye state
    /// becomes Awake (eyeballing complete), checks for unindexed files and
    /// proceeds to intake confirmation or content analysis.
    pub(super) fn tick_splash_screen(&mut self) {
        // Take splash_screen temporarily to avoid borrow conflicts
        let mut splash = match self.splash_screen.take() {
            Some(s) => s,
            None => return,
        };

        // Tick splash screen - it checks daemon.eye_state() for completion
        let completed = splash.tick(self.daemon());
        if completed {
            let status = self.daemon().status();
            let _ = config::log_message(&format!(
                "Startup eyeballing complete: {} processed",
                status.total_processed
            ));
        }

        // Update stats on splash screen (for optional display)
        self.update_progress_stats(&mut splash);

        // Put it back or transition
        if splash.is_complete() {
            // Don't put it back - check for unindexed files before transitioning
            let transition_start = std::time::Instant::now();
            if let Some(intake_state) = self.check_for_unindexed_files() {
                let check_duration = transition_start.elapsed();
                let _ = config::log_message(&format!(
                    "[TRANSITION] check_for_unindexed_files took {}ms, found {} files",
                    check_duration.as_millis(),
                    intake_state.file_count
                ));
                self.intake_confirmation = Some(intake_state);
                self.mode = UiMode::IntakeConfirmation;
            } else {
                let check_duration = transition_start.elapsed();
                let _ = config::log_message(&format!(
                    "[TRANSITION] check_for_unindexed_files took {}ms, no unindexed files",
                    check_duration.as_millis()
                ));
                // No unindexed files - skip intake, proceed to content analysis
                let analysis_start = std::time::Instant::now();
                self.start_content_analysis();
                let _ = config::log_message(&format!(
                    "[TRANSITION] start_content_analysis took {}ms",
                    analysis_start.elapsed().as_millis()
                ));
            }
        } else {
            self.splash_screen = Some(splash);
        }
    }

    /// Tick content analysis progress and check for completion.
    ///
    /// Called each frame while content_analysis is Some. When all content
    /// analysis computations complete, transitions to Insights view.
    pub(super) fn tick_content_analysis(&mut self) {
        // Take content_analysis temporarily to avoid borrow conflicts
        let mut progress = match self.content_analysis.take() {
            Some(p) => p,
            None => return,
        };

        // Tick progress screen - it checks daemon state for completion
        let completed = progress.tick(self.daemon());
        if completed {
            let status = self.daemon().status();
            let _ = config::log_message(&format!(
                "Metadata analysis complete: {} processed",
                status.total_processed
            ));
            // Transition to Insights view
            self.start_insights_view();
        } else {
            // Update stats on progress screen (for optional display)
            self.update_progress_stats(&mut progress);
            self.content_analysis = Some(progress);
        }
    }

    /// Tick intake confirmation while processing.
    ///
    /// Called each frame while intake_confirmation is in processing mode.
    /// When indexing completes, triggers transition to metadata analysis.
    pub(super) fn tick_intake_confirmation(&mut self) {
        // Take intake_confirmation temporarily to avoid borrow conflicts
        let mut state = match self.intake_confirmation.take() {
            Some(s) => s,
            None => return,
        };

        // Tick progress - it checks daemon state for completion
        let completed = state.tick(self.daemon());
        if completed {
            let status = self.daemon().status();
            let _ = config::log_message(&format!(
                "Intake indexing complete: {} processed",
                status.total_processed
            ));
            // Handle completion via action
            self.intake_confirmation = Some(state);
            self.handle_intake_confirmation_action(startup::IntakeConfirmationAction::ProcessingComplete);
        } else {
            // Update stats for display
            self.update_progress_stats(&mut state);
            self.intake_confirmation = Some(state);
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
        let corpus_root = self.config.corpus_root.clone();

        let eye_state = self.daemon().eye_state();
        let _ = config::log_message(&format!(
            "check_for_unindexed_files: eye_state={:?}",
            eye_state
        ));

        let db = self.db();

        // Query signal count - this can be slow with many signals
        let signals_start = std::time::Instant::now();
        let signal_count = db.get_health_signals(None)
            .map(|s| s.len())
            .unwrap_or(0);
        let _ = config::log_message(&format!(
            "[TRANSITION] get_health_signals(None) took {}ms, {} signals",
            signals_start.elapsed().as_millis(),
            signal_count
        ));

        let gather_start = std::time::Instant::now();
        let result = startup::IntakeConfirmationState::gather(db, &corpus_root, "corpus");
        let _ = config::log_message(&format!(
            "[TRANSITION] IntakeConfirmationState::gather took {}ms",
            gather_start.elapsed().as_millis()
        ));

        result
    }
}
