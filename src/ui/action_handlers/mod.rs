//! Action Handlers for View-Specific Events
//!
//! Each view (insights, tag search, tree browser, tag editor, etc.) emits
//! Actions that are handled here. These handlers coordinate state transitions,
//! Witch interactions, and modal displays.
//!
//! Feature-specific modals are split into sub-modules:
//! - `compound_split`: Compound tag split resolution
//! - `tag_canonicity`: Tag canonicity resolution
//! - `oob_resolution`: OOB sync, OOB conflict, moved file modals
//! - `deploy`: Deployment preview modal
//! - `simple_resolutions`: Missing file/dir, corrupt, shit format, subpar dupe, directory overlap

mod compound_split;
mod tag_canonicity;
mod oob_resolution;
mod deploy;
mod simple_resolutions;
mod witness;

use crate::ui::{filter_popup, insights_view, oob_sync_modal, oob_conflict_modal, progress_screen, tag_search, transaction_review, tree_browser, tag_editor, startup, widgets};
use crate::ui::active_view::{ActiveView, FilterOverlay, FilterPopupContext, SuspendedView, ViewAction};
use crate::ui::eye::Eye;
use super::App;

impl App {
    // =========================================================================
    // Action Dispatch
    // =========================================================================

    /// Dispatch a view action to the appropriate handler.
    ///
    /// `is_confirmation` is true when the triggering event was a confirmation
    /// gesture (Enter, Space, y/Y). The witness is minted internally from this
    /// flag - callers never touch the DecisionWitness type.
    pub(in crate::ui) fn dispatch_action(&mut self, action: ViewAction, is_confirmation: bool) {
        let witness = if is_confirmation {
            Some(witness::DecisionWitness::new())
        } else {
            None
        };

        match action {
            ViewAction::None => {}
            ViewAction::Insights(a) => self.handle_insights_action(a),
            ViewAction::CorpusBrowser(a) => self.handle_tree_browser_action(a),
            ViewAction::TagSearch(a) => self.handle_tag_search_action(a),
            ViewAction::ExitConfirm(a) => self.handle_exit_confirm_action(a),
            ViewAction::IntakeConfirmation(a) => self.handle_intake_confirmation_action(a, witness.as_ref()),
            ViewAction::UnifiedTagEditor(a) => self.handle_unified_tag_editor_action(a, witness.as_ref()),
            ViewAction::DeploymentPreview(a) => self.handle_deployment_preview_action(a, witness.as_ref()),
            ViewAction::MissingFileResolution(a) => self.handle_missing_file_preview_action(a, witness.as_ref()),
            ViewAction::MissingDirectoryResolution(a) => self.handle_missing_directory_preview_action(a, witness.as_ref()),
            ViewAction::CorruptFileResolution(a) => self.handle_corrupt_file_preview_action(a, witness.as_ref()),
            ViewAction::ShitFormatResolution(a) => self.handle_shit_format_preview_action(a, witness.as_ref()),
            ViewAction::EmbedAlbumArtResolution(a) => self.handle_embed_album_art_preview_action(a, witness.as_ref()),
            ViewAction::SubparDuplicateResolution(a) => self.handle_subpar_duplicate_preview_action(a, witness.as_ref()),
            ViewAction::DirectoryClusterResolution(a) => self.handle_directory_cluster_preview_action(a, witness.as_ref()),
            ViewAction::MovedFileAcknowledge(a) => self.handle_moved_file_action(a, witness.as_ref()),
            ViewAction::OobSyncResolution(a) => self.handle_oob_sync_action(a, witness.as_ref()),
            ViewAction::OobConflictInspection(a) => self.handle_oob_conflict_action(a, witness.as_ref()),
            ViewAction::TagCanonicityResolution(a) => self.handle_tag_canonicity_action(a, witness.as_ref()),
            ViewAction::CompoundTagSplit(a) => self.handle_compound_split_action(a, witness.as_ref()),
            ViewAction::TransactionReview(a) => self.handle_transaction_review_action(a, witness.as_ref()),
        }
    }

    // =========================================================================
    // Shared Helpers
    // =========================================================================

    /// Stage a tag editor decision to the Witch's transaction and update editor state.
    fn stage_tag_editor_decision(&mut self, index: usize, mutations: Vec<crate::meta::mutations::Mutation>, _witness: &witness::DecisionWitness) {
        if let Some(the_witch) = self.witch.as_mut() {
            let label = if let ActiveView::UnifiedTagEditor(ref editor) = self.view {
                editor.current_item_label()
            } else {
                "Tag edit".to_string()
            };
            let _ = super::operator_decisions::stage_decision(the_witch, index, &label, mutations.clone());
        }
        if let ActiveView::UnifiedTagEditor(ref mut editor) = self.view {
            editor.set_staged_mutations(mutations);
            editor.staged_decision_count += 1;
        }
        // Transaction summary in status_line_2 already reflects the staged state.
    }

    /// Stage mutations into a new transaction for review.
    ///
    /// Starts a transaction with the given label, stages the mutations as a
    /// single decision. Used by simple resolution modals that have a straightforward
    /// "collect mutations → review → commit" pattern.
    fn stage_mutations_with_transaction(&mut self, mutations: Vec<crate::meta::mutations::Mutation>, label: &str, _witness: &witness::DecisionWitness) {
        let Some(ref mut witch) = self.witch else { return };
        let _ = witch.start_transaction(label);
        let _ = super::operator_decisions::stage_decision(witch, 0, label, mutations);
    }

    /// Cancel the current modal: discard any active transaction and return to Insights.
    ///
    /// Logs the provided message, discards any open transaction, and navigates
    /// back to the Insights view. The old view is dropped when we set self.view.
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
        self.view = ActiveView::Progress { screen, eye: Eye::default() };
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
                    self.view = ActiveView::ExitConfirm(super::ExitConfirmModalState::default());
                }
            }
            insights_view::InsightsAction::CycleNext => {
                self.start_lateral_view(widgets::LateralView::Insights.next());
            }
            insights_view::InsightsAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::Insights.prev());
            }
            insights_view::InsightsAction::Launch => {
                // Use selected_action() to dispatch to appropriate modal
                let selected = if let ActiveView::Insights(ref v) = self.view {
                    v.selected_action()
                } else {
                    None
                };
                match selected {
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
                        let tag_name = if let ActiveView::Insights(ref v) = self.view {
                            v.selected_insight_type()
                                .and_then(|t| match t {
                                    insights_view::InsightType::CompoundTagValueSafe { tag_name } => Some(tag_name),
                                    _ => None,
                                })
                        } else {
                            None
                        };
                        self.start_compound_split_resolution(true, tag_name.as_deref());
                    }
                    Some(insights_view::InsightAction::LaunchCompoundTagSplitReview) => {
                        // Extract tag name from selected insight type
                        let tag_name = if let ActiveView::Insights(ref v) = self.view {
                            v.selected_insight_type()
                                .and_then(|t| match t {
                                    insights_view::InsightType::CompoundTagValueReview { tag_name } => Some(tag_name),
                                    _ => None,
                                })
                        } else {
                            None
                        };
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
                    Some(insights_view::InsightAction::LaunchEmbedAlbumArt) => {
                        self.start_embed_album_art_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchMissingDirectoryResolution) => {
                        self.start_missing_directory_resolution();
                    }
                    Some(insights_view::InsightAction::NotImplemented) => {
                        self.status_message = Some("Not yet implemented".to_string());
                    }
                    Some(insights_view::InsightAction::Informational) | None => {
                        // Informational entries have no action
                    }
                }
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
                self.view = ActiveView::IntakeConfirmation(state);
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
                self.start_insights_view();
            }
            tag_search::TagSearchAction::CycleNext => {
                self.start_lateral_view(widgets::LateralView::TagSearch.next());
            }
            tag_search::TagSearchAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::TagSearch.prev());
            }
            tag_search::TagSearchAction::ExecuteSearch => {
                // Execute search - access witch and view as disjoint fields
                if let (Some(ref mut witch), ActiveView::TagSearch(ref mut search)) =
                    (&mut self.witch, &mut self.view)
                {
                    let read_db = witch.read_db();
                    search.execute_search(&read_db);
                }
            }
            tag_search::TagSearchAction::EditAudioFile(audio_file) => {
                // Open unified tag editor for single audio file
                self.start_unified_tag_editor_for_audio_file(audio_file);
            }
        }
    }

    /// Handle intake confirmation dialog actions.
    fn handle_intake_confirmation_action(&mut self, action: startup::IntakeConfirmationAction, _witness: Option<&witness::DecisionWitness>) {
        use super::operator_decisions;

        match action {
            startup::IntakeConfirmationAction::None => {}
            startup::IntakeConfirmationAction::Confirmed => {
                // User confirmed - create IndexTrack mutations and stage for review
                let mutations = if let ActiveView::IntakeConfirmation(ref s) = self.view {
                    s.create_index_mutations()
                } else {
                    Vec::new()
                };

                if mutations.is_empty() {
                    // No files to index (all deleted since detection?) - skip to Insights
                    crate::logging::log_general("IntakeConfirmation: no mutations to queue, skipping to Insights");
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

                    // Note: IntakeConfirmation state is preserved inside the suspended view for Cancel return
                    // Transition to review modal with ContentAnalysis phase for post-commit
                    self.start_transaction_review_with_phase(
                        transaction_review::PostCommitPhase::ContentAnalysis,
                    );
                }
            }
            startup::IntakeConfirmationAction::Skipped => {
                // User skipped - no mutations ran, skip content analysis entirely
                // UnindexedFile signals remain for later handling
                crate::logging::log_general("IntakeConfirmation: user skipped indexing, going to Insights");

                // Discard any active transaction from review modal
                if let Some(ref mut witch) = self.witch {
                    if witch.has_transaction() {
                        let _ = operator_decisions::discard_transaction(witch);
                    }
                }
                self.start_insights_view();
            }
        }
    }

    pub(super) fn handle_tree_browser_action(&mut self, action: tree_browser::TreeBrowserAction) {
        match action {
            tree_browser::TreeBrowserAction::None => {}
            tree_browser::TreeBrowserAction::Cancel => {
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
                self.start_lateral_view(widgets::LateralView::CorpusBrowser.next());
            }
            tree_browser::TreeBrowserAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::CorpusBrowser.prev());
            }
            tree_browser::TreeBrowserAction::OpenFilter => {
                // Open filter popup for corpus browser
                self.filter_overlay = Some(FilterOverlay {
                    state: filter_popup::FilterPopupState::new(),
                    context: FilterPopupContext::CorpusBrowser,
                });
            }
        }
    }

    fn handle_unified_tag_editor_action(&mut self, action: tag_editor::UnifiedTagEditorAction, witness: Option<&witness::DecisionWitness>) {
        use tag_editor::UnifiedTagEditorAction;

        match action {
            UnifiedTagEditorAction::None => {}
            UnifiedTagEditorAction::CloseModal => {}

            UnifiedTagEditorAction::StageDecisionAndNavigate { index, mutations, direction } => {
                let Some(w) = witness else { return };
                // Stage the decision AND navigate (from confirmation modal Enter)
                use crate::ui::tag_editor::types::NavigationDirection;
                self.stage_tag_editor_decision(index, mutations, w);
                if let ActiveView::UnifiedTagEditor(ref mut editor) = self.view {
                    match direction {
                        NavigationDirection::Next => {
                            if editor.current_item_idx < editor.total_items.saturating_sub(1) {
                                editor.current_item_idx += 1;
                                editor.reset_field_state();
                            }
                        }
                        NavigationDirection::Prev => {
                            if editor.current_item_idx > 0 {
                                editor.current_item_idx -= 1;
                                editor.reset_field_state();
                            }
                        }
                    }
                }
            }

            UnifiedTagEditorAction::StageDecisionAndReview { index, mutations } => {
                let Some(w) = witness else { return };
                // Stage the decision AND immediately show transaction review
                // Used for aggregated mode or single-item contexts
                self.stage_tag_editor_decision(index, mutations, w);

                // Transition to standardized review modal
                // Note: unified_tag_editor state is preserved inside SuspendedView for Cancel return
                self.start_transaction_review();
            }

            UnifiedTagEditorAction::DiscardTransaction => {
                // Discard all staged decisions via sealed operator decision handler
                if let Some(the_witch) = self.witch.as_mut() {
                    let _ = super::operator_decisions::discard_transaction(the_witch);
                }
                self.start_insights_view();
                self.status_message = Some("Edits discarded".to_string());
            }

            UnifiedTagEditorAction::NextItem => {
                if let ActiveView::UnifiedTagEditor(ref mut editor) = self.view {
                    if editor.current_item_idx < editor.total_items.saturating_sub(1) {
                        editor.current_item_idx += 1;
                        editor.reset_field_state();
                    }
                }
            }

            UnifiedTagEditorAction::PrevItem => {
                if let ActiveView::UnifiedTagEditor(ref mut editor) = self.view {
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

                                if let ActiveView::UnifiedTagEditor(ref mut editor) = self.view {
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
                // Note: unified_tag_editor state is preserved inside SuspendedView for Cancel return
                self.start_transaction_review();
            }
        }
    }

    // ========================================================================
    // Standardized Transaction Review
    // ========================================================================

    /// Handle actions from the standardized transaction review modal.
    ///
    /// All mutation flows route through this review modal:
    /// - Cancel: return to source view (state preserved in SuspendedView)
    /// - Discard: discard transaction, drop suspended view, return to Insights
    /// - Confirm: commit transaction, drop suspended view, go to Progress
    fn handle_transaction_review_action(&mut self, action: transaction_review::TransactionReviewAction, _witness: Option<&witness::DecisionWitness>) {
        use transaction_review::TransactionReviewAction;

        match action {
            TransactionReviewAction::None => {}

            TransactionReviewAction::Cancel => {
                // Take the current view, extract suspended, restore it
                let old = std::mem::replace(&mut self.view, ActiveView::Insights(insights_view::InsightsViewState::new()));
                let ActiveView::TransactionReview { suspended, .. } = old else {
                    return;
                };
                match *suspended {
                    SuspendedView::Direct(view) => {
                        self.view = view;
                    }
                    SuspendedView::TagCanonicityReload { clusters } => {
                        if !self.load_current_cluster_signal_with_clusters(clusters) {
                            self.start_insights_view();
                        }
                    }
                    SuspendedView::CompoundTagSplitReload { clusters, safe_mode } => {
                        if !self.load_current_compound_split_signal_with_clusters(clusters, safe_mode) {
                            self.start_insights_view();
                        }
                    }
                }
            }

            TransactionReviewAction::Discard => {
                // Discard transaction and drop all suspended state
                if let Some(ref mut witch) = self.witch {
                    let _ = super::operator_decisions::discard_transaction(witch);
                }
                self.start_insights_view();
                self.status_message = Some("Transaction discarded".to_string());
            }

            TransactionReviewAction::Confirm => {
                // Determine progress phase before clearing state
                let post_commit_phase = if let ActiveView::TransactionReview { ref review, .. } = self.view {
                    review.post_commit_phase
                } else {
                    transaction_review::PostCommitPhase::default()
                };

                // Commit transaction - dropping the old view drops all suspended state
                let commit_result = if let Some(ref mut witch) = self.witch {
                    super::operator_decisions::commit_transaction(witch)
                } else {
                    Err(crate::witch::TransactionError::NoActiveTransaction)
                };

                match commit_result {
                    Ok(_summary) => {
                        // Don't set status_message here — it would suppress the
                        // selected-path display in status_line_1, and the commit
                        // outcome is already evident from the progress screen.

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

    /// Transition to the standardized transaction review modal.
    ///
    /// Called after staging decisions to show the review before commit.
    /// Takes ownership of the current view and wraps it as a SuspendedView.
    /// Cancel navigation restores the suspended view automatically.
    pub(in crate::ui) fn start_transaction_review(&mut self) {
        let suspended = self.suspend_current_view();
        self.view = ActiveView::TransactionReview {
            review: transaction_review::TransactionReviewState::new(),
            suspended: Box::new(suspended),
        };
    }

    /// Transition to transaction review modal with custom post-commit phase.
    ///
    /// Used for intake indexing which needs ContentAnalysis instead of SignalRefresh.
    pub(super) fn start_transaction_review_with_phase(
        &mut self,
        phase: transaction_review::PostCommitPhase,
    ) {
        let suspended = self.suspend_current_view();
        self.view = ActiveView::TransactionReview {
            review: transaction_review::TransactionReviewState::new()
                .with_post_commit_phase(phase),
            suspended: Box::new(suspended),
        };
    }

    /// Take the current view and wrap it as a SuspendedView for later restoration.
    ///
    /// TagCanonicityResolution and CompoundTagSplit need DB reload on restore,
    /// so they get special SuspendedView variants. Everything else restores directly.
    fn suspend_current_view(&mut self) -> SuspendedView {
        let old_view = std::mem::replace(&mut self.view, ActiveView::Insights(insights_view::InsightsViewState::new()));
        match old_view {
            ActiveView::TagCanonicityResolution { clusters, .. } => {
                SuspendedView::TagCanonicityReload { clusters }
            }
            ActiveView::CompoundTagSplit { clusters, safe_mode, .. } => {
                SuspendedView::CompoundTagSplitReload { clusters, safe_mode }
            }
            view => SuspendedView::Direct(view),
        }
    }

    // =========================================================================
    // Mouse Click Handling
    // =========================================================================

    /// Handle mouse click at the given position.
    ///
    /// This dispatches to the current view's click handler to check for
    /// button hits. Mouse clicks on decision buttons are equivalent to
    /// Enter key presses for decision witnessing - clicks always have authority.
    pub(super) fn handle_click(&mut self, x: u16, y: u16) {
        let click_witness = witness::DecisionWitness::new();

        match &self.view {
            ActiveView::OobSyncResolution(state) => {
                if let Some(button_name) = state.button_rects.hit_test(x, y) {
                    // Simulate the button press action
                    let action = match button_name {
                        "accept_disk" => oob_sync_modal::OobSyncAction::AcceptDisk,
                        "accept_db" => oob_sync_modal::OobSyncAction::AcceptDb,
                        "cancel" => oob_sync_modal::OobSyncAction::Cancel,
                        _ => oob_sync_modal::OobSyncAction::None,
                    };
                    self.handle_oob_sync_action(action, Some(&click_witness));
                }
            }
            ActiveView::OobConflictInspection(state) => {
                if let Some(button_name) = state.button_rects.hit_test(x, y) {
                    // Determine action and button state from button name
                    let action = match button_name {
                        "apply_db" => oob_conflict_modal::OobConflictAction::Resolve,
                        "assimilate_disk" => oob_conflict_modal::OobConflictAction::Resolve,
                        "acknowledge" => oob_conflict_modal::OobConflictAction::Acknowledge,
                        "cancel" => oob_conflict_modal::OobConflictAction::Cancel,
                        _ => oob_conflict_modal::OobConflictAction::None,
                    };
                    // Set button before handling (need mutable access)
                    if button_name == "apply_db" {
                        if let ActiveView::OobConflictInspection(ref mut state) = self.view {
                            state.selected_button = oob_conflict_modal::types::ResolutionButton::ApplyDb;
                        }
                    } else if button_name == "assimilate_disk" {
                        if let ActiveView::OobConflictInspection(ref mut state) = self.view {
                            state.selected_button = oob_conflict_modal::types::ResolutionButton::AssimilateDisk;
                        }
                    }
                    self.handle_oob_conflict_action(action, Some(&click_witness));
                }
            }
            ActiveView::Insights(_) => {
                if let ActiveView::Insights(ref mut view) = self.view {
                    view.handle_click(x, y);
                }
            }
            // Other views don't handle clicks
            _ => {}
        }
    }
}
