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
mod deploy;
mod inbox;
mod manual_review;
mod oob_resolution;
mod simple_resolutions;
mod tag_canonicity;
mod witness;

use crate::ui::{filter_popup, insights_view, oob_sync_modal, oob_conflict_modal, progress_screen, tag_search, transaction_review, tree_browser, tag_editor, startup, widgets};
use crate::ui::active_view::{ActiveView, FilterOverlay, FilterPopupContext, ViewAction};
use crate::ui::suspended_views::SuspendTarget;
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
            ViewAction::ConfigEditor(a) => self.handle_config_editor_action(a),
            ViewAction::Insights(a) => self.handle_insights_action(a),
            ViewAction::CorpusBrowser(a) => self.handle_tree_browser_action(a),
            ViewAction::TagSearch(a) => self.handle_tag_search_action(a),
            ViewAction::Inbox(a) => self.handle_inbox_action(a, witness.as_ref()),
            ViewAction::ExitConfirm(a) => self.handle_exit_confirm_action(a),
            ViewAction::IntakeConfirmation(a) => self.handle_intake_confirmation_action(a, witness.as_ref()),
            ViewAction::UnifiedTagEditor(a) => self.handle_unified_tag_editor_action(a, witness.as_ref()),
            ViewAction::Deploy(a) => self.handle_deploy_action(a, witness.as_ref()),
            ViewAction::MissingFileResolution(a) => self.handle_missing_file_preview_action(a, witness.as_ref()),
            ViewAction::MissingDirectoryResolution(a) => self.handle_missing_directory_preview_action(a, witness.as_ref()),
            ViewAction::CorruptFileResolution(a) => self.handle_corrupt_file_preview_action(a, witness.as_ref()),
            ViewAction::ShitFormatResolution(a) => self.handle_shit_format_preview_action(a, witness.as_ref()),
            ViewAction::EmbedAlbumArtResolution(a) => self.handle_embed_album_art_preview_action(a, witness.as_ref()),
            ViewAction::SubparDuplicateResolution(a) => self.handle_subpar_duplicate_preview_action(a, witness.as_ref()),
            ViewAction::InboxCorpusMatchResolution(a) => self.handle_inbox_corpus_match_preview_action(a, witness.as_ref()),
            ViewAction::InboxOrganize(a) => self.handle_inbox_organize_action(a, witness.as_ref()),
            ViewAction::DirectoryClusterResolution(a) => self.handle_directory_cluster_preview_action(a, witness.as_ref()),
            ViewAction::MovedFileAcknowledge(a) => self.handle_moved_file_action(a, witness.as_ref()),
            ViewAction::OobSyncResolution(a) => self.handle_oob_sync_action(a, witness.as_ref()),
            ViewAction::OobConflictInspection(a) => self.handle_oob_conflict_action(a, witness.as_ref()),
            ViewAction::TagCanonicityResolution(a) => self.handle_tag_canonicity_action(a, witness.as_ref()),
            ViewAction::CompoundTagSplit(a) => self.handle_compound_split_action(a, witness.as_ref()),
            ViewAction::MissingAlbumSingleResolution(a) => self.handle_missing_album_single_action(a, witness.as_ref()),
            ViewAction::ManualReview(a) => self.handle_manual_review_action(a, witness.as_ref()),
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

    fn handle_config_editor_action(&mut self, action: super::config_editor::ConfigEditorAction) {
        use crate::meta::mutations::Mutation;
        use crate::meta::mutations::config_edit::ApplyConfigEditsMutation;

        match action {
            super::config_editor::ConfigEditorAction::None => {}
            super::config_editor::ConfigEditorAction::Save => {
                // Build mutation from editor state and stage for transaction review.
                let mutation_data = if let ActiveView::ConfigEditor(ref state) = self.view {
                    if state.has_edits() {
                        let new_config = state.build_config();
                        let original_kdl = state.original_kdl.clone().unwrap_or_default();
                        let old_config = state.original_config.clone();
                        Some((original_kdl, old_config, new_config))
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some((original_kdl, old_config, new_config)) = mutation_data {
                    let mutation = Mutation::ApplyConfigEdits(ApplyConfigEditsMutation {
                        original_kdl,
                        old_config,
                        new_config,
                    });

                    if let Some(ref mut witch) = self.witch {
                        let _ = witch.start_transaction("Config update");
                        let _ = super::operator_decisions::stage_decision(
                            witch, 0, "Apply config changes", vec![mutation],
                        );
                    }

                    self.start_transaction_review();
                } else {
                    // No edits — just return to insights
                    self.start_insights_view();
                }
            }
            super::config_editor::ConfigEditorAction::Discard => {
                self.start_insights_view();
            }
            super::config_editor::ConfigEditorAction::CycleNext => {
                self.start_lateral_view(widgets::LateralView::Config.next());
            }
            super::config_editor::ConfigEditorAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::Config.prev());
            }
        }
    }

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
                    Some(insights_view::InsightAction::LaunchManualReview) => {
                        // Determine ReviewKind from the selected insight type
                        let kind = if let ActiveView::Insights(ref v) = self.view {
                            v.selected_insight_type().and_then(|t| match t {
                                insights_view::InsightType::RedundantDuplicates => {
                                    Some(crate::ui::manual_review_modal::ReviewKind::RedundantDuplicate)
                                }
                                insights_view::InsightType::OtherSignal { .. } => {
                                    // Check the signal_type from the entry
                                    v.selected_entry().and_then(|e| {
                                        match e.label.as_str() {
                                            "Deploy Conflicts" => Some(crate::ui::manual_review_modal::ReviewKind::DeployConflict),
                                            "Metadata Duplicates" => Some(crate::ui::manual_review_modal::ReviewKind::MetadataDuplicate),
                                            _ => None,
                                        }
                                    })
                                }
                                _ => None,
                            })
                        } else {
                            None
                        };
                        match kind {
                            Some(k) => self.start_manual_review(k),
                            None => {
                                self.status_message = Some("Unknown review type".to_string());
                            }
                        }
                    }
                    Some(insights_view::InsightAction::LaunchMissingTagResolution) => {
                        self.start_missing_tag_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchMissingAlbumSingleResolution) => {
                        self.start_missing_album_single_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchEmbeddedDiscNumberResolution) => {
                        self.status_message = Some("Not yet implemented".to_string());
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
        let corpus_root = self.config().corpus_dir();
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

    /// Start missing tag resolution from Insights view.
    ///
    /// Loads all MissingTag signals, collects unique inodes, and opens
    /// the bulk tag editor so the operator can fill in missing tags.
    fn start_missing_tag_resolution(&mut self) {
        use crate::corpus::db::types::Zone;
        use std::collections::BTreeSet;

        let read_db = self.read_db();

        let signals = match read_db.get_missing_tag_signals() {
            Ok(s) => s,
            Err(e) => {
                self.status_message = Some(format!("Failed to load missing tag signals: {}", e));
                return;
            }
        };

        if signals.is_empty() {
            self.status_message = Some("No missing tag signals".to_string());
            return;
        }

        // Collect all unique inodes across all signal groups
        let all_inodes: Vec<i64> = signals.iter()
            .flat_map(|s| s.data.inodes.iter().copied())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        let audio_files = match read_db.get_audio_files_by_inodes(&all_inodes, Zone::Corpus) {
            Ok(f) => f,
            Err(e) => {
                self.status_message = Some(format!("Failed to load audio files: {}", e));
                return;
            }
        };

        if audio_files.is_empty() {
            self.status_message = Some("No indexed audio files for missing tags".to_string());
            return;
        }

        self.open_unified_tag_editor_bulk(
            audio_files,
            tag_editor::TagEditorSource::HealthModal,
            None,
        );
    }

    /// Start missing album single resolution from Insights view.
    ///
    /// Loads MissingAlbumSingle signals, converts to modal data, starts a
    /// transaction, and switches to the MissingAlbumSingleResolution view.
    fn start_missing_album_single_resolution(&mut self) {
        use crate::ui::missing_album_modal;

        let read_db = self.read_db();

        let signals = match read_db.get_missing_album_single_signals() {
            Ok(s) => s,
            Err(e) => {
                self.status_message = Some(format!("Failed to load missing album singles: {}", e));
                return;
            }
        };

        if signals.is_empty() {
            self.status_message = Some("No missing album singles".to_string());
            return;
        }

        let data = missing_album_modal::MissingAlbumData::from_signals(signals);
        let suffix = self.config().opinions.health_detection.single_album_suffix.clone();

        // Start transaction for the resolution session
        if let Some(ref mut witch) = self.witch {
            let _ = witch.start_transaction("Missing album singles");
        }

        let state = missing_album_modal::MissingAlbumState::new(data, suffix);
        self.view = ActiveView::MissingAlbumSingleResolution(state);
    }

    /// Handle missing album single resolution actions.
    fn handle_missing_album_single_action(
        &mut self,
        action: crate::ui::missing_album_modal::MissingAlbumAction,
        witness: Option<&witness::DecisionWitness>,
    ) {
        use crate::corpus::db::types::Zone;
        use crate::meta::mutations::{Mutation, TagOp, tag_edit::ApplyTagOpsMutation, indexing::EmitExpectedMissingTagMutation};
        use crate::ui::missing_album_modal::{MissingAlbumAction, AlbumResolution};

        match action {
            MissingAlbumAction::None => {}

            MissingAlbumAction::Cancel => {
                self.cancel_and_return_to_insights("Missing album single resolution cancelled");
            }

            MissingAlbumAction::Confirm(resolution) => {
                let Some(_w) = witness else { return };

                let group_idx = {
                    let ActiveView::MissingAlbumSingleResolution(ref state) = self.view else { return };
                    state.current_group
                };

                match resolution {
                    AlbumResolution::PerTrackTitle => {
                        let ops: Vec<TagOp> = {
                            let ActiveView::MissingAlbumSingleResolution(ref state) = self.view else { return };
                            let Some(group) = state.current_group_data() else { return };
                            group.tracks.iter().map(|t| {
                                TagOp::add_tag(t.inode, "ALBUM", format!("{}{}", t.title, state.suffix))
                            }).collect()
                        };
                        if !ops.is_empty() {
                            let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation { ops, zone: Zone::Corpus });
                            if let Some(ref mut witch) = self.witch {
                                let _ = super::operator_decisions::stage_decision(
                                    witch, group_idx, "Tag as singles", vec![mutation],
                                );
                            }
                        }
                    }

                    AlbumResolution::AllSingles => {
                        let ops: Vec<TagOp> = {
                            let ActiveView::MissingAlbumSingleResolution(ref state) = self.view else { return };
                            let Some(group) = state.current_group_data() else { return };
                            group.tracks.iter().map(|t| {
                                TagOp::add_tag(t.inode, "ALBUM", "Singles")
                            }).collect()
                        };
                        if !ops.is_empty() {
                            let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation { ops, zone: Zone::Corpus });
                            if let Some(ref mut witch) = self.witch {
                                let _ = super::operator_decisions::stage_decision(
                                    witch, group_idx, "Tag all as Singles", vec![mutation],
                                );
                            }
                        }
                    }

                    AlbumResolution::Suppress => {
                        let inodes: Vec<i64> = {
                            let ActiveView::MissingAlbumSingleResolution(ref state) = self.view else { return };
                            state.current_group_inodes()
                        };
                        if !inodes.is_empty() {
                            let mutation = Mutation::EmitExpectedMissingTag(EmitExpectedMissingTagMutation { inodes });
                            if let Some(ref mut witch) = self.witch {
                                let _ = super::operator_decisions::stage_decision(
                                    witch, group_idx, "Suppress missing album", vec![mutation],
                                );
                            }
                        }
                    }
                }

                // Advance to next unresolved group, or show review if at end
                if let ActiveView::MissingAlbumSingleResolution(ref mut state) = self.view {
                    state.track_cursor = 0;
                    state.track_scroll = 0;
                    if state.current_group + 1 < state.data.groups.len() {
                        state.current_group += 1;
                    } else {
                        // All groups visited — go to review
                        self.start_transaction_review();
                        return;
                    }
                }
            }

            MissingAlbumAction::NavigateGroup(forward) => {
                if let ActiveView::MissingAlbumSingleResolution(ref mut state) = self.view {
                    if forward {
                        if state.current_group + 1 < state.data.groups.len() {
                            state.current_group += 1;
                            state.track_cursor = 0;
                            state.track_scroll = 0;
                        }
                    } else if state.current_group > 0 {
                        state.current_group -= 1;
                        state.track_cursor = 0;
                        state.track_scroll = 0;
                    }
                }
            }

            MissingAlbumAction::ShowReview => {
                self.start_transaction_review();
            }

            MissingAlbumAction::EditTracks => {
                let (inodes, decision_index, label) = {
                    let ActiveView::MissingAlbumSingleResolution(ref state) = self.view else { return };
                    let group = match state.current_group_data() {
                        Some(g) => g,
                        None => return,
                    };
                    // Offset decision index to avoid colliding with resolution decisions
                    let idx = state.data.groups.len() + state.current_group;
                    let label = format!("Manual tag edits: {}", group.artist);
                    (state.current_group_inodes(), idx, label)
                };
                if inodes.is_empty() {
                    return;
                }
                let read_db = self.read_db();
                let audio_files = read_db.get_audio_files_by_inodes(
                    &inodes,
                    crate::corpus::db::types::Zone::Corpus,
                ).unwrap_or_default();
                if !audio_files.is_empty() {
                    self.open_embedded_tag_editor(
                        tag_editor::TagEditorMode::Individual,
                        audio_files,
                        decision_index,
                        label,
                    );
                }
            }

            MissingAlbumAction::EditTracksAggregated => {
                let (inodes, decision_index, label) = {
                    let ActiveView::MissingAlbumSingleResolution(ref state) = self.view else { return };
                    let group = match state.current_group_data() {
                        Some(g) => g,
                        None => return,
                    };
                    let idx = state.data.groups.len() + state.current_group;
                    let label = format!("Manual tag edits: {}", group.artist);
                    (state.current_group_inodes(), idx, label)
                };
                if inodes.is_empty() {
                    return;
                }
                let read_db = self.read_db();
                let audio_files = read_db.get_audio_files_by_inodes(
                    &inodes,
                    crate::corpus::db::types::Zone::Corpus,
                ).unwrap_or_default();
                if !audio_files.is_empty() {
                    self.open_embedded_tag_editor(
                        tag_editor::TagEditorMode::Aggregated,
                        audio_files,
                        decision_index,
                        label,
                    );
                }
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
                self.push_current_view();
                self.start_unified_tag_editor_for_audio_file(audio_file);
            }
        }
    }

    /// Handle intake confirmation dialog actions.
    fn handle_intake_confirmation_action(&mut self, action: startup::IntakeConfirmationAction, _witness: Option<&witness::DecisionWitness>) {
        use super::operator_decisions;

        // Determine zone before matching (used for post-action routing)
        let is_inbox_zone = matches!(
            self.view,
            ActiveView::IntakeConfirmation(ref s) if s.zone == "inbox"
        );

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
                    // No files to index (all deleted since detection?)
                    crate::logging::log_general("IntakeConfirmation: no mutations to queue");
                    if is_inbox_zone {
                        self.view = ActiveView::Inbox(super::inbox_view::InboxViewState::new());
                    } else {
                        self.start_insights_view();
                    }
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
                // User skipped - no mutations ran
                // Unindexed signals remain for later handling
                crate::logging::log_general("IntakeConfirmation: user skipped indexing");

                // Discard any active transaction from review modal
                if let Some(ref mut witch) = self.witch {
                    if witch.has_transaction() {
                        let _ = operator_decisions::discard_transaction(witch);
                    }
                }

                if is_inbox_zone {
                    self.view = ActiveView::Inbox(super::inbox_view::InboxViewState::new());
                } else {
                    self.start_insights_view();
                }
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
                self.push_current_view();
                self.open_unified_tag_editor_for_directory(&path);
            }
            tree_browser::TreeBrowserAction::EditFile(path) => {
                self.push_current_view();
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
                use crate::ui::tag_editor::types::NavigationDirection;
                let is_embedded = matches!(&self.view, ActiveView::UnifiedTagEditor(ref e) if e.is_embedded());

                if is_embedded {
                    // Embedded mode: track locally, don't stage to transaction
                    if let ActiveView::UnifiedTagEditor(ref mut editor) = self.view {
                        editor.set_staged_mutations(mutations);
                        editor.staged_decision_count += 1;
                    }
                } else {
                    // Standalone mode: stage to transaction (requires witness)
                    let Some(w) = witness else { return };
                    self.stage_tag_editor_decision(index, mutations, w);
                }

                // Navigate in both modes
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
                // Note: tag editor state is preserved on the view stack for Cancel return
                self.start_transaction_review();
            }

            UnifiedTagEditorAction::DiscardTransaction => {
                // Discard all staged decisions via sealed operator decision handler
                if let Some(the_witch) = self.witch.as_mut() {
                    let _ = super::operator_decisions::discard_transaction(the_witch);
                }
                // Return to the view that launched the tag editor (e.g., corpus browser)
                if !self.pop_and_restore() {
                    self.start_insights_view();
                }
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

            UnifiedTagEditorAction::CloseEmbedded => {
                // Return to parent health modal without staging
                if !self.pop_and_restore() {
                    self.start_insights_view();
                }
            }

            UnifiedTagEditorAction::StageAndCloseEmbedded { decision_index, decision_label, mutations } => {
                let Some(_w) = witness else { return };
                // Stage collected mutations at parent's decision index
                if let Some(ref mut witch) = self.witch {
                    let _ = super::operator_decisions::stage_decision(
                        witch, decision_index, &decision_label, mutations,
                    );
                }
                // Return to parent health modal
                if !self.pop_and_restore() {
                    self.start_insights_view();
                }
            }

            UnifiedTagEditorAction::RequestTransactionReview => {
                // Transition to standardized review modal
                // Note: tag editor state is preserved on the view stack for Cancel return
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
    /// - Cancel: pop view stack to restore source view
    /// - Discard: discard transaction, clear view stack, return to Insights
    /// - Confirm: commit transaction, clear view stack, go to Progress
    fn handle_transaction_review_action(&mut self, action: transaction_review::TransactionReviewAction, _witness: Option<&witness::DecisionWitness>) {
        use transaction_review::TransactionReviewAction;

        match action {
            TransactionReviewAction::None => {}

            TransactionReviewAction::Cancel => {
                // Pop the view stack to restore the parent view
                if !self.pop_and_restore() {
                    self.start_insights_view();
                }
            }

            TransactionReviewAction::Discard => {
                // Discard transaction, clear entire view stack, return to insights
                self.clear_view_stack();
                if let Some(ref mut witch) = self.witch {
                    let _ = super::operator_decisions::discard_transaction(witch);
                }
                self.start_insights_view();
                self.status_message = Some("Transaction discarded".to_string());
            }

            TransactionReviewAction::Confirm => {
                // Determine progress phase before clearing state
                let post_commit_phase = if let ActiveView::TransactionReview(ref review) = self.view {
                    review.post_commit_phase
                } else {
                    transaction_review::PostCommitPhase::default()
                };

                // Clear entire view stack — commit is a hard navigation
                self.clear_view_stack();

                // Commit transaction
                let commit_result = if let Some(ref mut witch) = self.witch {
                    super::operator_decisions::commit_transaction(witch)
                } else {
                    Err(crate::witch::TransactionError::NoActiveTransaction)
                };

                match commit_result {
                    Ok(()) => {
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
    /// Pushes the current view onto the view stack and switches to review.
    pub(in crate::ui) fn start_transaction_review(&mut self) {
        self.push_and_switch(SuspendTarget::TransactionReview(
            transaction_review::TransactionReviewState::new(),
        ));
    }

    /// Transition to transaction review modal with custom post-commit phase.
    ///
    /// Used for intake indexing which needs ContentAnalysis instead of SignalRefresh.
    pub(super) fn start_transaction_review_with_phase(
        &mut self,
        phase: transaction_review::PostCommitPhase,
    ) {
        self.push_and_switch(SuspendTarget::TransactionReview(
            transaction_review::TransactionReviewState::new()
                .with_post_commit_phase(phase),
        ));
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
