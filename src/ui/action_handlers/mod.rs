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
pub(crate) mod witness;

use crate::meta::decisions::{DecisionKey, DecisionSource};
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
    /// gesture (Enter keypress). The witness is minted internally from this
    /// flag - callers never touch the ConfirmationGesture type.
    pub(in crate::ui) fn dispatch_action(&mut self, action: ViewAction, is_confirmation: bool) {
        let witness = if is_confirmation {
            Some(witness::ConfirmationGesture::new())
        } else {
            None
        };

        match action {
            ViewAction::None => {}
            ViewAction::MigrationApproval(a) => self.handle_migration_approval_action(a, witness.as_ref()),
            ViewAction::VacuumPrompt(a) => self.handle_vacuum_prompt_action(a, witness.as_ref()),
            ViewAction::ConfigEditor(a) => self.handle_config_editor_action(a, witness.as_ref()),
            ViewAction::Insights(a) => self.handle_insights_action(a),
            ViewAction::CorpusBrowser(a) => self.handle_tree_browser_action(a),
            ViewAction::TagSearch(a) => self.handle_tag_search_action(a),
            ViewAction::Inbox(a) => self.handle_inbox_action(a, witness.as_ref()),
            ViewAction::Transaction(a) => self.handle_transaction_view_action(a, witness.as_ref()),
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
    fn stage_tag_editor_decision(&mut self, key: DecisionKey, mutations: Vec<crate::meta::mutations::Mutation>, gesture: &witness::ConfirmationGesture) {
        let label = if let ActiveView::UnifiedTagEditor(ref editor) = self.view {
            editor.current_item_label()
        } else {
            "Tag edit".to_string()
        };
        let _ = super::operator_decisions::stage_decision(&mut self.witch, key, &label, mutations.clone(), gesture);
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
    ///
    /// In open-txn mode, skips `start_transaction` since the persistent transaction
    /// is already active.
    fn stage_mutations_with_transaction(&mut self, mutations: Vec<crate::meta::mutations::Mutation>, label: &str, key: DecisionKey, gesture: &witness::ConfirmationGesture) {
        let open_txn = self.open_txn_mode();
        if !open_txn {
            let _ = self.witch.start_transaction(label);
        }
        let _ = super::operator_decisions::stage_decision(&mut self.witch, key, label, mutations, gesture);
    }

    /// Cancel the current modal and return to the source view.
    ///
    /// In default (closed-txn) mode: discards any open transaction before returning.
    /// In open-txn mode: leaves the persistent transaction intact.
    pub(in crate::ui) fn cancel_and_return_to_source(&mut self, log_message: &str) {
        crate::logging::log_general(log_message);
        if !self.open_txn_mode() {
            if self.witch.has_transaction() {
                let _ = super::operator_decisions::discard_transaction(&mut self.witch);
            }
        }
        self.return_to_last_lateral_view();
    }

    /// After staging decisions: route to review (closed-txn) or return to source (open-txn).
    ///
    /// In default mode: shows the TransactionReview modal.
    /// In open-txn mode: returns to the last lateral view with a status message.
    pub(in crate::ui) fn after_staging_decisions(&mut self) {
        if self.open_txn_mode() {
            self.start_transaction_view();
            self.status_message = Some("Decision staged".into());
        } else {
            self.start_transaction_review();
        }
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
    // Startup Action Handlers
    // =========================================================================

    /// Handle migration approval actions.
    fn handle_migration_approval_action(
        &mut self,
        action: super::MigrationAction,
        _witness: Option<&witness::ConfirmationGesture>,
    ) {
        use super::MigrationPhase;
        match action {
            super::MigrationAction::None => {}
            super::MigrationAction::Approve => {
                if let super::ActiveView::MigrationApproval(ref mut state) = self.view {
                    if state.phase == MigrationPhase::Approval {
                                self.witch.queue_pending_migrations();
                        state.phase = MigrationPhase::Running;
                    }
                }
            }
            super::MigrationAction::Cancel => {
                self.should_quit = true;
            }
        }
    }

    /// Handle vacuum prompt actions.
    fn handle_vacuum_prompt_action(
        &mut self,
        action: super::VacuumAction,
        _witness: Option<&witness::ConfirmationGesture>,
    ) {
        use super::VacuumPhase;
        match action {
            super::VacuumAction::None => {}
            super::VacuumAction::Compact => {
                if let super::ActiveView::VacuumPrompt(ref mut state) = self.view {
                    if state.phase == VacuumPhase::Prompt {
                        state.phase = VacuumPhase::Compacting;
                    }
                }
            }
            super::VacuumAction::Skip => {
                self.complete_startup();
            }
        }
    }

    // =========================================================================
    // View Action Handlers
    // =========================================================================

    fn handle_config_editor_action(&mut self, action: super::config_editor::ConfigEditorAction, gesture: Option<&witness::ConfirmationGesture>) {
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
                    let Some(g) = gesture else { return };

                    let mutation = Mutation::ApplyConfigEdits(ApplyConfigEditsMutation {
                        original_kdl,
                        old_config,
                        new_config,
                    });

                    let open_txn = self.open_txn_mode();
                    if !open_txn {
                        let _ = self.witch.start_transaction("Config update");
                    }
                    let _ = super::operator_decisions::stage_decision(
                        &mut self.witch, DecisionKey::single(DecisionSource::ConfigEdit), "Apply config changes", vec![mutation], g,
                    );

                    self.after_staging_decisions();
                } else {
                    // No edits — just return to insights
                    self.start_insights_view();
                }
            }
            super::config_editor::ConfigEditorAction::Discard => {
                self.start_insights_view();
            }
            super::config_editor::ConfigEditorAction::CycleNext => {
                self.start_lateral_view(widgets::LateralView::Config.next(self.transactions_open()));
            }
            super::config_editor::ConfigEditorAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::Config.prev(self.transactions_open()));
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
                self.start_lateral_view(widgets::LateralView::Insights.next(self.transactions_open()));
            }
            insights_view::InsightsAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::Insights.prev(self.transactions_open()));
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
        let intake_state = {
            let read_db = self.witch.read_db();
            startup::IntakeConfirmationState::gather(&read_db, &corpus_root, "insights")
        };

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
        let _ = self.witch.start_transaction("Missing album singles");

        let state = missing_album_modal::MissingAlbumState::new(data, suffix);
        self.view = ActiveView::MissingAlbumSingleResolution(state);
    }

    /// Handle missing album single resolution actions.
    fn handle_missing_album_single_action(
        &mut self,
        action: crate::ui::missing_album_modal::MissingAlbumAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        use crate::corpus::db::types::Zone;
        use crate::meta::mutations::{Mutation, TagOp, tag_edit::ApplyTagOpsMutation, indexing::EmitExpectedMissingTagMutation};
        use crate::ui::missing_album_modal::{MissingAlbumAction, AlbumResolution};

        match action {
            MissingAlbumAction::None => {}

            MissingAlbumAction::Cancel => {
                self.cancel_and_return_to_source("Missing album single resolution cancelled");
            }

            MissingAlbumAction::Confirm(resolution) => {
                let Some(g) = witness else { return };

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
                            let _ = super::operator_decisions::stage_decision(
                                &mut self.witch, DecisionKey::new(DecisionSource::MissingAlbum, group_idx.to_string()), "Tag as singles", vec![mutation], g,
                            );
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
                            let _ = super::operator_decisions::stage_decision(
                                &mut self.witch, DecisionKey::new(DecisionSource::MissingAlbum, group_idx.to_string()), "Tag all as Singles", vec![mutation], g,
                            );
                        }
                    }

                    AlbumResolution::Suppress => {
                        let inodes: Vec<i64> = {
                            let ActiveView::MissingAlbumSingleResolution(ref state) = self.view else { return };
                            state.current_group_inodes()
                        };
                        if !inodes.is_empty() {
                            let mutation = Mutation::EmitExpectedMissingTag(EmitExpectedMissingTagMutation { inodes });
                            let _ = super::operator_decisions::stage_decision(
                                &mut self.witch, DecisionKey::new(DecisionSource::MissingAlbum, group_idx.to_string()), "Suppress missing album", vec![mutation], g,
                            );
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
                        self.after_staging_decisions();
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
                self.after_staging_decisions();
            }

            MissingAlbumAction::EditTracks => {
                let (inodes, decision_key, label) = {
                    let ActiveView::MissingAlbumSingleResolution(ref state) = self.view else { return };
                    let group = match state.current_group_data() {
                        Some(g) => g,
                        None => return,
                    };
                    // Use a distinct key to avoid colliding with resolution decisions
                    let key = DecisionKey::new(DecisionSource::TagEdit, format!("missing_album_{}", state.current_group));
                    let label = format!("Manual tag edits: {}", group.artist);
                    (state.current_group_inodes(), key, label)
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
                        decision_key,
                        label,
                    );
                }
            }

            MissingAlbumAction::EditTracksAggregated => {
                let (inodes, decision_key, label) = {
                    let ActiveView::MissingAlbumSingleResolution(ref state) = self.view else { return };
                    let group = match state.current_group_data() {
                        Some(g) => g,
                        None => return,
                    };
                    let key = DecisionKey::new(DecisionSource::TagEdit, format!("missing_album_{}", state.current_group));
                    let label = format!("Manual tag edits: {}", group.artist);
                    (state.current_group_inodes(), key, label)
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
                        decision_key,
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
                self.start_lateral_view(widgets::LateralView::TagSearch.next(self.transactions_open()));
            }
            tag_search::TagSearchAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::TagSearch.prev(self.transactions_open()));
            }
            tag_search::TagSearchAction::ExecuteSearch => {
                // Execute search - access witch and view as disjoint fields
                if let ActiveView::TagSearch(ref mut search) = self.view {
                    let read_db = self.witch.read_db();
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
    fn handle_intake_confirmation_action(&mut self, action: startup::IntakeConfirmationAction, gesture: Option<&witness::ConfirmationGesture>) {
        use super::operator_decisions;

        // Determine zone before matching (used for post-action routing)
        let is_inbox_zone = matches!(
            self.view,
            ActiveView::IntakeConfirmation(ref s) if s.zone == "inbox"
        );

        match action {
            startup::IntakeConfirmationAction::None => {}
            startup::IntakeConfirmationAction::Confirmed => {
                let Some(g) = gesture else { return };

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
                    let open_txn = self.open_txn_mode();
                    if !open_txn {
                        let _ = self.witch.start_transaction("Intake indexing");
                    }
                    let _ = operator_decisions::stage_decision(
                        &mut self.witch,
                        DecisionKey::single(DecisionSource::IntakeIndex),
                        "Index unindexed files",
                        mutations,
                        g,
                    );

                    if open_txn {
                        self.start_transaction_view();
                        self.status_message = Some(format!("{} files staged for indexing", count));
                    } else {
                        // Note: IntakeConfirmation state is preserved inside the suspended view for Cancel return
                        // Transition to review modal with ContentAnalysis phase for post-commit
                        self.start_transaction_review_with_phase(
                            transaction_review::PostCommitPhase::ContentAnalysis,
                        );
                    }
                }
            }
            startup::IntakeConfirmationAction::Skipped => {
                // User skipped - no mutations ran
                // Unindexed signals remain for later handling
                crate::logging::log_general("IntakeConfirmation: user skipped indexing");

                // Discard any active transaction from review modal (not in open-txn mode)
                if !self.open_txn_mode() {
                    if self.witch.has_transaction() {
                        let _ = operator_decisions::discard_transaction(&mut self.witch);
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
                self.start_lateral_view(widgets::LateralView::CorpusBrowser.next(self.transactions_open()));
            }
            tree_browser::TreeBrowserAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::CorpusBrowser.prev(self.transactions_open()));
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

    fn handle_unified_tag_editor_action(&mut self, action: tag_editor::UnifiedTagEditorAction, witness: Option<&witness::ConfirmationGesture>) {
        use tag_editor::UnifiedTagEditorAction;

        match action {
            UnifiedTagEditorAction::None => {}
            UnifiedTagEditorAction::CloseModal => {}

            UnifiedTagEditorAction::StageDecisionAndNavigate { key, mutations, direction } => {
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
                    self.stage_tag_editor_decision(key, mutations, w);
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

            UnifiedTagEditorAction::StageDecisionAndReview { key, mutations } => {
                let Some(w) = witness else { return };
                // Stage the decision AND immediately show transaction review
                // Used for aggregated mode or single-item contexts
                self.stage_tag_editor_decision(key, mutations, w);

                // Transition to standardized review modal
                // Note: tag editor state is preserved on the view stack for Cancel return
                self.after_staging_decisions();
            }

            UnifiedTagEditorAction::DiscardTransaction => {
                // In closed-txn mode, discard the transaction.
                // In open-txn mode, leave the persistent transaction intact.
                if !self.open_txn_mode() {
                    let _ = super::operator_decisions::discard_transaction(&mut self.witch);
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

            UnifiedTagEditorAction::StageAndCloseEmbedded { decision_key, decision_label, mutations } => {
                let Some(g) = witness else { return };
                // Stage collected mutations at parent's decision key
                let _ = super::operator_decisions::stage_decision(
                    &mut self.witch, decision_key, &decision_label, mutations, g,
                );
                // Return to parent health modal
                if !self.pop_and_restore() {
                    self.start_insights_view();
                }
            }

            UnifiedTagEditorAction::RequestTransactionReview => {
                // Transition to standardized review modal
                // Note: tag editor state is preserved on the view stack for Cancel return
                self.after_staging_decisions();
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
    fn handle_transaction_review_action(&mut self, action: transaction_review::TransactionReviewAction, gesture: Option<&witness::ConfirmationGesture>) {
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
                let _ = super::operator_decisions::discard_transaction(&mut self.witch);
                self.start_insights_view();
                self.status_message = Some("Transaction discarded".to_string());
            }

            TransactionReviewAction::Confirm => {
                let Some(g) = gesture else { return };

                // Determine progress phase before clearing state
                let post_commit_phase = if let ActiveView::TransactionReview(ref review) = self.view {
                    review.post_commit_phase
                } else {
                    transaction_review::PostCommitPhase::default()
                };

                // Clear entire view stack — commit is a hard navigation
                self.clear_view_stack();

                // Commit transaction
                let commit_result = super::operator_decisions::commit_transaction(&mut self.witch, g);

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

            TransactionReviewAction::RequestRemoval => {
                // Map cursor position to DecisionKey and set pending_removal
                let key = if let ActiveView::TransactionReview(ref review) = self.view {
                    let keys = self.witch.decision_keys();
                    let cursor = review.cursor.min(keys.len().saturating_sub(1));
                    keys.into_iter().nth(cursor)
                } else {
                    None
                };
                if let Some(key) = key {
                    if let ActiveView::TransactionReview(ref mut review) = self.view {
                        review.pending_removal = Some(key);
                    }
                }
            }

            TransactionReviewAction::ConfirmRemoval(key) => {
                let Some(g) = gesture else { return };
                let _ = super::operator_decisions::remove_decision(&mut self.witch, &key, g);

                // If transaction is now empty, auto-close review
                if self.witch.decision_keys().is_empty() {
                    if !self.pop_and_restore() {
                        self.start_insights_view();
                    }
                    self.status_message = Some("Decision removed, transaction empty".to_string());
                    return;
                }
                // Clamp cursor after removal
                if let ActiveView::TransactionReview(ref mut review) = self.view {
                    let count = self.witch.decision_keys().len();
                    if review.cursor >= count && count > 0 {
                        review.cursor = count - 1;
                    }
                }
                self.status_message = Some("Decision removed".to_string());
            }
        }
    }

    // ========================================================================
    // Transaction Tab View
    // ========================================================================

    /// Handle actions from the Transaction lateral tab view.
    fn handle_transaction_view_action(&mut self, action: super::transaction_view::TransactionViewAction, gesture: Option<&witness::ConfirmationGesture>) {
        use super::transaction_view::TransactionViewAction;
        match action {
            TransactionViewAction::None => {}
            TransactionViewAction::CycleNext => {
                self.start_lateral_view(widgets::LateralView::Transaction.next(self.transactions_open()));
            }
            TransactionViewAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::Transaction.prev(self.transactions_open()));
            }
            TransactionViewAction::Commit => {
                let Some(g) = gesture else { return };
                let _ = super::operator_decisions::commit_transaction(&mut self.witch, g);
                // Re-open transaction immediately
                let _ = self.witch.start_transaction("Open");
                self.transition_to_progress_after_mutations(
                    super::progress_screen::ProgressPhase::SignalRefresh,
                );
            }
            TransactionViewAction::DiscardAll => {
                let _ = super::operator_decisions::discard_transaction(&mut self.witch);
                // Re-open transaction immediately
                let _ = self.witch.start_transaction("Open");
                self.status_message = Some("Transaction discarded".into());
            }
            TransactionViewAction::RequestRemoval => {
                if let ActiveView::Transaction(ref mut state) = self.view {
                    let decisions = transaction_review::fetch_decision_summaries(&self.witch);
                    if let Some(d) = decisions.get(state.cursor) {
                        state.pending_removal = Some(d.key.clone());
                    }
                }
            }
            TransactionViewAction::RemoveDecision(key) => {
                let Some(g) = gesture else { return };
                let _ = super::operator_decisions::remove_decision(&mut self.witch, &key, g);
                self.status_message = Some("Decision removed".into());
                // Clamp cursor
                if let ActiveView::Transaction(ref mut state) = self.view {
                    let remaining = transaction_review::fetch_decision_summaries(&self.witch).len();
                    if state.cursor >= remaining && remaining > 0 {
                        state.cursor = remaining - 1;
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
        let click_witness = witness::ConfirmationGesture::new();

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
