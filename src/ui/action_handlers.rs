//! Action Handlers for View-Specific Events
//!
//! Each view (insights, tag search, tree browser, tag editor, etc.) emits
//! Actions that are handled here. These handlers coordinate state transitions,
//! Witch interactions, and modal displays.

use crate::config;
use crate::ui::{insights_view, missing_file_flow, progress_screen, tag_search, tree_browser, tag_editor, deploy_flow, startup};
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
                // Insights → Tag Search (Deploy removed from lateral ring)
                self.insights_view = None;
                self.start_tag_search();
            }
            insights_view::InsightsAction::CyclePrev => {
                // Insights → Corpus Browser
                self.insights_view = None;
                self.start_corpus_browser();
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
    ///
    /// Uses cached data if available, otherwise loads from database.
    fn start_deployment_preview_from_insights(&mut self) {
        // Try to use cached deploy modal data first
        let data = self.witch.as_ref()
            .and_then(|w| w.ui_read_cache().deploy_modal_data())
            .or_else(|| {
                // Fall back to loading from database
                self.witch.as_mut().and_then(|w| {
                    let db = w.read_only_db();
                    deploy_flow::DeployModalData::load(db).ok()
                })
            })
            .unwrap_or_default();

        // Trigger cache warming for next time
        if let Some(ref witch) = self.witch {
            witch.ui_read_cache().warm_deploy_modal_data();
        }

        // Create preview state with cached data
        let preview = deploy_flow::DeploymentPreviewState::new(data);
        self.deployment_preview = Some(preview);
        self.mode = UiMode::DeploymentPreview;

        // Keep insights view alive for return
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
                // TagSearch → CorpusBrowser
                self.tag_search = None;
                self.start_corpus_browser();
            }
            tag_search::TagSearchAction::CyclePrev => {
                // TagSearch → Insights (Deploy removed from lateral ring)
                self.tag_search = None;
                self.start_insights_view();
            }
            tag_search::TagSearchAction::ExecuteSearch => {
                // Execute search with db access - take ownership temporarily to avoid borrow conflict
                if let Some(mut search) = self.tag_search.take() {
                    let db = self.db();
                    search.execute_search(&db);
                    self.tag_search = Some(search);
                }
            }
            tag_search::TagSearchAction::EditTrack(track) => {
                // Open unified tag editor for single track
                self.tag_search = None;
                self.start_unified_tag_editor_for_track(track);
            }
            tag_search::TagSearchAction::EditAllTracks(tracks) => {
                // Open unified tag editor for all result tracks
                self.tag_search = None;
                self.start_unified_tag_editor_for_tracks(tracks);
            }
        }
    }

    /// Handle intake confirmation dialog actions.
    pub(super) fn handle_intake_confirmation_action(&mut self, action: startup::IntakeConfirmationAction) {
        use crate::witch::confirm_decision;

        match action {
            startup::IntakeConfirmationAction::None => {}
            startup::IntakeConfirmationAction::Confirmed => {
                // User confirmed - create IndexTrack mutations and immediately transition to progress screen
                let mutations = self.intake_confirmation
                    .as_ref()
                    .map(|s| s.create_index_mutations())
                    .unwrap_or_default();

                self.intake_confirmation = None;

                if mutations.is_empty() {
                    // No files to index (all deleted since detection?) - skip to Insights
                    let _ = config::log_message("IntakeConfirmation: no mutations to queue, skipping to Insights");
                    self.start_insights_view();
                } else {
                    let count = mutations.len();
                    let _ = config::log_message(&format!(
                        "IntakeConfirmation: user confirmed, queuing {} IndexTrack mutations",
                        count
                    ));

                    // Use the transaction API to queue mutations
                    let the_witch = self.witch();
                    if the_witch.start_transaction("Intake indexing").is_ok() {
                        let witness = confirm_decision();
                        let _ = the_witch.add_decision(0, &witness, "Index unindexed files", mutations);
                        let _ = the_witch.confirm_transaction(&witness);
                    }

                    // Transition to progress screen to wait for indexing + content analysis
                    self.transition_to_progress_after_mutations(progress_screen::ProgressPhase::ContentAnalysis);
                }
            }
            startup::IntakeConfirmationAction::Skipped => {
                // User skipped - no mutations ran, skip content analysis entirely
                // UnindexedFile signals remain for later handling
                let _ = config::log_message("IntakeConfirmation: user skipped indexing, going to Insights");

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
                // Corpus Browser → Insights
                self.tree_browser = None;
                self.start_insights_view();
            }
            tree_browser::TreeBrowserAction::CyclePrev => {
                // Corpus Browser → TagSearch
                self.tree_browser = None;
                self.start_tag_search();
            }
            tree_browser::TreeBrowserAction::SelectPaths(paths) => {
                // Directory selector completed - currently unused, placeholder for dedup flows
                let _ = crate::config::log_message(&format!(
                    "Directory selector returned {} paths (flow not yet wired)",
                    paths.len()
                ));
                self.tree_browser = None;
                self.start_insights_view();
            }
        }
    }

    pub(super) fn handle_unified_tag_editor_action(&mut self, action: tag_editor::UnifiedTagEditorAction) {
        use tag_editor::UnifiedTagEditorAction;

        match action {
            UnifiedTagEditorAction::None => {}
            UnifiedTagEditorAction::CloseModal => {}

            UnifiedTagEditorAction::StageDecision { index, mutations } => {
                // User confirmed changes for this item - stage to transaction
                self.stage_decision(index, mutations);
            }

            UnifiedTagEditorAction::StageDecisionAndNext { index, mutations } => {
                // Stage the decision AND navigate to next sibling
                self.stage_decision(index, mutations);
                self.navigate_to_next_sibling();
            }

            UnifiedTagEditorAction::StageDecisionAndReview { index, mutations } => {
                // Stage the decision AND immediately show transaction review
                // Used for aggregated mode or single-item contexts where "next sibling" is meaningless
                self.stage_decision(index, mutations);

                // Immediately show transaction review modal
                let decisions = self.gather_transaction_decisions();

                if let Some(ref mut editor) = self.unified_tag_editor {
                    editor.modal = Some(tag_editor::UnifiedTagEditorModal::TransactionReview {
                        decisions,
                        scroll: 0,
                        selected_button: tag_editor::TransactionReviewButton::CommitAll,
                    });
                }
            }

            UnifiedTagEditorAction::CommitTransaction => {
                // Commit all staged decisions
                let witness = crate::witch::confirm_decision();
                let commit_message = if let Some(the_witch) = self.witch.as_mut() {
                    match the_witch.confirm_transaction(&witness) {
                        Ok(summary) => {
                            format!(
                                "Committed {} decisions ({} mutations)",
                                summary.decision_count,
                                summary.mutation_count
                            )
                        }
                        Err(e) => {
                            format!("Commit failed: {}", e)
                        }
                    }
                } else {
                    "The Witch is not available".to_string()
                };
                self.unified_tag_editor = None;
                self.start_insights_view();
                self.status_message = Some(commit_message);
            }

            UnifiedTagEditorAction::DiscardTransaction => {
                // Discard all staged decisions
                let witness = crate::witch::confirm_decision();
                if let Some(the_witch) = self.witch.as_mut() {
                    let _ = the_witch.discard_transaction(&witness);
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

            UnifiedTagEditorAction::NextSibling => {
                self.navigate_to_next_sibling();
            }

            UnifiedTagEditorAction::PrevSibling => {
                self.navigate_to_prev_sibling();
            }

            UnifiedTagEditorAction::ShowModal(modal) => {
                if let Some(ref mut editor) = self.unified_tag_editor {
                    editor.modal = Some(modal);
                }
            }

            UnifiedTagEditorAction::StatusMessage(msg) => {
                self.status_message = Some(msg);
            }

            UnifiedTagEditorAction::RequestFillFromDb { track_id } => {
                match track_id {
                    Some(id) => {
                        let db = self.db();
                        match db.get_track_tags(id) {
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
                // Query the Witch for staged decisions and populate the review modal
                let decisions = self.gather_transaction_decisions();

                if let Some(ref mut editor) = self.unified_tag_editor {
                    editor.modal = Some(tag_editor::UnifiedTagEditorModal::TransactionReview {
                        decisions,
                        scroll: 0,
                        selected_button: tag_editor::TransactionReviewButton::CommitAll,
                    });
                }
            }
        }
    }

    pub(super) fn handle_deployment_preview_action(&mut self, action: deploy_flow::DeploymentPreviewAction) {
        match action {
            deploy_flow::DeploymentPreviewAction::None => {}
            deploy_flow::DeploymentPreviewAction::Confirm => {
                // Generate and queue deploy mutations
                // Clone the cached data to avoid borrow issues
                let cached_data = self.deployment_preview.as_ref()
                    .map(|p| p.cached_data.clone());
                if let Some(data) = cached_data {
                    let mutation_count = self.execute_deploy_mutations(&data);
                    if mutation_count > 0 {
                        self.deployment_preview = None;
                        self.transition_to_progress_after_mutations(progress_screen::ProgressPhase::SignalRefresh);
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
                let _ = config::log_message("Deployment preview cancelled");
                self.deployment_preview = None;
                self.start_insights_view();
                self.status_message = Some("Deployment cancelled".to_string());
            }
        }
    }

    /// Execute deploy mutations from the cached data.
    ///
    /// Returns the number of mutations queued.
    fn execute_deploy_mutations(&mut self, data: &deploy_flow::DeployModalData) -> usize {
        use crate::corpus::mutations::Mutation;
        use crate::witch::confirm_decision;
        use std::path::PathBuf;

        let Some(ref mut witch) = self.witch else {
            return 0;
        };
        let mut mutations = Vec::new();

        // New files: create hard links
        for file in &data.new {
            // Skip files with empty deploy_path (data integrity check)
            if file.deploy_path.is_empty() {
                continue;
            }
            mutations.push(Mutation::HardLink {
                source: PathBuf::from(&file.corpus_path),
                destination: PathBuf::from(&file.deploy_path),
            });
        }

        // Stale files: move from wrong path to correct path
        for file in &data.stale {
            mutations.push(Mutation::LibraryMove {
                source: PathBuf::from(&file.library_path),
                destination: PathBuf::from(&file.expected_path),
            });
        }

        // Leftover files: move to stash (preserve data, never destroy)
        for file in &data.leftover {
            mutations.push(Mutation::MoveToStash {
                path: PathBuf::from(&file.library_path),
                track_id: None, // No corpus backing
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
                mutations.push(Mutation::HardLink {
                    source: PathBuf::from(corpus_path),
                    destination: PathBuf::from(&group.deploy_path),
                });
            }
        }

        let count = mutations.len();
        if count == 0 {
            return 0;
        }

        // Submit via transaction API
        let witness = confirm_decision();
        if witch.start_transaction("Deploy").is_ok() {
            let _ = witch.add_decision(0, &witness, "Deploy operations", mutations);
            let _ = witch.confirm_transaction(&witness);
        }

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
                let db = w.read_only_db();
                missing_file_flow::MissingFileModalData::load(db).ok()
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
                // Generate and queue restore mutations (HardLink)
                if let Some(ref preview) = self.missing_file_preview {
                    let mutations = preview.cached_data.restore_mutations();
                    let count = mutations.len();
                    if count > 0 {
                        self.execute_missing_file_mutations(mutations, "Restore missing files");
                        self.missing_file_preview = None;
                        self.transition_to_progress_after_mutations(progress_screen::ProgressPhase::SignalRefresh);
                    } else {
                        self.status_message = Some("No files to restore".to_string());
                    }
                }
            }
            missing_file_flow::MissingFilePreviewAction::ConfirmDrop => {
                // Generate and queue drop mutations (DropFromIndex)
                if let Some(ref preview) = self.missing_file_preview {
                    let mutations = preview.cached_data.drop_mutations();
                    let count = mutations.len();
                    if count > 0 {
                        self.execute_missing_file_mutations(mutations, "Drop non-restorable files");
                        self.missing_file_preview = None;
                        self.transition_to_progress_after_mutations(progress_screen::ProgressPhase::SignalRefresh);
                    } else {
                        self.status_message = Some("No files to drop".to_string());
                    }
                }
            }
            missing_file_flow::MissingFilePreviewAction::Cancel => {
                let _ = config::log_message("Missing file resolution cancelled");
                self.missing_file_preview = None;
                self.start_insights_view();
            }
        }
    }

    /// Execute missing file mutations.
    fn execute_missing_file_mutations(&mut self, mutations: Vec<crate::corpus::mutations::Mutation>, label: &str) {
        use crate::witch::confirm_decision;

        let Some(ref mut witch) = self.witch else {
            return;
        };

        let witness = confirm_decision();
        if witch.start_transaction(label).is_ok() {
            let _ = witch.add_decision(0, &witness, label, mutations);
            let _ = witch.confirm_transaction(&witness);
        }
    }
}
