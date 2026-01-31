//! Action Handlers for View-Specific Events
//!
//! Each view (insights, tag search, tree browser, tag editor, etc.) emits
//! Actions that are handled here. These handlers coordinate state transitions,
//! Witch interactions, and modal displays.

use crate::corpus::paths;
use crate::ui::{compound_split, corrupt_file_flow, filter_popup, format_standardization, inode_changed_flow, insights_view, missing_file_flow, oob_sync_flow, oob_conflict_flow, progress_screen, shit_format_flow, tag_canonicity, tag_search, transaction_review, tree_browser, tag_editor, deploy_flow, startup, widgets, FilterPopupContext};
use crate::ui::types::{UiMode, ExitConfirmModalState};
use super::App;

impl App {
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
                    Some(insights_view::InsightAction::LaunchCompoundTagSplit) => {
                        self.start_compound_split_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchOobTagSync) => {
                        self.start_oob_sync_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchOobTagConflict) => {
                        self.start_oob_conflict_inspection();
                    }
                    Some(insights_view::InsightAction::LaunchInodeChangedAcknowledge) => {
                        self.start_inode_changed_acknowledge();
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
                    Some(insights_view::InsightAction::LaunchFingerprintDuplicateResolution) => {
                        self.status_message = Some("Fingerprint duplicate flow not yet implemented".to_string());
                    }
                    Some(insights_view::InsightAction::LaunchInferiorDuplicateResolution) => {
                        self.status_message = Some("Inferior duplicate flow not yet implemented".to_string());
                    }
                    Some(insights_view::InsightAction::NotImplemented) => {
                        self.status_message = Some("Flow not yet implemented".to_string());
                    }
                    Some(insights_view::InsightAction::Informational) | None => {
                        // Informational entries have no action
                    }
                }
            }
        }
    }

    /// Start deployment preview from Insights view.
    fn start_deployment_preview_from_insights(&mut self) {
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                deploy_flow::DeployModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        let preview = deploy_flow::DeploymentPreviewState::new(data);
        self.deployment_preview = Some(preview);
        self.mode = UiMode::DeploymentPreview;
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
            tag_search::TagSearchAction::EditTrack(track) => {
                // Open unified tag editor for single track
                self.tag_search = None;
                self.start_unified_tag_editor_for_track(track);
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

            UnifiedTagEditorAction::RequestFillFromDb { track_id } => {
                match track_id {
                    Some(id) => {
                        let read_db = self.read_db();
                        match read_db.get_track_tags(id) {
                            Ok(tags) => {
                                // Convert TrackTag to (name, value) pairs
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

    pub(super) fn handle_deployment_preview_action(&mut self, action: deploy_flow::DeploymentPreviewAction) {
        match action {
            deploy_flow::DeploymentPreviewAction::None => {}
            deploy_flow::DeploymentPreviewAction::Confirm => {
                // Generate deploy mutations and stage for review
                // Clone the cached data to avoid borrow issues
                let cached_data = self.deployment_preview.as_ref()
                    .map(|p| p.cached_data.clone());
                if let Some(data) = cached_data {
                    let mutation_count = self.stage_deploy_mutations(&data);
                    if mutation_count > 0 {
                        // Note: deployment_preview state is NOT cleared - preserved for Cancel return
                        self.start_transaction_review(transaction_review::TransactionReviewSource::DeployPreview);
                    } else {
                        // No mutations (edge case) - go directly to Insights
                        self.deployment_preview = None;
                        self.start_insights_view();
                        self.status_message = Some("No deploy operations needed".to_string());
                    }
                } else {
                    self.deployment_preview = None;
                    self.start_insights_view();
                }
            }
            deploy_flow::DeploymentPreviewAction::Cancel => {
                crate::logging::log_general("Deployment preview cancelled");
                // Discard any active transaction from review flow
                if let Some(ref mut witch) = self.witch {
                    if witch.has_transaction() {
                        let _ = super::operator_decisions::discard_transaction(witch);
                    }
                }
                self.deployment_preview = None;
                self.start_insights_view();
                self.status_message = Some("Deployment cancelled".to_string());
            }
        }
    }

    /// Stage deploy mutations for transaction review.
    ///
    /// Returns the number of mutations staged.
    ///
    /// Paths in the signal data are relative to their roots:
    /// - corpus_path: relative to corpus_root
    /// - deploy_path/library_path/expected_path: relative to libraries_root
    /// These must be resolved to absolute for filesystem mutations.
    fn stage_deploy_mutations(&mut self, data: &deploy_flow::DeployModalData) -> usize {
        use crate::corpus::mutations::Mutation;

        let Some(ref mut witch) = self.witch else {
            return 0;
        };
        let resolver = paths::get_resolver();
        let mut mutations = Vec::new();

        // New files: create hard links
        for file in &data.new {
            // Skip files with empty deploy_path (data integrity check)
            if file.deploy_path.is_empty() {
                continue;
            }
            // Resolve relative paths to absolute
            let source = resolver.resolve(std::path::Path::new(&file.corpus_path));
            let destination = resolver.resolve(std::path::Path::new(&file.deploy_path));
            mutations.push(Mutation::HardLink { source, destination });
        }

        // Stale files: move from wrong path to correct path
        for file in &data.stale {
            let source = resolver.resolve(std::path::Path::new(&file.library_path));
            let destination = resolver.resolve(std::path::Path::new(&file.expected_path));
            mutations.push(Mutation::LibraryMove { source, destination });
        }

        // Leftover files: move to stash (preserve data, never destroy)
        for file in &data.leftover {
            let path = resolver.resolve(std::path::Path::new(&file.library_path));
            mutations.push(Mutation::MoveToStash {
                path,
                stash_name: "library_leftovers".to_string(),
            });
        }

        // Conflicts: pick first alphabetical corpus path and deploy it
        for group in &data.conflicts {
            if let Some((corpus_path, _track_id)) = group
                .conflicting_files
                .iter()
                .min_by(|a, b| a.0.cmp(&b.0))
            {
                let source = resolver.resolve(std::path::Path::new(corpus_path));
                let destination = resolver.resolve(std::path::Path::new(&group.deploy_path));
                mutations.push(Mutation::HardLink { source, destination });
            }
        }

        let count = mutations.len();
        if count == 0 {
            return 0;
        }

        // Start transaction and stage the decision
        let _ = witch.start_transaction("Deploy");
        let _ = super::operator_decisions::stage_decision(
            witch,
            0,
            "Deploy operations",
            mutations,
        );

        count
    }

    // =========================================================================
    // Missing File Resolution
    // =========================================================================

    /// Start missing file resolution modal from Insights view.
    fn start_missing_file_resolution(&mut self) {
        // Load categorized missing file data
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                missing_file_flow::MissingFileModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.total_count() == 0 {
            self.status_message = Some("No missing files to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = missing_file_flow::MissingFilePreviewState::new(data);
        self.missing_file_preview = Some(preview);
        self.mode = UiMode::MissingFileResolution;
    }

    /// Handle missing file preview actions.
    pub(super) fn handle_missing_file_preview_action(&mut self, action: missing_file_flow::MissingFilePreviewAction) {
        match action {
            missing_file_flow::MissingFilePreviewAction::None => {}
            missing_file_flow::MissingFilePreviewAction::ConfirmRestore => {
                // Generate restore mutations (HardLink) and stage for review
                if let Some(ref preview) = self.missing_file_preview {
                    let mutations = preview.cached_data.restore_mutations();
                    let count = mutations.len();
                    if count > 0 {
                        self.stage_missing_file_mutations(mutations, "Restore missing files");
                        // Note: missing_file_preview state is NOT cleared - preserved for Cancel return
                        self.start_transaction_review(transaction_review::TransactionReviewSource::MissingFileResolution);
                    } else {
                        self.status_message = Some("No files to restore".to_string());
                    }
                }
            }
            missing_file_flow::MissingFilePreviewAction::ConfirmDrop => {
                // Generate drop mutations (DropFromIndex) and stage for review
                if let Some(ref preview) = self.missing_file_preview {
                    let mutations = preview.cached_data.drop_mutations();
                    let count = mutations.len();
                    if count > 0 {
                        self.stage_missing_file_mutations(mutations, "Drop non-restorable files");
                        // Note: missing_file_preview state is NOT cleared - preserved for Cancel return
                        self.start_transaction_review(transaction_review::TransactionReviewSource::MissingFileResolution);
                    } else {
                        self.status_message = Some("No files to drop".to_string());
                    }
                }
            }
            missing_file_flow::MissingFilePreviewAction::Cancel => {
                crate::logging::log_general("Missing file resolution cancelled");
                // Discard any active transaction from review flow
                if let Some(ref mut witch) = self.witch {
                    if witch.has_transaction() {
                        let _ = super::operator_decisions::discard_transaction(witch);
                    }
                }
                self.missing_file_preview = None;
                self.start_insights_view();
            }
        }
    }

    /// Stage missing file mutations for transaction review.
    fn stage_missing_file_mutations(&mut self, mutations: Vec<crate::corpus::mutations::Mutation>, label: &str) {
        let Some(ref mut witch) = self.witch else {
            return;
        };

        // Start transaction and stage the decision
        let _ = witch.start_transaction(label);
        let _ = super::operator_decisions::stage_decision(
            witch,
            0,
            label,
            mutations,
        );
    }

    // ========================================================================
    // Corrupt File Resolution
    // ========================================================================

    /// Start corrupt file resolution modal from Insights view.
    fn start_corrupt_file_resolution(&mut self) {
        // Load corrupt file data
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                corrupt_file_flow::CorruptFileModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.total_count() == 0 {
            self.status_message = Some("No corrupt files to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = corrupt_file_flow::CorruptFilePreviewState::new(data);
        self.corrupt_file_preview = Some(preview);
        self.mode = UiMode::CorruptFileResolution;
    }

    /// Handle corrupt file preview actions.
    pub(super) fn handle_corrupt_file_preview_action(&mut self, action: corrupt_file_flow::CorruptFilePreviewAction) {
        match action {
            corrupt_file_flow::CorruptFilePreviewAction::None => {}
            corrupt_file_flow::CorruptFilePreviewAction::ConfirmStashAll => {
                // Generate stash + drop mutations and stage for review
                if let Some(ref preview) = self.corrupt_file_preview {
                    let mutations = preview.cached_data.stash_and_drop_mutations();
                    let count = mutations.len();
                    if count > 0 {
                        self.stage_corrupt_file_mutations(mutations, "Stash corrupt files");
                        // Note: corrupt_file_preview state is NOT cleared - preserved for Cancel return
                        self.start_transaction_review(transaction_review::TransactionReviewSource::CorruptFileResolution);
                    } else {
                        self.status_message = Some("No files to stash".to_string());
                    }
                }
            }
            corrupt_file_flow::CorruptFilePreviewAction::Cancel => {
                crate::logging::log_general("Corrupt file resolution cancelled");
                // Discard any active transaction from review flow
                if let Some(ref mut witch) = self.witch {
                    if witch.has_transaction() {
                        let _ = super::operator_decisions::discard_transaction(witch);
                    }
                }
                self.corrupt_file_preview = None;
                self.start_insights_view();
            }
        }
    }

    /// Stage corrupt file mutations for transaction review.
    fn stage_corrupt_file_mutations(&mut self, mutations: Vec<crate::corpus::mutations::Mutation>, label: &str) {
        let Some(ref mut witch) = self.witch else {
            return;
        };

        // Start transaction and stage the decision
        let _ = witch.start_transaction(label);
        let _ = super::operator_decisions::stage_decision(
            witch,
            0,
            label,
            mutations,
        );
    }

    // ========================================================================
    // Shit Format Resolution
    // ========================================================================

    /// Start shit format resolution modal from Insights view.
    fn start_shit_format_resolution(&mut self) {
        // Load shit format file data
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                shit_format_flow::ShitFormatModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        if data.total_count() == 0 {
            self.status_message = Some("No shit format files to resolve".to_string());
            return;
        }

        // Create preview state with cached data
        let preview = shit_format_flow::ShitFormatPreviewState::new(data);
        self.shit_format_preview = Some(preview);
        self.mode = UiMode::ShitFormatResolution;
    }

    /// Handle shit format preview actions.
    pub(super) fn handle_shit_format_preview_action(&mut self, action: shit_format_flow::ShitFormatPreviewAction) {
        match action {
            shit_format_flow::ShitFormatPreviewAction::None => {}
            shit_format_flow::ShitFormatPreviewAction::ConfirmRemuxLossless => {
                // Generate FLAC remux mutations for lossless files only
                if let Some(ref preview) = self.shit_format_preview {
                    let mutations = preview.cached_data.lossless_mutations();
                    if !mutations.is_empty() {
                        self.stage_shit_format_mutations(mutations, "Remux to FLAC");
                        self.start_transaction_review(transaction_review::TransactionReviewSource::ShitFormatResolution);
                    } else {
                        self.status_message = Some("No lossless files to remux".to_string());
                    }
                }
            }
            shit_format_flow::ShitFormatPreviewAction::ConfirmTranscodeLossy => {
                // Generate Opus transcode mutations for lossy files only
                if let Some(ref preview) = self.shit_format_preview {
                    let mutations = preview.cached_data.lossy_mutations();
                    if !mutations.is_empty() {
                        self.stage_shit_format_mutations(mutations, "Transcode to Opus");
                        self.start_transaction_review(transaction_review::TransactionReviewSource::ShitFormatResolution);
                    } else {
                        self.status_message = Some("No lossy files to transcode".to_string());
                    }
                }
            }
            shit_format_flow::ShitFormatPreviewAction::ConfirmConvertAll => {
                // Generate mutations for all files (lossless → FLAC, lossy → Opus)
                if let Some(ref preview) = self.shit_format_preview {
                    let mutations = preview.cached_data.all_mutations();
                    if !mutations.is_empty() {
                        self.stage_shit_format_mutations(mutations, "Convert all formats");
                        self.start_transaction_review(transaction_review::TransactionReviewSource::ShitFormatResolution);
                    } else {
                        self.status_message = Some("No files to convert".to_string());
                    }
                }
            }
            shit_format_flow::ShitFormatPreviewAction::Cancel => {
                crate::logging::log_general("Shit format resolution cancelled");
                // Discard any active transaction from review flow
                if let Some(ref mut witch) = self.witch {
                    if witch.has_transaction() {
                        let _ = super::operator_decisions::discard_transaction(witch);
                    }
                }
                self.shit_format_preview = None;
                self.start_insights_view();
            }
        }
    }

    /// Stage shit format mutations for transaction review.
    fn stage_shit_format_mutations(&mut self, mutations: Vec<crate::corpus::mutations::Mutation>, label: &str) {
        let Some(ref mut witch) = self.witch else {
            return;
        };

        // Start transaction and stage the decision
        let _ = witch.start_transaction(label);
        let _ = super::operator_decisions::stage_decision(
            witch,
            0,
            label,
            mutations,
        );
    }

    // ========================================================================
    // Tag Canonicity Resolution
    // ========================================================================

    /// Start tag canonicity resolution from Insights view.
    ///
    /// Uses the selected insight type to determine which signals to load:
    /// - InconsistentAlbumArtist: loads all inconsistent_album_artist signals
    /// - TagCanonicity { tag_name }: loads tag_canonicity signals filtered by tag_name
    fn start_tag_canonicity_resolution(&mut self) {
        use crate::corpus::db::types::AggregateSignalType;

        // Get the selected insight type to determine what to load
        let insight_type = match self.insights_view.as_ref().and_then(|v| v.selected_insight_type()) {
            Some(t) => t,
            None => {
                self.status_message = Some("No insight selected".to_string());
                return;
            }
        };

        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => {
                self.status_message = Some("Database not available".to_string());
                return;
            }
        };

        // Load signals based on insight type
        let (signals, pre_fill) = match &insight_type {
            insights_view::InsightType::InconsistentAlbumArtist => {
                let sigs = read_db.get_aggregate_signals(Some(AggregateSignalType::InconsistentAlbumArtist))
                    .unwrap_or_default();
                (sigs, false) // No pre-fill for album_artist
            }
            insights_view::InsightType::TagCanonicity { tag_name } => {
                // Load all TagCanonicity signals, then filter by tag_name prefix
                let all_sigs = read_db.get_aggregate_signals(Some(AggregateSignalType::TagCanonicity))
                    .unwrap_or_default();
                let filtered: Vec<_> = all_sigs.into_iter()
                    .filter(|s| s.key.starts_with(&format!("{}:", tag_name)))
                    .collect();
                (filtered, true) // Pre-fill for tag canonicity
            }
            _ => {
                self.status_message = Some("Invalid insight type for tag resolution".to_string());
                return;
            }
        };

        if signals.is_empty() {
            self.status_message = Some("No signals to resolve".to_string());
            return;
        }

        // Store signal IDs for cluster navigation
        let signal_ids: Vec<i64> = signals.iter().filter_map(|s| s.id).collect();
        self.tag_canonicity_clusters = Some(super::TagCanonicityClusters::new(signal_ids));

        // Start transaction ONCE for entire flow
        if let Some(ref mut witch) = self.witch {
            let _ = witch.start_transaction("Tag canonicalization");
        }

        // Load the first signal into modal data
        let first_signal = &signals[0];
        let data = match tag_canonicity::TagCanonicalityModalData::from_signal(first_signal) {
            Some(d) => d,
            None => {
                self.status_message = Some("Failed to parse signal data".to_string());
                self.tag_canonicity_clusters = None;
                // Discard the transaction we just started via sealed operator decision handler
                if let Some(ref mut witch) = self.witch {
                    let _ = super::operator_decisions::discard_transaction(witch);
                }
                return;
            }
        };

        // Get group info from clusters (just set above)
        let (group_index, total_groups) = self.tag_canonicity_clusters
            .as_ref()
            .map(|c| (c.current_index, c.signal_ids.len()))
            .unwrap_or((0, 1));

        let state = tag_canonicity::TagCanonicalityState::new(data, pre_fill, group_index, total_groups);
        self.tag_canonicity_state = Some(state);
        self.mode = UiMode::TagCanonicityResolution;
    }

    /// Start compound tag split resolution flow.
    ///
    /// Loads CompoundTagValue signals and enters the split modal.
    fn start_compound_split_resolution(&mut self) {
        use crate::corpus::db::types::AggregateSignalType;

        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => {
                self.status_message = Some("Database not available".to_string());
                return;
            }
        };

        // Load all CompoundTagValue signals
        let signals = read_db.get_aggregate_signals(Some(AggregateSignalType::CompoundTagValue))
            .unwrap_or_default();

        if signals.is_empty() {
            self.status_message = Some("No compound tag values to split".to_string());
            return;
        }

        // Store signal IDs for cluster navigation
        let signal_ids: Vec<i64> = signals.iter().filter_map(|s| s.id).collect();
        self.compound_split_clusters = Some(compound_split::CompoundSplitClusters::new(signal_ids));

        // Start transaction ONCE for entire flow
        if let Some(ref mut witch) = self.witch {
            let _ = witch.start_transaction("Compound tag split");
        }

        // Load the first signal into modal data
        let first_signal = &signals[0];
        let data = match compound_split::CompoundSplitData::from_signal(first_signal) {
            Some(d) => d,
            None => {
                self.status_message = Some("Failed to parse signal data".to_string());
                self.compound_split_clusters = None;
                // Discard the transaction we just started
                if let Some(ref mut witch) = self.witch {
                    let _ = super::operator_decisions::discard_transaction(witch);
                }
                return;
            }
        };

        // Get group info from clusters
        let (group_index, total_groups) = self.compound_split_clusters
            .as_ref()
            .map(|c| (c.current_index(), c.total()))
            .unwrap_or((0, 1));

        let state = compound_split::CompoundSplitState::new(data, group_index, total_groups);
        self.compound_split_state = Some(state);
        self.mode = UiMode::CompoundTagSplit;
    }

    /// Start OOB tag sync resolution from Insights view.
    fn start_oob_sync_resolution(&mut self) {
        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => {
                self.status_message = Some("Database not available".to_string());
                return;
            }
        };

        let files = read_db.get_oob_sync_files().unwrap_or_default();
        if files.is_empty() {
            self.status_message = Some("No syncable tag changes".to_string());
            return;
        }

        // Start transaction for the sync resolution
        if let Some(ref mut witch) = self.witch {
            let _ = witch.start_transaction("OOB tag sync");
        }

        let state = oob_sync_flow::OobSyncState::new(files);
        self.oob_sync_state = Some(state);
        self.mode = UiMode::OobSyncResolution;
    }

    /// Start OOB tag conflict inspection from Insights view.
    ///
    /// Loads all OOB signal files classified into four buckets, starts a
    /// transaction for potential resolution, and computes the initial diff.
    fn start_oob_conflict_inspection(&mut self) {
        // First pass: query bucketed files (scoped borrow)
        let files = {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => {
                    self.status_message = Some("Database not available".to_string());
                    return;
                }
            };
            match read_db.get_oob_files_bucketed() {
                Ok(f) => f,
                Err(e) => {
                    crate::logging::log_error(format!("get_oob_files_bucketed failed: {}", e));
                    self.status_message = Some(format!("Query failed: {}", e));
                    return;
                }
            }
        };

        if files.is_empty() {
            self.status_message = Some("No OOB tag signals to inspect".to_string());
            return;
        }

        // Start transaction for potential resolution
        if let Some(ref mut witch) = self.witch {
            let _ = witch.start_transaction("OOB tag resolution");
        }

        let mut state = oob_conflict_flow::OobConflictState::new(files);

        // Second pass: compute initial diff for first file in the active bucket
        if let Some(file) = state.active_bucket_state().current_file() {
            let track_id = file.track_id;
            let path = file.path.clone();
            if let Some(w) = self.witch.as_mut() {
                let read_db = w.read_db();
                let resolver = paths::get_resolver();
                let abs_path = resolver.resolve(std::path::Path::new(&path));
                state.current_diff = oob_conflict_flow::types::compute_tag_diff(&read_db, track_id, &abs_path);
            }
        }

        self.oob_conflict_state = Some(state);
        self.mode = UiMode::OobConflictInspection;
    }

    /// Start inode changed acknowledgement flow.
    fn start_inode_changed_acknowledge(&mut self) {
        let Some(ref mut witch) = self.witch else {
            self.status_message = Some("No database connection".to_string());
            return;
        };

        // Query files with inode_changed signals
        let files = {
            let read_db = witch.read_db();
            match read_db.get_inode_changed_files() {
                Ok(f) => f,
                Err(e) => {
                    self.status_message = Some(format!("Failed to query inode changes: {}", e));
                    return;
                }
            }
        };

        if files.is_empty() {
            self.status_message = Some("No inode-changed files to acknowledge".to_string());
            return;
        }

        crate::logging::log_general(format!(
            "Starting inode changed acknowledgement: {} files",
            files.len()
        ));

        // Start transaction for the acknowledgement
        let _ = witch.start_transaction("Inode changed acknowledgement");

        self.inode_changed_state = Some(inode_changed_flow::InodeChangedState::new(files));
        self.mode = UiMode::InodeChangedAcknowledge;
    }

    /// Handle inode changed acknowledgement actions.
    pub(super) fn handle_inode_changed_action(&mut self, action: inode_changed_flow::InodeChangedAction) {
        match action {
            inode_changed_flow::InodeChangedAction::None => {}
            inode_changed_flow::InodeChangedAction::Acknowledge => {
                self.stage_inode_changed_acknowledge();
                // Transition to review (no source variant needed - just cancel goes to insights)
                self.start_transaction_review(transaction_review::TransactionReviewSource::OobConflictResolution);
            }
            inode_changed_flow::InodeChangedAction::Cancel => {
                crate::logging::log_general("Inode changed acknowledgement cancelled");
                // Discard any active transaction
                if let Some(ref mut witch) = self.witch {
                    if witch.has_transaction() {
                        let _ = super::operator_decisions::discard_transaction(witch);
                    }
                }
                self.inode_changed_state = None;
                self.start_insights_view();
            }
        }
    }

    /// Stage mutations for inode changed acknowledgement.
    fn stage_inode_changed_acknowledge(&mut self) {
        use crate::corpus::mutations::Mutation;

        let Some(ref state) = self.inode_changed_state else {
            return;
        };

        if state.files.is_empty() {
            return;
        }

        let tracks = state.tracks_with_paths();
        let label = format!(
            "Acknowledge {} inode change{}",
            tracks.len(),
            if tracks.len() == 1 { "" } else { "s" }
        );
        let mutations = vec![Mutation::AcknowledgeInodeChanged { tracks }];

        // Stage the AcknowledgeInodeChanged mutation
        if let Some(ref mut witch) = self.witch {
            let _ = super::operator_decisions::stage_decision(witch, 0, &label, mutations);
        }
    }

    /// Handle OOB sync resolution actions.
    pub(super) fn handle_oob_sync_action(&mut self, action: oob_sync_flow::OobSyncAction) {
        match action {
            oob_sync_flow::OobSyncAction::None => {}
            oob_sync_flow::OobSyncAction::AcceptDisk => {
                self.stage_oob_sync_mutations(crate::corpus::db::types::OobSyncDirection::DiskToIndex);
                // Transition to review
                self.start_transaction_review(transaction_review::TransactionReviewSource::OobSyncResolution);
            }
            oob_sync_flow::OobSyncAction::AcceptDb => {
                self.stage_oob_sync_mutations(crate::corpus::db::types::OobSyncDirection::IndexToDisk);
                // Transition to review
                self.start_transaction_review(transaction_review::TransactionReviewSource::OobSyncResolution);
            }
            oob_sync_flow::OobSyncAction::Cancel => {
                crate::logging::log_general("OOB sync resolution cancelled");
                // Discard any active transaction
                if let Some(ref mut witch) = self.witch {
                    if witch.has_transaction() {
                        let _ = super::operator_decisions::discard_transaction(witch);
                    }
                }
                self.oob_sync_state = None;
                self.start_insights_view();
            }
            oob_sync_flow::OobSyncAction::OpenFilter => {
                // Open filter popup overlay
                self.filter_popup_state = Some(filter_popup::FilterPopupState::new());
                self.filter_popup_context = Some(FilterPopupContext::OobSync);
            }
        }
    }

    /// Stage mutations for OOB tag sync (accept one direction).
    ///
    /// If selection is active, only selected files are included.
    /// Otherwise, all files matching the direction are included.
    ///
    /// Uses the dedicated batch mutations which properly handle multi-value tags:
    /// - IndexToDisk: ApplyDbTagsToDisk (writes DB tags to disk files)
    /// - DiskToIndex: AssimilateDiskTagsToDb (reads disk tags into DB index)
    fn stage_oob_sync_mutations(&mut self, direction: crate::corpus::db::types::OobSyncDirection) {
        use crate::corpus::db::types::OobSyncDirection;
        use crate::corpus::mutations::Mutation;

        let Some(ref state) = self.oob_sync_state else {
            return;
        };

        let resolver = paths::get_resolver();

        // Determine which indices to process
        let selected_indices = if state.selection.is_active() {
            state.selection.selected_indices()
        } else {
            // No selection - process all files matching direction
            (0..state.files.len()).collect()
        };

        // Collect tracks matching the direction
        let tracks: Vec<(i64, std::path::PathBuf)> = selected_indices
            .iter()
            .filter_map(|&idx| state.files.get(idx))
            .filter(|file| file.direction == direction)
            .map(|file| {
                let abs_path = resolver.resolve(std::path::Path::new(&file.path));
                (file.track_id, abs_path)
            })
            .collect();

        if tracks.is_empty() {
            self.status_message = Some("No files to sync".to_string());
            return;
        }

        // Generate individual single-track mutations for each track
        let (label, mutations): (&str, Vec<Mutation>) = match direction {
            OobSyncDirection::IndexToDisk => (
                "Sync index tags → disk",
                tracks.into_iter()
                    .map(|(track_id, path)| Mutation::ApplyDbTagsToDisk { track_id, path })
                    .collect(),
            ),
            OobSyncDirection::DiskToIndex => (
                "Sync disk tags → index",
                tracks.into_iter()
                    .map(|(track_id, path)| Mutation::AssimilateDiskTagsToDb { track_id, path })
                    .collect(),
            ),
        };

        if let Some(ref mut witch) = self.witch {
            let _ = super::operator_decisions::stage_decision(witch, 0, label, mutations);
        }
    }

    /// Handle OOB conflict inspection actions.
    pub(super) fn handle_oob_conflict_action(&mut self, action: oob_conflict_flow::OobConflictAction) {
        match action {
            oob_conflict_flow::OobConflictAction::None => {}
            oob_conflict_flow::OobConflictAction::Navigate => {
                // File or bucket selection changed — recompute diff for the new file
                let diff = self.compute_current_conflict_diff();
                if let Some(ref mut state) = self.oob_conflict_state {
                    state.current_diff = diff;
                }
            }
            oob_conflict_flow::OobConflictAction::Resolve => {
                self.stage_oob_bucket_resolution();
            }
            oob_conflict_flow::OobConflictAction::Acknowledge => {
                self.stage_oob_mtime_acknowledgement();
            }
            oob_conflict_flow::OobConflictAction::Cancel => {
                crate::logging::log_general("OOB conflict inspection closed");
                // Discard any active transaction
                if let Some(ref mut witch) = self.witch {
                    if witch.has_transaction() {
                        let _ = super::operator_decisions::discard_transaction(witch);
                    }
                }
                self.oob_conflict_state = None;
                self.start_insights_view();
            }
            oob_conflict_flow::OobConflictAction::OpenFilter => {
                // Open filter popup overlay
                self.filter_popup_state = Some(filter_popup::FilterPopupState::new());
                self.filter_popup_context = Some(FilterPopupContext::OobConflict);
            }
        }
    }

    /// Compute the tag diff for the currently selected conflict file.
    fn compute_current_conflict_diff(&mut self) -> Vec<crate::corpus::db::types::TagMismatchEntry> {
        let (track_id, path) = match self.oob_conflict_state.as_ref()
            .and_then(|s| s.active_bucket_state().current_file())
        {
            Some(file) => (file.track_id, file.path.clone()),
            None => return Vec::new(),
        };

        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => return Vec::new(),
        };

        let resolver = paths::get_resolver();
        let abs_path = resolver.resolve(std::path::Path::new(&path));
        oob_conflict_flow::types::compute_tag_diff(&read_db, track_id, &abs_path)
    }

    /// Stage resolution mutations for files in the active bucket.
    ///
    /// If selection is active, only selected files are included.
    /// Otherwise, all files in the bucket are included.
    ///
    /// Uses the dedicated batch mutations which properly handle multi-value tags:
    /// - ApplyDbTagsToDisk: writes DB tags to disk files
    /// - AssimilateDiskTagsToDb: reads disk tags into DB index
    fn stage_oob_bucket_resolution(&mut self) {
        use crate::corpus::mutations::Mutation;
        use crate::ui::oob_conflict_flow::types::ResolutionButton;

        let (files_data, button) = match self.oob_conflict_state.as_ref() {
            Some(state) => {
                let bucket_state = state.active_bucket_state();

                // Determine which indices to process
                let indices: Vec<usize> = if bucket_state.selection.is_active() {
                    bucket_state.selection.selected_indices()
                } else {
                    // No selection - process all files in bucket
                    (0..bucket_state.files.len()).collect()
                };

                let files: Vec<(i64, String)> = indices
                    .iter()
                    .filter_map(|&idx| bucket_state.files.get(idx))
                    .map(|f| (f.track_id, f.path.clone()))
                    .collect();
                (files, state.selected_button)
            }
            None => return,
        };

        if files_data.is_empty() {
            self.status_message = Some("No files selected to resolve".to_string());
            return;
        }

        let resolver = paths::get_resolver();

        // Convert to (track_id, abs_path) pairs for individual mutations
        let tracks: Vec<(i64, std::path::PathBuf)> = files_data
            .iter()
            .map(|(track_id, path)| {
                let abs_path = resolver.resolve(std::path::Path::new(path));
                (*track_id, abs_path)
            })
            .collect();

        // Generate individual single-track mutations (batch scheduling at UI layer)
        let (label, mutations): (&str, Vec<Mutation>) = match button {
            ResolutionButton::ApplyDb => (
                "Apply DB tags → files",
                tracks.into_iter()
                    .map(|(track_id, path)| Mutation::ApplyDbTagsToDisk { track_id, path })
                    .collect(),
            ),
            ResolutionButton::AssimilateDisk => (
                "Assimilate file tags → DB",
                tracks.into_iter()
                    .map(|(track_id, path)| Mutation::AssimilateDiskTagsToDb { track_id, path })
                    .collect(),
            ),
        };

        if let Some(ref mut witch) = self.witch {
            let _ = super::operator_decisions::stage_decision(witch, 0, label, mutations);
        }

        // Note: oob_conflict_state is NOT cleared - preserved for Cancel return
        self.start_transaction_review(transaction_review::TransactionReviewSource::OobConflictResolution);
    }

    /// Stage acknowledgement mutation for mtime-only files in MtimeOnly bucket.
    ///
    /// If selection is active, only selected files are included.
    /// Otherwise, all files in the bucket are included.
    fn stage_oob_mtime_acknowledgement(&mut self) {
        use crate::corpus::mutations::Mutation;

        let resolver = paths::get_resolver();

        let tracks = match self.oob_conflict_state.as_ref() {
            Some(state) => {
                let bucket_state = state.active_bucket_state();

                // Determine which indices to process
                let indices: Vec<usize> = if bucket_state.selection.is_active() {
                    bucket_state.selection.selected_indices()
                } else {
                    // No selection - process all files in bucket
                    (0..bucket_state.files.len()).collect()
                };

                indices
                    .iter()
                    .filter_map(|&idx| bucket_state.files.get(idx))
                    .map(|f| {
                        let abs_path = resolver.resolve(std::path::Path::new(&f.path));
                        (f.track_id, abs_path)
                    })
                    .collect::<Vec<_>>()
            }
            None => return,
        };

        if tracks.is_empty() {
            self.status_message = Some("No files selected to acknowledge".to_string());
            return;
        }

        // Create single mutation with all tracks (id, path)
        let mutations = vec![Mutation::AcknowledgeMtimeOnly { tracks }];

        if let Some(ref mut witch) = self.witch {
            let _ = super::operator_decisions::stage_decision(
                witch,
                0,
                "Acknowledge mtime changes",
                mutations,
            );
        }

        // Note: oob_conflict_state is NOT cleared - preserved for Cancel return
        self.start_transaction_review(transaction_review::TransactionReviewSource::OobConflictResolution);
    }

    /// Handle tag canonicity modal actions.
    pub(super) fn handle_tag_canonicity_action(&mut self, action: tag_canonicity::TagCanonicalityAction) {
        match action {
            tag_canonicity::TagCanonicalityAction::None => {}
            tag_canonicity::TagCanonicalityAction::Confirmed => {
                // Stage decision and advance to next cluster
                self.stage_canonicity_decision();
                self.advance_to_next_cluster();
            }
            tag_canonicity::TagCanonicalityAction::Cancelled => {
                // Discard transaction if active via sealed operator decision handler
                if let Some(ref mut witch) = self.witch {
                    let _ = super::operator_decisions::discard_transaction(witch);
                }
                crate::logging::log_general("Tag canonicity resolution cancelled");
                self.tag_canonicity_state = None;
                self.tag_canonicity_clusters = None;
                self.start_insights_view();
            }
            tag_canonicity::TagCanonicalityAction::Navigate { forward } => {
                // User navigated to next/prev cluster - do NOT stage decision
                self.navigate_cluster(forward);
            }
            tag_canonicity::TagCanonicalityAction::ShowReview => {
                // Ctrl+R - stage current decision and show review
                self.stage_canonicity_decision();
                self.show_transaction_review_for_canonicity();
            }
        }
    }

    /// Navigate to next/prev cluster without staging a decision.
    fn navigate_cluster(&mut self, forward: bool) {
        let Some(ref mut clusters) = self.tag_canonicity_clusters else {
            self.tag_canonicity_state = None;
            self.start_insights_view();
            return;
        };

        if !forward && clusters.current_index == 0 {
            // Shift-Tab from first group = do nothing
            return;
        }

        if forward && clusters.is_last() {
            // Tab from last group = show review screen
            self.show_transaction_review_for_canonicity();
            return;
        }

        // Normal navigation
        let moved = if forward { clusters.next() } else { clusters.prev() };
        if moved {
            if !self.load_current_cluster_signal() {
                // Signal load failed - return to insights
                self.start_insights_view();
            }
        }
    }

    /// Advance to next cluster after confirming current one (via Enter).
    fn advance_to_next_cluster(&mut self) {
        let Some(ref mut clusters) = self.tag_canonicity_clusters else {
            self.show_transaction_review_for_canonicity();
            return;
        };

        if clusters.is_last() {
            // At last cluster - show review screen
            self.show_transaction_review_for_canonicity();
        } else if clusters.next() {
            // Load next signal
            if !self.load_current_cluster_signal() {
                // Signal load failed - show review with what we have
                self.show_transaction_review_for_canonicity();
            }
        } else {
            // No more clusters - show review
            self.show_transaction_review_for_canonicity();
        }
    }

    /// Show the transaction review screen for tag canonicity.
    fn show_transaction_review_for_canonicity(&mut self) {
        // Check if there are any decisions staged
        let has_decisions = self.witch.as_ref()
            .map(|w| !w.decision_indices().is_empty())
            .unwrap_or(false);

        if !has_decisions {
            // No decisions staged - just return to insights
            self.tag_canonicity_state = None;
            self.tag_canonicity_clusters = None;
            self.start_insights_view();
            return;
        }

        // Clear the resolution modal state (but keep clusters for Cancel navigation)
        self.tag_canonicity_state = None;

        // Transition to standardized review modal
        self.start_transaction_review(transaction_review::TransactionReviewSource::TagCanonicityResolution);
    }

    // ========================================================================
    // Compound Tag Split Resolution
    // ========================================================================

    /// Handle compound tag split modal actions.
    pub(super) fn handle_compound_split_action(&mut self, action: compound_split::CompoundSplitAction) {
        match action {
            compound_split::CompoundSplitAction::None => {}
            compound_split::CompoundSplitAction::Confirmed => {
                // Stage decision and advance to next signal
                self.stage_compound_split_decision();
                self.advance_to_next_compound_split();
            }
            compound_split::CompoundSplitAction::Cancelled => {
                // Discard transaction if active
                if let Some(ref mut witch) = self.witch {
                    let _ = super::operator_decisions::discard_transaction(witch);
                }
                crate::logging::log_general("Compound tag split cancelled");
                self.compound_split_state = None;
                self.compound_split_clusters = None;
                self.start_insights_view();
            }
            compound_split::CompoundSplitAction::Navigate { forward } => {
                // Navigate to next/prev signal without staging
                self.navigate_compound_split(forward);
            }
            compound_split::CompoundSplitAction::ShowReview => {
                // Ctrl+R - stage current decision and show review
                self.stage_compound_split_decision();
                self.show_transaction_review_for_compound_split();
            }
            compound_split::CompoundSplitAction::StageAllAndReview => {
                // Ctrl+A - stage ALL splits and go to review
                self.stage_all_compound_splits();
                self.show_transaction_review_for_compound_split();
            }
        }
    }

    /// Stage the current compound split decision.
    fn stage_compound_split_decision(&mut self) {
        let Some(ref state) = self.compound_split_state else {
            return;
        };

        let cluster_idx = self.compound_split_clusters
            .as_ref()
            .map(|c| c.current_index())
            .unwrap_or(0);

        // Get track info from witch: path and current TagSet
        // This allows mutations_with_paths to verify each track still has the compound value
        // and build the complete new tag set for DB-first pattern.
        let track_info = self.witch.as_mut()
            .map(|w| {
                let read_db = w.read_db();
                let resolver = paths::get_resolver();
                let mut info = std::collections::HashMap::new();
                for &track_id in &state.data.track_ids {
                    if let Ok(Some(track)) = read_db.get_track_by_id(track_id) {
                        // Resolve relative DB path to absolute for filesystem operations
                        let abs_path = resolver.resolve(std::path::Path::new(&track.path));
                        // Load complete TagSet from disk for building new tag set
                        let tagset = crate::corpus::tags::TagSet::from_file(&abs_path)
                            .unwrap_or_else(|_| crate::corpus::tags::TagSet::empty());
                        info.insert(track_id, (abs_path, tagset));
                    }
                }
                info
            })
            .unwrap_or_default();

        // Generate mutations (only for tracks that still have the compound value)
        let mutations = state.mutations_with_paths(&track_info);

        if mutations.is_empty() {
            return;
        }

        // Stage the decision via operator_decisions
        let description = format!(
            "Split \"{}\" in {} → [{}]",
            state.data.compound_value,
            state.data.tag_name,
            state.data.split_parts.join(", ")
        );

        if let Some(ref mut witch) = self.witch {
            let _ = super::operator_decisions::stage_decision(witch, cluster_idx, &description, mutations);
        }
    }

    /// Stage ALL compound split decisions at once.
    fn stage_all_compound_splits(&mut self) {
        use crate::corpus::db::types::{AggregateSignal, AggregateSignalType};
        use crate::corpus::mutations::Mutation;

        let Some(ref clusters) = self.compound_split_clusters else {
            return;
        };

        let signal_ids = clusters.all_signal_ids().to_vec();
        let total = signal_ids.len();

        // First pass: collect all data from database
        let mut decisions_to_stage: Vec<(usize, String, Vec<Mutation>)> = Vec::new();

        {
            let read_db = match self.witch.as_mut() {
                Some(w) => w.read_db(),
                None => return,
            };
            let resolver = paths::get_resolver();

            for (idx, signal_id) in signal_ids.iter().enumerate() {
                // Get signal by ID
                let signal = match read_db.get_signal_by_id(*signal_id) {
                    Ok(Some(s)) => s,
                    _ => continue,
                };

                // Convert to AggregateSignal
                let agg_signal = AggregateSignal {
                    id: signal.id,
                    signal_type: match AggregateSignalType::from_str(signal.issue_type.as_str()) {
                        Some(t) => t,
                        None => continue,
                    },
                    key: signal.issue_key,
                    discovered_at: signal.discovered_at,
                    metadata_json: signal.metadata_json,
                };

                // Skip if wrong type
                if agg_signal.signal_type != AggregateSignalType::CompoundTagValue {
                    continue;
                }

                // Parse data
                let Some(data) = compound_split::CompoundSplitData::from_signal(&agg_signal) else {
                    continue;
                };

                // Build track info: path and current TagSet
                let mut track_info = std::collections::HashMap::new();
                for &track_id in &data.track_ids {
                    if let Ok(Some(track)) = read_db.get_track_by_id(track_id) {
                        let abs_path = resolver.resolve(std::path::Path::new(&track.path));
                        let tagset = crate::corpus::tags::TagSet::from_file(&abs_path)
                            .unwrap_or_else(|_| crate::corpus::tags::TagSet::empty());
                        track_info.insert(track_id, (abs_path, tagset));
                    }
                }

                // Create temporary state to generate mutations
                let state = compound_split::CompoundSplitState::new(data.clone(), idx, total);
                let mutations = state.mutations_with_paths(&track_info);

                if mutations.is_empty() {
                    continue;
                }

                let description = format!(
                    "Split \"{}\" in {} → [{}]",
                    data.compound_value,
                    data.tag_name,
                    data.split_parts.join(", ")
                );

                decisions_to_stage.push((idx, description, mutations));
            }
        } // db borrow ends here

        // Second pass: stage all decisions
        let staged_count = decisions_to_stage.len();
        if let Some(ref mut witch) = self.witch {
            for (idx, description, mutations) in decisions_to_stage {
                let _ = super::operator_decisions::stage_decision(witch, idx, &description, mutations);
            }
        }

        self.status_message = Some(format!("Staged {} compound tag splits", staged_count));
    }

    /// Navigate to next/prev compound split signal without staging.
    fn navigate_compound_split(&mut self, forward: bool) {
        let Some(ref mut clusters) = self.compound_split_clusters else {
            self.compound_split_state = None;
            self.start_insights_view();
            return;
        };

        if !forward && clusters.is_first() {
            // Shift-Tab from first = do nothing
            return;
        }

        if forward && clusters.is_last() {
            // Tab from last = show review screen
            self.show_transaction_review_for_compound_split();
            return;
        }

        // Normal navigation
        let moved = if forward { clusters.next() } else { clusters.prev() };
        if moved {
            if !self.load_current_compound_split_signal() {
                // Signal load failed - return to insights
                self.start_insights_view();
            }
        }
    }

    /// Advance to next compound split signal after confirming current (via Enter).
    fn advance_to_next_compound_split(&mut self) {
        let Some(ref mut clusters) = self.compound_split_clusters else {
            self.show_transaction_review_for_compound_split();
            return;
        };

        if clusters.is_last() {
            // At last signal - show review
            self.show_transaction_review_for_compound_split();
        } else if clusters.next() {
            // Load next signal
            if !self.load_current_compound_split_signal() {
                // Signal load failed - show review with what we have
                self.show_transaction_review_for_compound_split();
            }
        } else {
            // No more signals - show review
            self.show_transaction_review_for_compound_split();
        }
    }

    /// Show the transaction review screen for compound tag splits.
    fn show_transaction_review_for_compound_split(&mut self) {
        // Check if there are any decisions staged
        let has_decisions = self.witch.as_ref()
            .map(|w| !w.decision_indices().is_empty())
            .unwrap_or(false);

        if !has_decisions {
            // No decisions staged - just return to insights
            self.compound_split_state = None;
            self.compound_split_clusters = None;
            self.start_insights_view();
            return;
        }

        // Clear the resolution modal state (but keep clusters for Cancel navigation)
        self.compound_split_state = None;

        // Transition to standardized review modal
        self.start_transaction_review(transaction_review::TransactionReviewSource::CompoundTagSplit);
    }

    /// Load the compound split signal at the current cluster index into modal state.
    /// Load the compound split signal at the current cluster index into modal state.
    /// Returns true if successfully loaded, false if failed (caller should handle fallback).
    fn load_current_compound_split_signal(&mut self) -> bool {
        use crate::corpus::db::types::{AggregateSignal, AggregateSignalType};

        let Some(ref clusters) = self.compound_split_clusters else {
            return false;
        };

        let Some(signal_id) = clusters.current_signal_id() else {
            self.compound_split_state = None;
            self.compound_split_clusters = None;
            return false;
        };

        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => {
                self.compound_split_state = None;
                self.compound_split_clusters = None;
                return false;
            }
        };

        // Fetch the signal by ID
        let signal = match read_db.get_signal_by_id(signal_id) {
            Ok(Some(s)) => s,
            _ => {
                self.status_message = Some("Signal not found".to_string());
                self.compound_split_state = None;
                self.compound_split_clusters = None;
                return false;
            }
        };

        // Convert to AggregateSignal
        let agg_signal = AggregateSignal {
            id: signal.id,
            signal_type: match AggregateSignalType::from_str(signal.issue_type.as_str()) {
                Some(t) => t,
                None => {
                    self.status_message = Some("Invalid signal type".to_string());
                    return false;
                }
            },
            key: signal.issue_key,
            discovered_at: signal.discovered_at,
            metadata_json: signal.metadata_json,
        };

        // Verify signal type
        if agg_signal.signal_type != AggregateSignalType::CompoundTagValue {
            self.status_message = Some(format!(
                "Wrong signal type: expected compound_tag_value, got {:?}",
                agg_signal.signal_type
            ));
            return false;
        }

        // Parse modal data from signal
        let Some(data) = compound_split::CompoundSplitData::from_signal(&agg_signal) else {
            self.status_message = Some("Failed to parse signal data".to_string());
            return false;
        };

        // Create modal state
        let state = compound_split::CompoundSplitState::new(
            data,
            clusters.current_index(),
            clusters.total(),
        );

        self.compound_split_state = Some(state);
        true
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
                        // Reload current cluster into tag_canonicity_state
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
                    Some(TransactionReviewSource::FormatStandardization) => {
                        // format_std state was preserved
                        self.mode = UiMode::FormatStandardization;
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
        self.intake_confirmation = None;
        self.format_std = None;
        self.oob_sync_state = None;
        self.oob_conflict_state = None;
        self.corrupt_file_preview = None;
        self.shit_format_preview = None;
    }

    /// Transition to the standardized transaction review modal.
    ///
    /// Called after staging decisions to show the review before commit.
    pub(super) fn start_transaction_review(&mut self, source: transaction_review::TransactionReviewSource) {
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

    /// Load the signal at the current cluster index into modal state.
    /// Returns true if successfully loaded, false if failed (caller should handle fallback).
    fn load_current_cluster_signal(&mut self) -> bool {
        use crate::corpus::db::types::AggregateSignalType;

        let Some(ref clusters) = self.tag_canonicity_clusters else {
            return false;
        };

        let Some(signal_id) = clusters.current_signal_id() else {
            self.tag_canonicity_state = None;
            self.tag_canonicity_clusters = None;
            return false;
        };

        let read_db = match self.witch.as_mut() {
            Some(w) => w.read_db(),
            None => {
                self.tag_canonicity_state = None;
                self.tag_canonicity_clusters = None;
                return false;
            }
        };

        // Load signal by ID
        let signal = match read_db.get_signal_by_id(signal_id) {
            Ok(Some(s)) => s,
            _ => {
                self.status_message = Some("Signal not found".to_string());
                self.tag_canonicity_state = None;
                self.tag_canonicity_clusters = None;
                return false;
            }
        };

        // Convert to AggregateSignal for modal data loading
        let agg_signal = crate::corpus::db::types::AggregateSignal {
            id: signal.id,
            signal_type: match crate::corpus::db::types::AggregateSignalType::from_str(signal.issue_type.as_str()) {
                Some(t) => t,
                None => {
                    self.status_message = Some("Invalid signal type".to_string());
                    return false;
                }
            },
            key: signal.issue_key,
            discovered_at: signal.discovered_at,
            metadata_json: signal.metadata_json,
        };

        let data = match tag_canonicity::TagCanonicalityModalData::from_signal(&agg_signal) {
            Some(d) => d,
            None => {
                self.status_message = Some("Failed to parse signal data".to_string());
                return false;
            }
        };

        // Determine pre-fill based on signal type
        let pre_fill = agg_signal.signal_type == AggregateSignalType::TagCanonicity;

        // Get group info from clusters
        let (group_index, total_groups) = self.tag_canonicity_clusters
            .as_ref()
            .map(|c| (c.current_index, c.signal_ids.len()))
            .unwrap_or((0, 1));

        let state = tag_canonicity::TagCanonicalityState::new(data, pre_fill, group_index, total_groups);
        self.tag_canonicity_state = Some(state);
        true
    }

    /// Stage a decision for the current canonicity cluster.
    ///
    /// This adds the decision to the transaction but does NOT confirm it.
    /// The transaction is confirmed when the user completes the review screen.
    fn stage_canonicity_decision(&mut self) {
        let Some(ref state) = self.tag_canonicity_state else {
            return;
        };

        let Some(ref mut witch) = self.witch else {
            return;
        };

        // Get track paths and current TagSets from disk
        let read_db = witch.read_db();
        let resolver = paths::get_resolver();
        let mut track_info = std::collections::HashMap::new();
        for &track_id in &state.data.track_ids {
            if let Ok(Some(track)) = read_db.get_track_by_id(track_id) {
                // Resolve relative DB path to absolute for filesystem operations
                let abs_path = resolver.resolve(std::path::Path::new(&track.path));
                // Load complete TagSet from disk for building new tag set
                let tagset = crate::corpus::tags::TagSet::from_file(&abs_path)
                    .unwrap_or_else(|_| crate::corpus::tags::TagSet::empty());
                track_info.insert(track_id, (abs_path, tagset));
            }
        }

        // Generate mutations - only for tracks whose current value is a selected variant
        let mutations = state.mutations_with_paths(&track_info);
        if mutations.is_empty() {
            // No mutations for this cluster - that's OK, skip it
            return;
        }

        let cluster_idx = self.tag_canonicity_clusters
            .as_ref()
            .map(|c| c.current_index)
            .unwrap_or(0);

        let label = format!("Canonicalize {}", state.data.tag_name);

        // Add decision to existing transaction via sealed operator decision handler
        let _ = super::operator_decisions::stage_decision(witch, cluster_idx, &label, mutations);
    }

    // =========================================================================
    // Format Standardization
    // =========================================================================

    pub(super) fn handle_format_std_action(&mut self, action: format_standardization::FormatStdAction) {
        match action {
            format_standardization::FormatStdAction::None => {}
            format_standardization::FormatStdAction::RequestQuit => {
                if self.has_pending_operations() {
                    self.status_message = Some("Cannot quit while operations are pending".to_string());
                } else {
                    self.exit_confirm_modal_state = Some(ExitConfirmModalState::default());
                    self.mode = UiMode::ExitConfirmModal;
                }
            }
            format_standardization::FormatStdAction::CycleNext => {
                self.format_std = None;
                self.start_lateral_view(widgets::LateralView::FormatStandardization.next());
            }
            format_standardization::FormatStdAction::CyclePrev => {
                self.format_std = None;
                self.start_lateral_view(widgets::LateralView::FormatStandardization.prev());
            }
            format_standardization::FormatStdAction::ConvertLossy { bitrate_kbps } => {
                let target = crate::corpus::transcode::TranscodeTarget::Opus { bitrate_kbps };
                self.stage_format_conversion(
                    format_standardization::LOSSY_TYPES,
                    target,
                    &format!("Convert lossy → {}", target.label()),
                );
            }
            format_standardization::FormatStdAction::ConvertLossless => {
                let target = crate::corpus::transcode::TranscodeTarget::Flac;
                self.stage_format_conversion(
                    format_standardization::LOSSLESS_TYPES,
                    target,
                    &format!("Convert lossless → {}", target.label()),
                );
            }
        }
    }

    /// Stage transcode mutations for all tracks matching given file types.
    fn stage_format_conversion(
        &mut self,
        file_types: &[&str],
        target: crate::corpus::transcode::TranscodeTarget,
        label: &str,
    ) {
        use crate::corpus::mutations::Mutation;

        let Some(ref mut witch) = self.witch else {
            self.status_message = Some("Witch not available".to_string());
            return;
        };

        let read_db = witch.read_db();
        let resolver = paths::get_resolver();

        let tracks = read_db.get_tracks_by_file_types(file_types).unwrap_or_default();
        if tracks.is_empty() {
            self.status_message = Some("No matching tracks found".to_string());
            return;
        }

        let mutations: Vec<Mutation> = tracks
            .iter()
            .map(|(track_id, rel_path, _file_type)| {
                let abs_path = resolver.resolve(
                    std::path::Path::new(rel_path),
                );
                Mutation::Transcode {
                    track_id: *track_id,
                    source_path: abs_path,
                    target_format: target,
                    stash_name: "remux-input".to_string(),
                }
            })
            .collect();

        if mutations.is_empty() {
            self.status_message = Some("No resolvable tracks found".to_string());
            return;
        }

        let count = mutations.len();
        let _ = witch.start_transaction(label);
        let _ = super::operator_decisions::stage_decision(witch, 0, label, mutations);

        self.status_message = Some(format!("Staged {} transcode operations", count));
        // Note: format_std state is NOT cleared - preserved for Cancel return
        self.start_transaction_review(transaction_review::TransactionReviewSource::FormatStandardization);
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
            // Add other modes as needed
            _ => {}
        }
    }
}
