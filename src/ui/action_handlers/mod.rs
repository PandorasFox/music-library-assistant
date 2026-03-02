//! Action Handlers for View-Specific Events
//!
//! Each view (health, search, tree browser, tag editor, etc.) emits
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
mod external_match;
mod history;
mod tag_canonicity;
pub(crate) mod witness;

use crate::meta::decisions::DecisionKey;
use crate::ui::{filter_popup, insights_view, progress_screen, tag_search, transaction_review, tree_browser, tag_editor, startup, widgets};
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
            ViewAction::SchemaUpdate(a) => self.handle_schema_update_action(a, witness.as_ref()),
            ViewAction::VacuumPrompt(a) => self.handle_vacuum_prompt_action(a, witness.as_ref()),
            ViewAction::ConfigEditor(a) => self.handle_config_editor_action(a, witness.as_ref()),
            ViewAction::Insights(a) => self.handle_health_action(a),
            ViewAction::CorpusBrowser(a) => self.handle_tree_browser_action(a, witness.as_ref()),
            ViewAction::TagSearch(a) => self.handle_tag_search_action(a),
            ViewAction::Inbox(a) => self.handle_inbox_action(a, witness.as_ref()),
            ViewAction::TabbedTransactionReview(a) => self.handle_tabbed_transaction_review_action(a, witness.as_ref()),
            ViewAction::ExitConfirm(a) => self.handle_exit_confirm_action(a),
            ViewAction::IntakeConfirmation(a) => self.handle_intake_confirmation_action(a, witness.as_ref()),
            ViewAction::UnifiedTagEditor(a) => self.handle_unified_tag_editor_action(a, witness.as_ref()),
            ViewAction::Deploy(a) => self.handle_deploy_action(a, witness.as_ref()),
            ViewAction::ExternalMatches(a) => self.handle_external_matches_action(a),
            ViewAction::MissingFileResolution(a) => self.handle_missing_file_preview_action(a, witness.as_ref()),
            ViewAction::MissingDirectoryResolution(a) => self.handle_missing_directory_preview_action(a, witness.as_ref()),
            ViewAction::CorruptFileResolution(a) => self.handle_corrupt_file_preview_action(a, witness.as_ref()),
            ViewAction::ShitFormatResolution(a) => self.handle_shit_format_preview_action(a, witness.as_ref()),
            ViewAction::EmbedAlbumArtResolution(a) => self.handle_album_art_review_action(a, witness.as_ref()),
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
            ViewAction::DiscExtractionResolution(a) => self.handle_disc_extraction_action(a, witness.as_ref()),
            ViewAction::ManualReview(a) => self.handle_manual_review_action(a, witness.as_ref()),
            ViewAction::ExternalMatchReview(a) => self.handle_external_match_review_action(a, witness.as_ref()),
            ViewAction::History(a) => self.handle_history_action(a, witness.as_ref()),
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
            self.start_tabbed_transaction_review();
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

    /// Handle schema update approval actions.
    fn handle_schema_update_action(
        &mut self,
        action: super::SchemaUpdateAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        use super::SchemaUpdatePhase;
        match action {
            super::SchemaUpdateAction::None => {}
            super::SchemaUpdateAction::Approve => {
                if let (super::ActiveView::SchemaUpdate(ref mut state), Some(gesture)) = (&mut self.view, witness) {
                    if state.phase == SchemaUpdatePhase::Approval {
                        self.witch.queue_schema_reconciliation(gesture);
                        state.phase = SchemaUpdatePhase::Running;
                    }
                }
            }
            super::SchemaUpdateAction::Cancel => {
                self.should_quit = true;
            }
        }
    }

    /// Handle vacuum prompt actions.
    fn handle_vacuum_prompt_action(
        &mut self,
        action: super::VacuumAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        use super::VacuumPhase;
        match action {
            super::VacuumAction::None => {}
            super::VacuumAction::Compact => {
                if let (super::ActiveView::VacuumPrompt(ref mut state), Some(gesture)) = (&mut self.view, witness) {
                    if state.phase == VacuumPhase::Prompt {
                        self.witch.queue_vacuum(gesture);
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
                        &mut self.witch, DecisionKey::ConfigEdit, "Apply config changes", vec![mutation], g,
                    );

                    self.after_staging_decisions();
                } else {
                    // No edits — just return to health
                    self.start_health_view();
                }
            }
            super::config_editor::ConfigEditorAction::Discard => {
                self.start_health_view();
            }
            super::config_editor::ConfigEditorAction::CycleNext => {
                self.start_lateral_view(widgets::LateralView::Config.next(self.transactions_open()));
            }
            super::config_editor::ConfigEditorAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::Config.prev(self.transactions_open()));
            }
        }
    }

    pub(super) fn handle_health_action(&mut self, action: insights_view::InsightsAction) {
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
                self.start_lateral_view(widgets::LateralView::Health.next(self.transactions_open()));
            }
            insights_view::InsightsAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::Health.prev(self.transactions_open()));
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
                        self.start_intake_confirmation_from_health();
                    }
                    Some(insights_view::InsightAction::LaunchDirectoryOverlapResolution) => {
                        self.start_directory_overlap_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchReleaseOverlapResolution) => {
                        self.start_release_overlap_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchSubparDuplicateResolution) => {
                        self.start_subpar_duplicate_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchAlbumArtReview) => {
                        self.start_album_art_review();
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
                    Some(insights_view::InsightAction::LaunchDiscExtractionResolution) => {
                        self.start_disc_extraction_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchPathTagMismatchResolution) => {
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
    fn start_intake_confirmation_from_health(&mut self) {
        let corpus_root = self.config().corpus_dir();
        let intake_state = self.cache.query(move |db| {
            startup::IntakeConfirmationState::gather(&db, &corpus_root, startup::IntakeSource::Health)
        }).recv();

        if let Some(state) = intake_state {
            self.view = ActiveView::IntakeConfirmation(state);
        }
    }

    /// Start missing tag resolution from Insights view.
    ///
    /// Loads all MissingTag signals, collects unique inodes, and opens
    /// the bulk tag editor so the operator can fill in missing tags.
    fn start_missing_tag_resolution(&mut self) {
        use crate::db::types::Zone;
        use std::collections::BTreeSet;

        let audio_files = self.cache.query(|db| {
            let signals = db.get_missing_tag_signals().unwrap_or_default();

            // Collect all unique inodes across all signal groups
            let all_inodes: Vec<i64> = signals.iter()
                .flat_map(|s| s.data.inodes.iter().copied())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();

            db.get_audio_files_by_inodes(&all_inodes, Zone::Corpus)
                .unwrap_or_default()
        }).recv();

        if audio_files.is_empty() {
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

        let signals = self.cache.query(|db| {
            db.get_missing_album_single_signals().unwrap_or_default()
        }).recv();

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
        use crate::db::types::Zone;
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
                                &mut self.witch, DecisionKey::MissingAlbum { group_index: group_idx }, "Tag as singles", vec![mutation], g,
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
                                &mut self.witch, DecisionKey::MissingAlbum { group_index: group_idx }, "Tag all as Singles", vec![mutation], g,
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
                                &mut self.witch, DecisionKey::MissingAlbum { group_index: group_idx }, "Suppress missing album", vec![mutation], g,
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
                    let key = DecisionKey::TagEdit { key_item: format!("missing_album_{}", state.current_group) };
                    let label = format!("Manual tag edits: {}", group.artist);
                    (state.current_group_inodes(), key, label)
                };
                if inodes.is_empty() {
                    return;
                }
                let audio_files = self.cache.query(move |db| {
                    db.get_audio_files_by_inodes(
                        &inodes,
                        crate::db::types::Zone::Corpus,
                    ).unwrap_or_default()
                }).recv();
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
                    let key = DecisionKey::TagEdit { key_item: format!("missing_album_{}", state.current_group) };
                    let label = format!("Manual tag edits: {}", group.artist);
                    (state.current_group_inodes(), key, label)
                };
                if inodes.is_empty() {
                    return;
                }
                let audio_files = self.cache.query(move |db| {
                    db.get_audio_files_by_inodes(
                        &inodes,
                        crate::db::types::Zone::Corpus,
                    ).unwrap_or_default()
                }).recv();
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

    /// Start disc extraction resolution from Insights view.
    ///
    /// Loads DiscExtraction signals, resolves file paths, builds modal data,
    /// starts a transaction, and switches to the DiscExtractionResolution view.
    fn start_disc_extraction_resolution(&mut self) {
        use crate::ui::disc_extraction_modal;

        let config = self.config().opinions.disc_extraction.clone();

        let (signals, path_map) = self.cache.query(|db| {
            let sigs = db.get_disc_extraction_signals().unwrap_or_default();
            // Collect all inodes for path lookup
            let all_inodes: Vec<i64> = sigs.iter()
                .flat_map(|s| s.data.inodes.iter().copied())
                .collect();
            let paths = db.get_file_paths_batch(crate::db::types::Zone::Corpus, &all_inodes)
                .unwrap_or_default();
            (sigs, paths)
        }).recv();

        if signals.is_empty() {
            self.status_message = Some("No disc extraction signals found".to_string());
            return;
        }

        let data = disc_extraction_modal::DiscExtractionData::from_signals(
            signals,
            |inode| path_map.get(&inode).cloned().unwrap_or_else(|| format!("<inode {}>", inode)),
            config.map_letters_to_numbers,
        );

        // Start transaction for the resolution session
        let _ = self.witch.start_transaction("Disc extraction");

        let state = disc_extraction_modal::DiscExtractionState::new(data, config.disc_tag_name);
        self.view = ActiveView::DiscExtractionResolution(state);
    }

    /// Handle disc extraction resolution actions.
    fn handle_disc_extraction_action(
        &mut self,
        action: crate::ui::disc_extraction_modal::DiscExtractionAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        use crate::db::types::Zone;
        use crate::meta::mutations::{Mutation, TagOp, tag_edit::ApplyTagOpsMutation};
        use crate::ui::disc_extraction_modal::{DiscExtractionAction, DiscResolution};

        match action {
            DiscExtractionAction::None => {}

            DiscExtractionAction::Cancel => {
                self.cancel_and_return_to_source("Disc extraction resolution cancelled");
            }

            DiscExtractionAction::Confirm(resolution) => {
                let Some(g) = witness else { return };

                let group_idx = {
                    let ActiveView::DiscExtractionResolution(ref state) = self.view else { return };
                    state.current_group
                };

                match resolution {
                    DiscResolution::Apply => {
                        let ops: Vec<TagOp> = {
                            let ActiveView::DiscExtractionResolution(ref state) = self.view else { return };
                            let Some(group) = state.current_group_data() else { return };
                            let mut ops = Vec::new();
                            for file in &group.files {
                                // Replace source tag value
                                ops.push(TagOp::replace_tag(
                                    file.inode,
                                    &file.source_tag,
                                    &file.original_value,
                                    &file.cleaned_value,
                                ));
                                // Add disc number tag
                                ops.push(TagOp::add_tag(
                                    file.inode,
                                    &state.disc_tag_name,
                                    &group.disc_value,
                                ));
                            }
                            ops
                        };
                        if !ops.is_empty() {
                            let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation { ops, zone: Zone::Corpus });
                            let _ = super::operator_decisions::stage_decision(
                                &mut self.witch, DecisionKey::DiscExtraction { group_index: group_idx },
                                "Extract disc value", vec![mutation], g,
                            );
                        }
                    }
                    DiscResolution::Skip => {
                        // No mutations for skip — just advance
                    }
                }

                // Advance to next group, or show review if at end
                if let ActiveView::DiscExtractionResolution(ref mut state) = self.view {
                    state.file_cursor = 0;
                    state.file_scroll = 0;
                    if state.current_group + 1 < state.data.groups.len() {
                        state.current_group += 1;
                    } else {
                        self.after_staging_decisions();
                        return;
                    }
                }
            }

            DiscExtractionAction::NavigateGroup(forward) => {
                if let ActiveView::DiscExtractionResolution(ref mut state) = self.view {
                    if forward {
                        if state.current_group + 1 < state.data.groups.len() {
                            state.current_group += 1;
                            state.file_cursor = 0;
                            state.file_scroll = 0;
                        }
                    } else if state.current_group > 0 {
                        state.current_group -= 1;
                        state.file_cursor = 0;
                        state.file_scroll = 0;
                    }
                }
            }

            DiscExtractionAction::ShowReview => {
                self.after_staging_decisions();
            }

            DiscExtractionAction::EditTracks => {
                let (inodes, decision_key, label) = {
                    let ActiveView::DiscExtractionResolution(ref state) = self.view else { return };
                    let group = match state.current_group_data() {
                        Some(g) => g,
                        None => return,
                    };
                    let key = DecisionKey::TagEdit { key_item: format!("disc_extraction_{}", state.current_group) };
                    let label = format!("Manual tag edits: {}", group.description);
                    (state.current_group_inodes(), key, label)
                };
                if inodes.is_empty() {
                    return;
                }
                let audio_files = self.cache.query(move |db| {
                    db.get_audio_files_by_inodes(
                        &inodes,
                        crate::db::types::Zone::Corpus,
                    ).unwrap_or_default()
                }).recv();
                if !audio_files.is_empty() {
                    self.open_embedded_tag_editor(
                        tag_editor::TagEditorMode::Individual,
                        audio_files,
                        decision_key,
                        label,
                    );
                }
            }

            DiscExtractionAction::EditTracksAggregated => {
                let (inodes, decision_key, label) = {
                    let ActiveView::DiscExtractionResolution(ref state) = self.view else { return };
                    let group = match state.current_group_data() {
                        Some(g) => g,
                        None => return,
                    };
                    let key = DecisionKey::TagEdit { key_item: format!("disc_extraction_{}", state.current_group) };
                    let label = format!("Manual tag edits: {}", group.description);
                    (state.current_group_inodes(), key, label)
                };
                if inodes.is_empty() {
                    return;
                }
                let audio_files = self.cache.query(move |db| {
                    db.get_audio_files_by_inodes(
                        &inodes,
                        crate::db::types::Zone::Corpus,
                    ).unwrap_or_default()
                }).recv();
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
                self.start_health_view();
            }
            tag_search::TagSearchAction::CycleNext => {
                self.start_lateral_view(widgets::LateralView::Search.next(self.transactions_open()));
            }
            tag_search::TagSearchAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::Search.prev(self.transactions_open()));
            }
            tag_search::TagSearchAction::ExecuteSearch => {
                // Execute search via cache thread (blocking — fast single query)
                if let ActiveView::TagSearch(ref mut search) = self.view {
                    let all_files = self.cache.query(|db| {
                        db.get_all_audio_files_with_tags(crate::db::types::Zone::Corpus)
                            .unwrap_or_default()
                    }).recv();
                    search.execute_search(all_files);
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

        // Determine source before matching (used for post-action routing)
        let is_inbox_source = matches!(
            self.view,
            ActiveView::IntakeConfirmation(ref s) if s.source == startup::IntakeSource::Inbox
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
                    if is_inbox_source {
                        self.view = ActiveView::Inbox(super::inbox_view::InboxViewState::new());
                    } else {
                        self.start_health_view();
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
                        DecisionKey::IntakeIndex,
                        "Index unindexed files",
                        mutations,
                        g,
                    );

                    if open_txn {
                        self.start_tabbed_transaction_review();
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

                if is_inbox_source {
                    self.view = ActiveView::Inbox(super::inbox_view::InboxViewState::new());
                } else {
                    self.start_health_view();
                }
            }
        }
    }

    pub(super) fn handle_tree_browser_action(&mut self, action: tree_browser::TreeBrowserAction, witness: Option<&witness::ConfirmationGesture>) {
        match action {
            tree_browser::TreeBrowserAction::None => {}
            tree_browser::TreeBrowserAction::Cancel => {
                self.start_health_view();
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
                self.start_lateral_view(widgets::LateralView::Files.next(self.transactions_open()));
            }
            tree_browser::TreeBrowserAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::Files.prev(self.transactions_open()));
            }
            tree_browser::TreeBrowserAction::OpenFilter => {
                // Open filter popup for corpus browser
                self.filter_overlay = Some(FilterOverlay {
                    state: filter_popup::FilterPopupState::new(),
                    context: FilterPopupContext::CorpusBrowser,
                });
            }
            tree_browser::TreeBrowserAction::OpenDirConfig(path) => {
                self.open_dir_config_panel(path);
            }
            tree_browser::TreeBrowserAction::SaveDirConfig => {
                if let Some(gesture) = witness {
                    self.save_dir_config(gesture);
                }
            }
            tree_browser::TreeBrowserAction::CloseDirConfig => {
                self.close_dir_config_panel();
            }
            tree_browser::TreeBrowserAction::ReviewTransaction => {
                self.after_staging_decisions();
            }
        }
    }

    /// Open a dir config panel for the given absolute corpus directory path.
    ///
    /// If an exact SourceDir match exists, loads its values. Otherwise opens
    /// panel with defaults so the user can create a new config entry.
    fn open_dir_config_panel(&mut self, abs_path: std::path::PathBuf) {
        let config = self.config();
        let corpus_dir = config.corpus_dir();

        // Strip corpus_dir prefix to get relative path
        let relative = match abs_path.strip_prefix(&corpus_dir) {
            Ok(r) => r.to_path_buf(),
            Err(_) => return,
        };

        // Find exact matching SourceDir (raw, with Option fields) for editing.
        // We show what THIS dir explicitly sets, not the resolved/inherited values.
        let (libraries, can_stash_dupes, interior_dupes, path_schema, enable_acoustid) =
            match config.get_raw_source_dir(&relative) {
                Some(sd) => {
                    (sd.libraries.clone(), sd.can_stash_dupes, sd.interior_dupes,
                     sd.path_schema.as_ref().map(|s| s.template.clone()), sd.enable_acoustid)
                }
                None => {
                    // Defaults for a new (unconfigured) directory
                    (vec![], None, None, None, None)
                }
            };
        drop(config);

        let panel = tree_browser::variants::corpus::DirConfigPanelState {
            source_path: relative,
            libraries: libraries.clone(),
            can_stash_dupes,
            interior_dupes,
            path_schema: path_schema.clone(),
            enable_acoustid,
            orig_libraries: libraries,
            orig_can_stash_dupes: can_stash_dupes,
            orig_interior_dupes: interior_dupes,
            orig_path_schema: path_schema,
            orig_enable_acoustid: enable_acoustid,
            field_cursor: 0,
            focus: tree_browser::variants::corpus::PanelFocus::default(),
            button_cursor: 0,
            lib_cursor: None,
            text_input: None,
        };

        if let super::active_view::ActiveView::CorpusBrowser(ref mut browser) = self.view {
            browser.set_config_panel(panel);
        }
    }

    /// Save dir config edits and stage decision.
    fn save_dir_config(&mut self, gesture: &witness::ConfirmationGesture) {
        // Extract panel data from the browser view
        let (source_path, old_dir, new_dir) = {
            let panel = match self.view {
                super::active_view::ActiveView::CorpusBrowser(ref browser) => {
                    match browser.config_panel() {
                        Some(p) => p,
                        None => return,
                    }
                }
                _ => return,
            };

            if !panel.has_edits() {
                // No edits, just close
                self.close_dir_config_panel();
                return;
            }

            let old_dir = crate::config::SourceDir {
                path: panel.source_path.clone(),
                libraries: panel.orig_libraries.clone(),
                can_stash_dupes: panel.orig_can_stash_dupes,
                interior_dupes: panel.orig_interior_dupes,
                path_schema: panel.orig_path_schema.as_ref()
                    .and_then(|t| crate::config::parse_path_schema(t).ok()),
                enable_acoustid: panel.orig_enable_acoustid,
            };
            let new_dir = crate::config::SourceDir {
                path: panel.source_path.clone(),
                libraries: panel.libraries.clone(),
                can_stash_dupes: panel.can_stash_dupes,
                interior_dupes: panel.interior_dupes,
                path_schema: panel.path_schema.as_ref()
                    .and_then(|t| crate::config::parse_path_schema(t).ok()),
                enable_acoustid: panel.enable_acoustid,
            };
            (panel.source_path.clone(), old_dir, new_dir)
        };

        // Construct the full new Config with the dir edit applied,
        // so the Witch can update SharedConfig in-memory after execution.
        // Default entries (all-defaults, no meaningful config) are elided —
        // they carry no information and will be dropped from dirs.kdl on write.
        let new_config = {
            let mut cfg = self.config().clone();
            let mut found = false;
            for sd in &mut cfg.source_dirs {
                if sd.path == source_path {
                    *sd = new_dir.clone();
                    found = true;
                    break;
                }
            }
            if !found {
                cfg.source_dirs.push(new_dir.clone());
            }
            cfg.source_dirs.retain(|sd| !sd.is_default());
            cfg
        };

        let mutation = crate::meta::mutations::Mutation::ApplyDirConfigEdit(
            crate::meta::mutations::dir_config_edit::ApplyDirConfigEditMutation {
                source_path: source_path.clone(),
                old_dir,
                new_dir,
                new_config,
            },
        );

        let key = DecisionKey::DirConfigEdit {
            source_path: source_path.clone(),
        };
        let label = format!("Dir config: {}", source_path.display());

        let open_txn = self.open_txn_mode();
        if !open_txn {
            let _ = self.witch.start_transaction(&label);
        }
        let _ = super::operator_decisions::stage_decision(
            &mut self.witch,
            key,
            &label,
            vec![mutation],
            gesture,
        );

        // Close panel and stay in browser for batch editing
        self.close_dir_config_panel();
        self.sync_browser_pending_edits();
        self.status_message = Some(format!("Dir config staged: {}", source_path.display()));
    }

    /// Close the dir config panel.
    fn close_dir_config_panel(&mut self) {
        if let super::active_view::ActiveView::CorpusBrowser(ref mut browser) = self.view {
            browser.clear_config_panel();
        }
    }

    /// Sync browser's pending-edit markers from the current transaction's DirConfigEdit decisions.
    fn sync_browser_pending_edits(&mut self) {
        let mut pending = std::collections::HashSet::new();
        for key in self.witch.decision_keys() {
            if let DecisionKey::DirConfigEdit { source_path } = key {
                pending.insert(source_path);
            }
        }
        if let super::active_view::ActiveView::CorpusBrowser(ref mut browser) = self.view {
            browser.set_pending_edit_paths(pending);
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
                    self.start_health_view();
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
                        let tag_pairs = self.cache.query(move |db| {
                            db.get_corpus_tags(inode)
                                .unwrap_or_default()
                                .into_iter()
                                .map(|t| (t.tag_name, t.tag_value))
                                .collect::<Vec<(String, String)>>()
                        }).recv();

                        if let ActiveView::UnifiedTagEditor(ref mut editor) = self.view {
                            editor.fill_from_db_result(tag_pairs);
                        }
                        self.status_message = Some("Tags loaded from database".to_string());
                    }
                    None => {
                        self.status_message = Some("Track not indexed - no database tags available".to_string());
                    }
                }
            }

            UnifiedTagEditorAction::CloseEmbedded => {
                // Return to parent health modal without staging
                if !self.pop_and_restore() {
                    self.start_health_view();
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
                    self.start_health_view();
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
                if self.witch.decision_keys().is_empty() {
                    // Empty transaction — treat Esc as exit request
                    if self.has_pending_operations() {
                        self.status_message = Some("Cannot quit while operations are pending".to_string());
                    } else {
                        self.view = ActiveView::ExitConfirm(super::ExitConfirmModalState::default());
                    }
                } else {
                    // Pop the view stack to restore the parent view
                    if !self.pop_and_restore() {
                        self.start_health_view();
                    }
                }
            }

            TransactionReviewAction::Discard => {
                // Discard transaction, clear entire view stack, return to health
                self.clear_view_stack();
                let _ = super::operator_decisions::discard_transaction(&mut self.witch);
                self.start_health_view();
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
                        self.start_health_view();
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
                        self.start_health_view();
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
    // Tabbed Transaction Review
    // ========================================================================

    /// Handle actions from the tabbed transaction review lateral view.
    fn handle_tabbed_transaction_review_action(
        &mut self,
        action: super::tabbed_transaction_review::TabbedTransactionReviewAction,
        gesture: Option<&witness::ConfirmationGesture>,
    ) {
        use super::tabbed_transaction_review::TabbedTransactionReviewAction;
        use transaction_review::TransactionReviewAction;

        match action {
            TabbedTransactionReviewAction::None => {}
            TabbedTransactionReviewAction::CycleNext => {
                self.start_lateral_view(widgets::LateralView::Transaction.next(self.transactions_open()));
            }
            TabbedTransactionReviewAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::Transaction.prev(self.transactions_open()));
            }
            TabbedTransactionReviewAction::Review(review_action) => match review_action {
                TransactionReviewAction::None => {}
                TransactionReviewAction::Cancel => {} // No cancel in tabbed mode
                TransactionReviewAction::Confirm => {
                    let Some(g) = gesture else { return };
                    let _ = super::operator_decisions::commit_transaction(&mut self.witch, g);
                    // Re-open transaction immediately
                    let _ = self.witch.start_transaction("Open");
                    self.transition_to_progress_after_mutations(
                        super::progress_screen::ProgressPhase::SignalRefresh,
                    );
                }
                TransactionReviewAction::Discard => {
                    let _ = super::operator_decisions::discard_transaction(&mut self.witch);
                    // Re-open transaction immediately
                    let _ = self.witch.start_transaction("Open");
                    self.sync_browser_pending_edits();
                    self.status_message = Some("Transaction discarded".into());
                }
                TransactionReviewAction::RequestRemoval => {
                    if let ActiveView::TabbedTransactionReview(ref mut state) = self.view {
                        let decisions = transaction_review::fetch_decision_summaries(&self.witch);
                        if let Some(d) = decisions.get(state.review.cursor) {
                            state.review.pending_removal = Some(d.key.clone());
                        }
                    }
                }
                TransactionReviewAction::ConfirmRemoval(key) => {
                    let Some(g) = gesture else { return };
                    let _ = super::operator_decisions::remove_decision(&mut self.witch, &key, g);
                    self.status_message = Some("Decision removed".into());
                    self.sync_browser_pending_edits();
                    // Clamp cursor
                    if let ActiveView::TabbedTransactionReview(ref mut state) = self.view {
                        let remaining = transaction_review::fetch_decision_summaries(&self.witch).len();
                        if state.review.cursor >= remaining && remaining > 0 {
                            state.review.cursor = remaining - 1;
                        }
                    }
                }
            },
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
    /// Delegates to per-view `handle_click` methods. The ConfirmationGesture is
    /// minted once here and threaded to views that need it for button witnessing.
    /// Views return an optional action; if present, it's dispatched as a confirmation.
    pub(super) fn handle_click(&mut self, x: u16, y: u16) {
        let gesture = witness::ConfirmationGesture::new();

        let action = match &mut self.view {
            ActiveView::OobSyncResolution(state) => {
                state.handle_click(x, y, &gesture).map(ViewAction::OobSyncResolution)
            }
            ActiveView::OobConflictInspection(state) => {
                state.handle_click(x, y, &gesture).map(ViewAction::OobConflictInspection)
            }
            ActiveView::Insights(state) => {
                state.handle_click(x, y);
                None
            }
            _ => None,
        };

        if let Some(action) = action {
            self.dispatch_action(action, true);
        }
    }
}
