//! Action Handlers for View-Specific Events
//!
//! Each view (insights, tag search, tree browser, tag editor, etc.) emits
//! Actions that are handled here. These handlers coordinate state transitions,
//! Witch interactions, and modal displays.
//!
//! Feature-specific flows are split into sub-modules:
//! - `compound_split`: Compound tag split resolution
//! - `tag_canonicity`: Tag canonicity resolution
//! - `oob_resolution`: OOB sync, OOB conflict, moved file flows
//! - `deploy`: Deployment preview flow
//! - `simple_resolutions`: Missing file/dir, corrupt, shit format, subpar dupe, directory overlap

mod compound_split;
mod tag_canonicity;
mod oob_resolution;
mod deploy;
mod simple_resolutions;

use crate::ui::{filter_popup, insights_view, oob_sync_flow, oob_conflict_flow, progress_screen, tag_search, transaction_review, tree_browser, tag_editor, startup, widgets, FilterPopupContext};
use crate::ui::types::{UiMode, ExitConfirmModalState};
use super::App;

impl App {
    // =========================================================================
    // Shared Helpers
    // =========================================================================

    /// Stage mutations into a new transaction for review.
    ///
    /// Starts a transaction with the given label, stages the mutations as a
    /// single decision. Used by simple resolution flows that have a straightforward
    /// "collect mutations → review → commit" pattern.
    pub(in crate::ui) fn stage_mutations_with_transaction(&mut self, mutations: Vec<crate::corpus::mutations::Mutation>, label: &str) {
        let Some(ref mut witch) = self.witch else { return };
        let _ = witch.start_transaction(label);
        let _ = super::operator_decisions::stage_decision(witch, 0, label, mutations);
    }

    /// Cancel the current flow: discard any active transaction and return to Insights.
    ///
    /// Logs the provided message, discards any open transaction, and navigates
    /// back to the Insights view. Callers should clear their own modal state after
    /// calling this.
    pub(in crate::ui) fn cancel_and_return_to_insights(&mut self, log_message: &str) {
        crate::logging::log_general(log_message);
        if let Some(ref mut witch) = self.witch {
            if witch.has_transaction() {
                let _ = super::operator_decisions::discard_transaction(witch);
            }
        }
        self.start_insights_view();
    }

    // =========================================================================
    // Post-Mutation Helpers
    // =========================================================================

    /// Transition to progress screen after queueing mutations.
    ///
    /// This is the standard post-mutation hook - use it after any transaction
    /// that queues mutations to the Witch. The progress screen shows work status
    /// and transitions to Insights when complete.
    ///
    /// - `ContentAnalysis`: For intake indexing (triggers metadata extraction)
    /// - `SignalRefresh`: For file operations (health signals need update)
    pub(super) fn transition_to_progress_after_mutations(&mut self, phase: progress_screen::ProgressPhase) {
        use progress_screen::ProgressPhase;

        let screen = match phase {
            ProgressPhase::Eyeballing => progress_screen::ProgressScreen::new_eyeballing(),
            ProgressPhase::ContentAnalysis => progress_screen::ProgressScreen::new_content_analysis(),
            ProgressPhase::SignalRefresh => progress_screen::ProgressScreen::new_signal_refresh(),
        };
        self.progress_screen = Some(screen);
        self.mode = UiMode::Progress;
    }

    // =========================================================================
    // View Action Handlers
    // =========================================================================

    pub(super) fn handle_insights_action(&mut self, action: insights_view::InsightsAction) {
        match action {
            insights_view::InsightsAction::None => {}
            insights_view::InsightsAction::RequestQuit => {
                // Check if operations are pending
                if self.has_pending_operations() {
                    self.status_message = Some("Cannot quit while operations are pending".to_string());
                } else {
                    // Show exit confirmation modal
                    self.exit_confirm_modal_state = Some(ExitConfirmModalState::default());
                    self.mode = UiMode::ExitConfirmModal;
                }
            }
            insights_view::InsightsAction::CycleNext => {
                self.insights_view = None;
                self.start_lateral_view(widgets::LateralView::Insights.next());
            }
            insights_view::InsightsAction::CyclePrev => {
                self.insights_view = None;
                self.start_lateral_view(widgets::LateralView::Insights.prev());
            }
            insights_view::InsightsAction::LaunchFlow => {
                // Use selected_action() to dispatch to appropriate flow
                match self.insights_view.as_ref().and_then(|v| v.selected_action()) {
                    Some(insights_view::InsightAction::LaunchDeploymentPreview) => {
                        self.start_deployment_preview_from_insights();
                    }
                    Some(insights_view::InsightAction::LaunchMissingFileResolution) => {
                        self.start_missing_file_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchTagCanonicityResolution) => {
                        self.start_tag_canonicity_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchCompoundTagSplitSafe) => {
                        // Extract tag name from selected insight type
                        let tag_name = self.insights_view.as_ref()
                            .and_then(|v| v.selected_insight_type())
                            .and_then(|t| match t {
                                insights_view::InsightType::CompoundTagValueSafe { tag_name } => Some(tag_name),
                                _ => None,
                            });
                        self.start_compound_split_resolution(true, tag_name.as_deref());
                    }
                    Some(insights_view::InsightAction::LaunchCompoundTagSplitReview) => {
                        // Extract tag name from selected insight type
                        let tag_name = self.insights_view.as_ref()
                            .and_then(|v| v.selected_insight_type())
                            .and_then(|t| match t {
                                insights_view::InsightType::CompoundTagValueReview { tag_name } => Some(tag_name),
                                _ => None,
                            });
                        self.start_compound_split_resolution(false, tag_name.as_deref());
                    }
                    Some(insights_view::InsightAction::LaunchOobTagSync) => {
                        self.start_oob_sync_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchOobTagConflict) => {
                        self.start_oob_conflict_inspection();
                    }
                    Some(insights_view::InsightAction::LaunchMovedFileAcknowledge) => {
                        self.start_moved_file_acknowledge();
                    }
                    Some(insights_view::InsightAction::LaunchCorruptFileResolution) => {
                        self.start_corrupt_file_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchShitFormatTranscode) => {
                        self.start_shit_format_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchIntakeConfirmation) => {
                        self.start_intake_confirmation_from_insights();
                    }
                    Some(insights_view::InsightAction::LaunchDirectoryOverlapResolution) => {
                        self.start_directory_overlap_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchSubparDuplicateResolution) => {
                        self.start_subpar_duplicate_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchMissingDirectoryResolution) => {
                        self.start_missing_directory_resolution();
                    }
                    Some(insights_view::InsightAction::NotImplemented) => {
                        self.status_message = Some("Flow not yet implemented".to_string());
                    }
                    Some(insights_view::InsightAction::Informational) | None => {
                        // Informational entries have no action
                    }
                }
            }
            insights_view::InsightsAction::ConfirmAllSafeCompoundSplits => {
                // Ctrl+A from insights: extract tag name and stage all for that tag
                let tag_name = self.insights_view.as_ref()
                    .and_then(|v| v.selected_insight_type())
                    .and_then(|t| match t {
                        insights_view::InsightType::CompoundTagValueSafe { tag_name } => Some(tag_name),
                        _ => None,
                    });
                self.confirm_all_safe_compound_splits_from_insights(tag_name.as_deref());
            }
        }
    }

    /// Start intake confirmation from Insights view.
    ///
    /// Gathers unindexed files and opens the intake confirmation modal.
    fn start_intake_confirmation_from_insights(&mut self) {
        let corpus_root = self.config.corpus_dir();
        let intake_state = self.witch.as_mut().and_then(|w| {
            let read_db = w.read_db();
            startup::IntakeConfirmationState::gather(&read_db, &corpus_root, "insights")
        });

        match intake_state {
            Some(state) => {
                self.intake_confirmation = Some(state);
                self.mode = UiMode::IntakeConfirmation;
            }
            None => {
                self.status_message = Some("No unindexed files to process".to_string());
            }
        }
    }

    /// Handle tag search actions.
    pub(super) fn handle_tag_search_action(&mut self, action: tag_search::TagSearchAction) {
        match action {
            tag_search::TagSearchAction::None => {}
            tag_search::TagSearchAction::Cancel => {
                // Return to Insights view
                self.tag_search = None;
                self.start_insights_view();
            }
            tag_search::TagSearchAction::CycleNext => {
                self.tag_search = None;
                self.start_lateral_view(widgets::LateralView::TagSearch.next());
            }
            tag_search::TagSearchAction::CyclePrev => {
                self.tag_search = None;
                self.start_lateral_view(widgets::LateralView::TagSearch.prev());
            }
            tag_search::TagSearchAction::ExecuteSearch => {
                // Execute search with db access - take ownership temporarily to avoid borrow conflict
                if let Some(mut search) = self.tag_search.take() {
                    let read_db = self.read_db();
                    search.execute_search(&read_db);
                    self.tag_search = Some(search);
                }
            }
            tag_search::TagSearchAction::EditAudioFile(audio_file) => {
                // Open unified tag editor for single audio file
                self.tag_search = None;
                self.start_unified_tag_editor_for_audio_file(audio_file);
            }
        }
    }

    /// Handle intake confirmation dialog actions.
    pub(super) fn handle_intake_confirmation_action(&mut self, action: startup::IntakeConfirmationAction) {
        use super::operator_decisions;

        match action {
            startup::IntakeConfirmationAction::None => {}
            startup::IntakeConfirmationAction::Confirmed => {
                // User confirmed - create IndexTrack mutations and stage for review
                let mutations = self.intake_confirmation
                    .as_ref()
                    .map(|s| s.create_index_mutations())
                    .unwrap_or_default();

                if mutations.is_empty() {
                    // No files to index (all deleted since detection?) - skip to Insights
                    crate::logging::log_general("IntakeConfirmation: no mutations to queue, skipping to Insights");
                    self.intake_confirmation = None;
                    self.start_insights_view();
                } else {
                    let count = mutations.len();
                    crate::logging::log_general(format!(
                        "IntakeConfirmation: user confirmed, staging {} IndexTrack mutations for review",
                        count
                    ));

                    // Start transaction and stage the decision
                    if let Some(the_witch) = self.witch.as_mut() {
                        let _ = the_witch.start_transaction("Intake indexing");
                        let _ = operator_decisions::stage_decision(
                            the_witch,
                            0,
                            "Index unindexed files",
                            mutations,
                        );
                    }

                    // Note: intake_confirmation state is NOT cleared - preserved for Cancel return
                    // Transition to review modal with ContentAnalysis phase for post-commit
                    self.start_transaction_review_with_phase(
                        transaction_review::TransactionReviewSource::IntakeConfirmation,
                        transaction_review::PostCommitPhase::ContentAnalysis,
                    );
                }
            }
            startup::IntakeConfirmationAction::Skipped => {
                // User skipped - no mutations ran, skip content analysis entirely
                // UnindexedFile signals remain for later handling
                crate::logging::log_general("IntakeConfirmation: user skipped indexing, going to Insights");

                // Discard any active transaction from review flow
                if let Some(ref mut witch) = self.witch {
                    if witch.has_transaction() {
                        let _ = operator_decisions::discard_transaction(witch);
                    }
                }
                self.intake_confirmation = None;
                self.start_insights_view();
            }
        }
    }

    pub(super) fn handle_tree_browser_action(&mut self, action: tree_browser::TreeBrowserAction) {
        match action {
            tree_browser::TreeBrowserAction::None => {}
            tree_browser::TreeBrowserAction::Cancel => {
                self.tree_browser = None;
                self.start_insights_view();
            }
            tree_browser::TreeBrowserAction::EditDirectory(path) => {
                // Load tracks from directory and open unified tag editor
                self.open_unified_tag_editor_for_directory(&path);
            }
            tree_browser::TreeBrowserAction::EditFile(path) => {
                // Load single track for editing
                self.start_tag_editor_for_path(&path, false);
            }
            tree_browser::TreeBrowserAction::CycleNext => {
                self.tree_browser = None;
                self.start_lateral_view(widgets::LateralView::CorpusBrowser.next());
            }
            tree_browser::TreeBrowserAction::CyclePrev => {
                self.tree_browser = None;
                self.start_lateral_view(widgets::LateralView::CorpusBrowser.prev());
            }
            tree_browser::TreeBrowserAction::OpenFilter => {
                // Open filter popup for corpus browser
                self.filter_popup_state = Some(filter_popup::FilterPopupState::new());
                self.filter_popup_context = Some(FilterPopupContext::CorpusBrowser);
            }
        }
    }

    pub(super) fn handle_unified_tag_editor_action(&mut self, action: tag_editor::UnifiedTagEditorAction) {
        use tag_editor::UnifiedTagEditorAction;

        match action {
            UnifiedTagEditorAction::None => {}
            UnifiedTagEditorAction::CloseModal => {}

            UnifiedTagEditorAction::StageDecisionAndNext { index, mutations } => {
                // Stage the decision AND navigate to next item
                self.stage_decision(index, mutations);
                if let Some(ref mut editor) = self.unified_tag_editor {
                    if editor.current_item_idx < editor.total_items.saturating_sub(1) {
                        editor.current_item_idx += 1;
                        editor.reset_field_state();
                    }
                }
            }

            UnifiedTagEditorAction::StageDecisionAndPrev { index, mutations } => {
                // Stage the decision AND navigate to previous item
                self.stage_decision(index, mutations);
                if let Some(ref mut editor) = self.unified_tag_editor {
                    if editor.current_item_idx > 0 {
                        editor.current_item_idx -= 1;
                        editor.reset_field_state();
                    }
                }
            }

            UnifiedTagEditorAction::StageDecisionAndReview { index, mutations } => {
                // Stage the decision AND immediately show transaction review
                // Used for aggregated mode or single-item contexts
                self.stage_decision(index, mutations);

                // Transition to standardized review modal
                // Note: unified_tag_editor state is NOT cleared - preserved for Cancel return
                self.start_transaction_review(transaction_review::TransactionReviewSource::TagEditor);
            }

            UnifiedTagEditorAction::DiscardTransaction => {
                // Discard all staged decisions via sealed operator decision handler
                if let Some(the_witch) = self.witch.as_mut() {
                    let _ = super::operator_decisions::discard_transaction(the_witch);
                }
                self.unified_tag_editor = None;
                self.start_insights_view();
                self.status_message = Some("Edits discarded".to_string());
            }

            UnifiedTagEditorAction::NextItem => {
                if let Some(ref mut editor) = self.unified_tag_editor {
                    if editor.current_item_idx < editor.total_items.saturating_sub(1) {
                        editor.current_item_idx += 1;
                        editor.reset_field_state();
                    }
                }
            }

            UnifiedTagEditorAction::PrevItem => {
                if let Some(ref mut editor) = self.unified_tag_editor {
                    if editor.current_item_idx > 0 {
                        editor.current_item_idx -= 1;
                        editor.reset_field_state();
                    }
                }
            }

            UnifiedTagEditorAction::StatusMessage(msg) => {
                self.status_message = Some(msg);
            }

            UnifiedTagEditorAction::RequestFillFromDb { inode } => {
                match inode {
                    Some(inode) => {
                        let read_db = self.read_db();
                        match read_db.get_corpus_tags(inode) {
                            Ok(tags) => {
                                // Convert AudioTag to (name, value) pairs
                                let tag_pairs: Vec<(String, String)> = tags
                                    .into_iter()
                                    .map(|t| (t.tag_name, t.tag_value))
                                    .collect();

                                if let Some(ref mut editor) = self.unified_tag_editor {
                                    editor.fill_from_db_result(tag_pairs);
                                }
                                self.status_message = Some("Tags loaded from database".to_string());
                            }
                            Err(e) => {
                                self.status_message = Some(format!("Error loading tags: {}", e));
                            }
                        }
                    }
                    None => {
                        self.status_message = Some("Track not indexed - no database tags available".to_string());
                    }
                }
            }

            UnifiedTagEditorAction::RequestTransactionReview => {
                // Transition to standardized review modal
                // Note: unified_tag_editor state is NOT cleared - preserved for Cancel return
                self.start_transaction_review(transaction_review::TransactionReviewSource::TagEditor);
            }
        }
    }

    // ========================================================================
    // Standardized Transaction Review
    // ========================================================================

    /// Handle actions from the standardized transaction review modal.
    ///
    /// All mutation flows route through this review modal:
    /// - Cancel: return to source modal (state preserved)
    /// - Discard: discard transaction, clear all modal states, return to Insights
    /// - Confirm: commit transaction, clear all modal states, go to Progress
    pub(super) fn handle_transaction_review_action(&mut self, action: transaction_review::TransactionReviewAction) {
        use transaction_review::{TransactionReviewAction, TransactionReviewSource};

        match action {
            TransactionReviewAction::None => {}

            TransactionReviewAction::Cancel => {
                // Return to source modal - state was preserved
                let source = self.transaction_review.as_ref().map(|r| r.source);
                self.transaction_review = None;

                match source {
                    Some(TransactionReviewSource::TagEditor) => {
                        // unified_tag_editor state was preserved
                        self.mode = UiMode::UnifiedTagEditor;
                    }
                    Some(TransactionReviewSource::TagCanonicityResolution) => {
                        // Reload current cluster into V2 tag_canonicity_state
                        if self.load_current_cluster_signal() {
                            self.mode = UiMode::TagCanonicityResolution;
                        } else {
                            // Signal no longer exists - return to Insights
                            self.start_insights_view();
                        }
                    }
                    Some(TransactionReviewSource::DeployPreview) => {
                        // deployment_preview state was preserved
                        self.mode = UiMode::DeploymentPreview;
                    }
                    Some(TransactionReviewSource::MissingFileResolution) => {
                        // missing_file_preview state was preserved
                        self.mode = UiMode::MissingFileResolution;
                    }
                    Some(TransactionReviewSource::MissingDirectoryResolution) => {
                        // missing_directory_preview state was preserved
                        self.mode = UiMode::MissingDirectoryResolution;
                    }
                    Some(TransactionReviewSource::IntakeConfirmation) => {
                        // intake_confirmation state was preserved
                        self.mode = UiMode::IntakeConfirmation;
                    }
                    Some(TransactionReviewSource::CompoundTagSplit) => {
                        // Reload current signal into compound_split_state
                        if self.load_current_compound_split_signal() {
                            self.mode = UiMode::CompoundTagSplit;
                        } else {
                            // Signal no longer exists - return to Insights
                            self.start_insights_view();
                        }
                    }
                    Some(TransactionReviewSource::OobSyncResolution) => {
                        // oob_sync_state was preserved
                        self.mode = UiMode::OobSyncResolution;
                    }
                    Some(TransactionReviewSource::OobConflictResolution) => {
                        // oob_conflict_state was preserved
                        self.mode = UiMode::OobConflictInspection;
                    }
                    Some(TransactionReviewSource::CorruptFileResolution) => {
                        // corrupt_file_preview state was preserved
                        self.mode = UiMode::CorruptFileResolution;
                    }
                    Some(TransactionReviewSource::ShitFormatResolution) => {
                        // shit_format_preview state was preserved
                        self.mode = UiMode::ShitFormatResolution;
                    }
                    Some(TransactionReviewSource::SubparDuplicateResolution) => {
                        // subpar_duplicate_preview state was preserved
                        self.mode = UiMode::SubparDuplicateResolution;
                    }
                    Some(TransactionReviewSource::DirectoryClusterResolution) => {
                        // directory_cluster_preview state was preserved
                        self.mode = UiMode::DirectoryClusterResolution;
                    }
                    None => self.start_insights_view(),
                }
            }

            TransactionReviewAction::Discard => {
                // Discard transaction and clear all modal states
                if let Some(ref mut witch) = self.witch {
                    let _ = super::operator_decisions::discard_transaction(witch);
                }
                self.clear_all_modal_states();
                self.start_insights_view();
                self.status_message = Some("Transaction discarded".to_string());
            }

            TransactionReviewAction::Confirm => {
                // Determine progress phase before clearing state
                let post_commit_phase = self.transaction_review.as_ref()
                    .map(|r| r.post_commit_phase)
                    .unwrap_or_default();

                // Commit transaction and clear all modal states
                let commit_result = if let Some(ref mut witch) = self.witch {
                    super::operator_decisions::commit_transaction(witch)
                } else {
                    Err(crate::witch::TransactionError::NoActiveTransaction)
                };

                self.clear_all_modal_states();

                match commit_result {
                    Ok(summary) => {
                        self.status_message = Some(format!(
                            "Committed {} decision{} ({} mutation{})",
                            summary.decision_count,
                            if summary.decision_count == 1 { "" } else { "s" },
                            summary.mutation_count,
                            if summary.mutation_count == 1 { "" } else { "s" },
                        ));
                        // Use appropriate progress phase based on source
                        let phase = match post_commit_phase {
                            transaction_review::PostCommitPhase::SignalRefresh => {
                                progress_screen::ProgressPhase::SignalRefresh
                            }
                            transaction_review::PostCommitPhase::ContentAnalysis => {
                                progress_screen::ProgressPhase::ContentAnalysis
                            }
                        };
                        self.transition_to_progress_after_mutations(phase);
                    }
                    Err(e) => {
                        self.status_message = Some(format!("Commit failed: {}", e));
                        self.start_insights_view();
                    }
                }
            }
        }
    }

    /// Clear all modal states when committing or discarding a transaction.
    fn clear_all_modal_states(&mut self) {
        self.transaction_review = None;
        self.unified_tag_editor = None;
        self.tag_canonicity_state = None;
        self.tag_canonicity_clusters = None;
        self.compound_split_state = None;
        self.compound_split_clusters = None;
        self.deployment_preview = None;
        self.missing_file_preview = None;
        self.missing_directory_preview = None;
        self.intake_confirmation = None;
        self.oob_sync_state = None;
        self.oob_conflict_state = None;
        self.corrupt_file_preview = None;
        self.shit_format_preview = None;
        self.subpar_duplicate_preview = None;
        self.directory_cluster_preview = None;
    }

    /// Transition to the standardized transaction review modal.
    ///
    /// Called after staging decisions to show the review before commit.
    pub(in crate::ui) fn start_transaction_review(&mut self, source: transaction_review::TransactionReviewSource) {
        self.transaction_review = Some(transaction_review::TransactionReviewState::new(source));
        self.mode = UiMode::TransactionReview;
    }

    /// Transition to transaction review modal with custom post-commit phase.
    ///
    /// Used for intake indexing which needs ContentAnalysis instead of SignalRefresh.
    pub(super) fn start_transaction_review_with_phase(
        &mut self,
        source: transaction_review::TransactionReviewSource,
        phase: transaction_review::PostCommitPhase,
    ) {
        self.transaction_review = Some(
            transaction_review::TransactionReviewState::new(source)
                .with_post_commit_phase(phase)
        );
        self.mode = UiMode::TransactionReview;
    }

    // =========================================================================
    // Mouse Click Handling
    // =========================================================================

    /// Handle mouse click at the given position.
    ///
    /// This dispatches to the current mode's click handler to check for
    /// button hits. Mouse clicks on decision buttons are equivalent to
    /// Enter key presses for decision witnessing.
    pub(super) fn handle_click(&mut self, x: u16, y: u16) {
        match self.mode {
            UiMode::OobSyncResolution => {
                if let Some(ref state) = self.oob_sync_state {
                    if let Some(button_name) = state.button_rects.hit_test(x, y) {
                        // Simulate the button press action
                        let action = match button_name {
                            "accept_disk" => oob_sync_flow::OobSyncAction::AcceptDisk,
                            "accept_db" => oob_sync_flow::OobSyncAction::AcceptDb,
                            "cancel" => oob_sync_flow::OobSyncAction::Cancel,
                            _ => oob_sync_flow::OobSyncAction::None,
                        };
                        self.handle_oob_sync_action(action);
                    }
                }
            }
            UiMode::OobConflictInspection => {
                if let Some(ref state) = self.oob_conflict_state {
                    if let Some(button_name) = state.button_rects.hit_test(x, y) {
                        // Simulate the button press action
                        let action = match button_name {
                            "apply_db" => oob_conflict_flow::OobConflictAction::Resolve,
                            "assimilate_disk" => oob_conflict_flow::OobConflictAction::Resolve,
                            "acknowledge" => oob_conflict_flow::OobConflictAction::Acknowledge,
                            "cancel" => oob_conflict_flow::OobConflictAction::Cancel,
                            _ => oob_conflict_flow::OobConflictAction::None,
                        };
                        if button_name == "apply_db" {
                            // Set button to ApplyDb before handling
                            if let Some(ref mut state) = self.oob_conflict_state {
                                state.selected_button = oob_conflict_flow::types::ResolutionButton::ApplyDb;
                            }
                        } else if button_name == "assimilate_disk" {
                            if let Some(ref mut state) = self.oob_conflict_state {
                                state.selected_button = oob_conflict_flow::types::ResolutionButton::AssimilateDisk;
                            }
                        }
                        self.handle_oob_conflict_action(action);
                    }
                }
            }
            UiMode::Insights => {
                if let Some(ref mut view) = self.insights_view {
                    view.handle_click(x, y);
                }
            }
            // Add other modes as needed
            _ => {}
        }
    }
}
