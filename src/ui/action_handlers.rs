//! Action Handlers for View-Specific Events
//!
//! Each view (insights, tag search, tree browser, tag editor, etc.) emits
//! Actions that are handled here. These handlers coordinate state transitions,
//! daemon interactions, and modal displays.

use crate::config;
use crate::ui::{insights_view, progress_screen, tag_search, tree_browser, tag_editor, deploy_flow, startup};
use crate::ui::types::{UiMode, ExitConfirmModalState};
use super::App;

impl App {
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
                // Insights → Deploy
                self.insights_view = None;
                self.start_deployment_preview();
            }
            insights_view::InsightsAction::CyclePrev => {
                // Insights → Corpus Browser
                self.insights_view = None;
                self.start_corpus_browser();
            }
            insights_view::InsightsAction::LaunchFlow => {
                // Stub: flows not yet implemented
                self.status_message = Some("Flows not yet implemented".to_string());
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
                // TagSearch → CorpusBrowser
                self.tag_search = None;
                self.start_corpus_browser();
            }
            tag_search::TagSearchAction::CyclePrev => {
                // TagSearch → Deploy
                self.tag_search = None;
                self.start_deployment_preview();
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
        use crate::daemon::confirm_decision;

        match action {
            startup::IntakeConfirmationAction::None => {}
            startup::IntakeConfirmationAction::Confirmed => {
                // User confirmed - create IndexTrack mutations and start processing
                // Extract mutations first to avoid borrow conflicts
                let mutations = self.intake_confirmation
                    .as_ref()
                    .map(|s| s.create_index_mutations())
                    .unwrap_or_default();

                if !mutations.is_empty() {
                    let count = mutations.len();

                    let _ = config::log_message(&format!(
                        "IntakeConfirmation: user confirmed, queuing {} IndexTrack mutations",
                        count
                    ));

                    // Use the transaction API to queue mutations
                    let daemon = self.daemon();
                    if daemon.start_transaction("Intake indexing").is_ok() {
                        let witness = confirm_decision();
                        let _ = daemon.add_decision(0, &witness, "Index unindexed files", mutations);
                        let _ = daemon.confirm_transaction(&witness);
                    }

                    // Start processing mode - stay on this screen until complete
                    if let Some(ref mut state) = self.intake_confirmation {
                        state.start_processing();
                    }
                }
            }
            startup::IntakeConfirmationAction::Skipped => {
                // User skipped - no mutations ran, skip content analysis entirely
                // UnindexedFile signals remain for later handling
                let _ = config::log_message("IntakeConfirmation: user skipped indexing, going to Insights");

                self.intake_confirmation = None;
                self.start_insights_view();
            }
            startup::IntakeConfirmationAction::ProcessingComplete => {
                // Indexing complete - daemon auto-queued content analysis via transition_to_completed
                // Just show progress screen to wait for it to finish
                let _ = config::log_message("IntakeConfirmation: indexing complete, showing content analysis progress");

                self.intake_confirmation = None;
                self.progress_screen = Some(progress_screen::ProgressScreen::new_content_analysis());
                self.mode = UiMode::Progress;
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
                let witness = crate::daemon::confirm_decision();
                let commit_message = if let Some(daemon) = self.task_daemon.as_mut() {
                    match daemon.confirm_transaction(&witness) {
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
                    "No daemon available".to_string()
                };
                self.unified_tag_editor = None;
                self.start_insights_view();
                self.status_message = Some(commit_message);
            }

            UnifiedTagEditorAction::DiscardTransaction => {
                // Discard all staged decisions
                let witness = crate::daemon::confirm_decision();
                if let Some(daemon) = self.task_daemon.as_mut() {
                    let _ = daemon.discard_transaction(&witness);
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
                // Query daemon for staged decisions and populate the review modal
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
                // TODO: Reconnect when corpus::deploy is re-enabled
                // This function requires all_deployment_statuses_to_decisions from the disabled deploy module.
                self.status_message = Some("Deployment confirm disabled - deploy module being updated".to_string());
                self.deployment_preview = None;
                self.start_insights_view();
            }
            deploy_flow::DeploymentPreviewAction::Cancel => {
                let _ = config::log_message("Deployment preview cancelled");
                self.deployment_preview = None;
                self.start_insights_view();
                self.status_message = Some("Deployment cancelled".to_string());
            }
            deploy_flow::DeploymentPreviewAction::CycleNext => {
                // Deploy → TagSearch
                self.deployment_preview = None;
                self.start_tag_search();
            }
            deploy_flow::DeploymentPreviewAction::CyclePrev => {
                // Deploy → Insights
                self.deployment_preview = None;
                self.start_insights_view();
            }
        }
    }
}
