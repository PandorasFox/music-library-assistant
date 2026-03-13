//! Action Handlers for View-Specific Events
//!
//! Each view emits Actions that are handled via the `HandleAction` trait.
//! The trait structurally enforces that handler implementations live in
//! submodule files rather than accumulating here.
//!
//! This file contains:
//! - The `HandleAction` trait definition
//! - `dispatch_action` (routes ViewAction to the trait)
//! - Shared helper methods used across handlers
//! - `handle_click` (mouse click routing)
//! - Small inline trait impls (ConfigEditor, Insights, TagSearch)

mod compound_split;
mod deploy;
mod disc_extraction;
mod external_match;
mod history;
mod inbox;
mod manual_review;
mod missing_album;
mod oob_resolution;
mod simple_resolutions;
mod startup;
mod tag_canonicity;
mod tag_editor_handler;
mod transaction;
mod tree_browser_handler;
pub(crate) mod witness;

use super::App;
use mm_meta::decisions::DecisionKey;
use crate::active_view::{ActiveView, ViewAction};
use crate::eye::Eye;
use crate::{insights_view, progress_screen, tag_editor, tag_search, widgets};

/// Trait for action types that can be dispatched from ViewAction.
///
/// Each action type implements this in its own submodule file.
/// This prevents handler implementations from accumulating in mod.rs.
pub(super) trait HandleAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>);
}

impl App {
    // =========================================================================
    // Action Dispatch
    // =========================================================================

    /// Dispatch a view action to the appropriate handler.
    ///
    /// `is_confirmation` is true when the triggering event was a confirmation
    /// gesture (Enter keypress). The witness is minted internally from this
    /// flag - callers never touch the ConfirmationGesture type.
    pub(crate) fn dispatch_action(&mut self, action: ViewAction, is_confirmation: bool) {
        let witness = if is_confirmation {
            Some(witness::ConfirmationGesture::new())
        } else {
            None
        };

        match action {
            ViewAction::None => {}
            ViewAction::ConfigEditor(a) => a.handle(self, witness.as_ref()),
            ViewAction::Insights(a) => a.handle(self, witness.as_ref()),
            ViewAction::CorpusBrowser(a) => a.handle(self, witness.as_ref()),
            ViewAction::TagSearch(a) => a.handle(self, witness.as_ref()),
            ViewAction::Inbox(a) => a.handle(self, witness.as_ref()),
            ViewAction::TabbedTransactionReview(a) => a.handle(self, witness.as_ref()),
            ViewAction::ExitConfirm(a) => a.handle(self, witness.as_ref()),
            ViewAction::IntakeConfirmation(a) => a.handle(self, witness.as_ref()),
            ViewAction::UnifiedTagEditor(a) => a.handle(self, witness.as_ref()),
            ViewAction::Deploy(a) => a.handle(self, witness.as_ref()),
            ViewAction::ExternalMatches(a) => a.handle(self, witness.as_ref()),
            ViewAction::MissingFileResolution(a) => a.handle(self, witness.as_ref()),
            ViewAction::MissingDirectoryResolution(a) => a.handle(self, witness.as_ref()),
            ViewAction::CorruptFileResolution(a) => a.handle(self, witness.as_ref()),
            ViewAction::ShitFormatResolution(a) => a.handle(self, witness.as_ref()),
            ViewAction::SubparDuplicateResolution(a) => a.handle(self, witness.as_ref()),
            ViewAction::InboxCorpusMatchResolution(a) => a.handle(self, witness.as_ref()),
            ViewAction::InboxOrganize(a) => a.handle(self, witness.as_ref()),
            ViewAction::DirectoryClusterResolution(a) => a.handle(self, witness.as_ref()),
            ViewAction::MovedFileAcknowledge(a) => a.handle(self, witness.as_ref()),
            ViewAction::OobSyncResolution(a) => a.handle(self, witness.as_ref()),
            ViewAction::OobConflictInspection(a) => a.handle(self, witness.as_ref()),
            ViewAction::ExternalMatchReview(a) => a.handle(self, witness.as_ref()),
            ViewAction::ReleasePackingBrowser(a) => a.handle(self, witness.as_ref()),
            ViewAction::KnotBrowser(a) => a.handle(self, witness.as_ref()),
            ViewAction::History(a) => a.handle(self, witness.as_ref()),
            ViewAction::TagCanonicityResolution(a) => a.handle(self, witness.as_ref()),
            ViewAction::CompoundTagSplit(a) => a.handle(self, witness.as_ref()),
            ViewAction::MissingAlbumSingleResolution(a) => a.handle(self, witness.as_ref()),
            ViewAction::DiscExtractionResolution(a) => a.handle(self, witness.as_ref()),
            ViewAction::ManualReview(a) => a.handle(self, witness.as_ref()),
            ViewAction::TransactionReview(a) => a.handle(self, witness.as_ref()),
        }
    }

    // =========================================================================
    // Shared Helpers
    // =========================================================================

    /// Stage a tag editor decision to the Witch's transaction and update editor state.
    fn stage_tag_editor_decision(
        &mut self,
        key: DecisionKey,
        mutations: Vec<mm_meta::mutations::Mutation>,
        gesture: &witness::ConfirmationGesture,
    ) {
        let label = if let ActiveView::UnifiedTagEditor(ref editor) = self.view {
            editor.current_item_label()
        } else {
            "Tag edit".to_string()
        };
        let decision = gesture.decide(&label, mutations.clone());
        let _ = super::operator_decisions::stage_decision(
            &mut self.witch,
            key,
            decision,
        );
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
    fn stage_mutations_with_transaction(
        &mut self,
        mutations: Vec<mm_meta::mutations::Mutation>,
        label: &str,
        key: DecisionKey,
        gesture: &witness::ConfirmationGesture,
    ) {
        let open_txn = self.open_txn_mode();
        if !open_txn {
            let _ = self.witch.start_transaction(label);
        }
        let decision = gesture.decide(label, mutations);
        let _ = super::operator_decisions::stage_decision(
            &mut self.witch,
            key,
            decision,
        );
    }

    /// Cancel the current modal and return to the source view.
    ///
    /// In default (closed-txn) mode: discards any open transaction before returning.
    /// In open-txn mode: leaves the persistent transaction intact.
    pub(crate) fn cancel_and_return_to_source(&mut self, log_message: &str) {
        mm_meta::logging::log_general(log_message);
        if !self.open_txn_mode() && self.witch_status().transaction.is_some() {
            let _ = super::operator_decisions::discard_transaction(&mut self.witch);
        }
        self.return_to_last_lateral_view();
    }

    /// After staging decisions: route to review (closed-txn) or return to source (open-txn).
    ///
    /// In default mode: shows the TransactionReview modal.
    /// In open-txn mode: returns to the last lateral view with a status message.
    pub(crate) fn after_staging_decisions(&mut self) {
        if self.open_txn_mode() {
            self.start_tabbed_transaction_review();
            self.status_message = Some("Decision staged".into());
        } else {
            self.start_transaction_review();
        }
    }

    /// Stage mutations for a simple resolution modal and transition to review.
    ///
    /// Returns early if `mutations` is empty (setting a status message instead).
    fn stage_resolution(
        &mut self,
        mutations: Vec<mm_meta::mutations::Mutation>,
        label: &str,
        key: DecisionKey,
        empty_msg: &str,
        gesture: &witness::ConfirmationGesture,
    ) {
        if mutations.is_empty() {
            self.status_message = Some(empty_msg.to_string());
        } else {
            self.stage_mutations_with_transaction(mutations, label, key, gesture);
            self.after_staging_decisions();
        }
    }

    /// Handle RequestQuit action (shared across all lateral views).
    pub(crate) fn handle_request_quit(&mut self) {
        if self.has_pending_operations() {
            self.status_message = Some("Cannot quit while operations are pending".to_string());
        } else {
            self.view = ActiveView::ExitConfirm(super::ExitConfirmModalState::default());
        }
    }

    /// Open an embedded tag editor for a set of inodes.
    ///
    /// Common helper for EditTracks/EditTracksAggregated actions across modals.
    fn open_tag_editor_for_inodes(
        &mut self,
        inodes: Vec<i64>,
        zone: mm_meta::db_types::Zone,
        key: DecisionKey,
        label: String,
        mode: tag_editor::TagEditorMode,
    ) {
        if inodes.is_empty() {
            return;
        }
        let audio_files = self
            .witch
            .query(mm_meta::domain_queries::GetAudioFilesByInodes { inodes, zone });
        if !audio_files.is_empty() {
            self.open_embedded_tag_editor(mode, audio_files, key, label);
        }
    }

    /// Position the tag editor cursor on a specific inode after opening.
    fn position_editor_cursor(&mut self, target_inode: Option<i64>) {
        if let Some(target_inode) = target_inode {
            if let ActiveView::UnifiedTagEditor(ref mut editor) = self.view {
                if let tag_editor::types::TagEditContext::BulkEdit {
                    ref audio_files, ..
                } = editor.context
                {
                    if let Some(idx) = audio_files.iter().position(|af| af.inode() == target_inode)
                    {
                        editor.current_item_idx = idx;
                    }
                }
            }
        }
    }

    /// Handle CycleNext/CyclePrev for a lateral view.
    pub(crate) fn handle_lateral_cycle(
        &mut self,
        lateral: super::widgets::LateralView,
        forward: bool,
    ) {
        let txn = self.transactions_open();
        let target = if forward {
            lateral.next(txn)
        } else {
            lateral.prev(txn)
        };
        self.start_lateral_view(target);
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
    pub(super) fn transition_to_progress_after_mutations(
        &mut self,
        phase: progress_screen::ProgressPhase,
    ) {
        use progress_screen::ProgressPhase;

        let screen = match phase {
            ProgressPhase::Eyeballing => progress_screen::ProgressScreen::new_eyeballing(),
            ProgressPhase::ContentAnalysis => {
                progress_screen::ProgressScreen::new_content_analysis()
            }
            ProgressPhase::SignalRefresh => progress_screen::ProgressScreen::new_signal_refresh(),
        };
        self.view = ActiveView::Progress {
            screen,
            eye: Eye::default(),
        };
    }

    /// Start missing tag resolution from Insights view.
    ///
    /// Loads all MissingTag signals, collects unique inodes, and opens
    /// the bulk tag editor so the operator can fill in missing tags.
    fn start_missing_tag_resolution(&mut self) {
        let audio_files = self
            .witch
            .query(mm_meta::domain_queries::GetMissingTagAudioFiles);

        if audio_files.is_empty() {
            return;
        }

        self.open_unified_tag_editor_bulk(
            audio_files,
            tag_editor::TagEditorSource::HealthModal,
            None,
        );
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
        // Check titlebar tab clicks first (applies to all lateral views)
        if self.view.lateral_view().is_some() {
            for (view, rect) in &self.tab_click_rects {
                if crate::widgets::rect_contains(*rect, x, y) {
                    let target = *view;
                    self.start_lateral_view(target);
                    return;
                }
            }
        }

        let gesture = witness::ConfirmationGesture::new();

        // Macro for the two common click dispatch patterns:
        // - gesture: handle_click(x, y, &gesture) -> Option<Action>, wrapped in ViewAction
        // - void: handle_click(x, y) -> (), always None
        macro_rules! click_dispatch {
            (gesture $variant:ident, $state:expr) => {
                $state.handle_click(x, y, &gesture).map(ViewAction::$variant)
            };
            (void $state:expr) => {{
                $state.handle_click(x, y);
                None
            }};
        }

        let action = match &mut self.view {
            ActiveView::ExitConfirm(state) => {
                if let Some(button) = state.button_rects.hit_test(x, y) {
                    match button {
                        "yes" => {
                            state.selected = 0;
                            Some(ViewAction::ExitConfirm(super::ExitConfirmAction::Quit))
                        }
                        "no" => {
                            state.selected = 1;
                            Some(ViewAction::ExitConfirm(super::ExitConfirmAction::Cancel))
                        }
                        "shutdown" => {
                            state.selected = 2;
                            Some(ViewAction::ExitConfirm(super::ExitConfirmAction::QuitAndShutdown))
                        }
                        _ => None,
                    }
                } else {
                    None
                }
            }
            ActiveView::OobSyncResolution(s) => click_dispatch!(gesture OobSyncResolution, s),
            ActiveView::OobConflictInspection(s) => click_dispatch!(gesture OobConflictInspection, s),
            ActiveView::ExternalMatchReview(state) => state.handle_click(x, y).map(ViewAction::ExternalMatchReview),
            ActiveView::MovedFileAcknowledge(s) => click_dispatch!(gesture MovedFileAcknowledge, s),
            ActiveView::SubparDuplicateResolution(s) => click_dispatch!(gesture SubparDuplicateResolution, s),
            ActiveView::InboxCorpusMatchResolution(s) => click_dispatch!(gesture InboxCorpusMatchResolution, s),
            ActiveView::MissingAlbumSingleResolution(s) => click_dispatch!(gesture MissingAlbumSingleResolution, s),
            ActiveView::DiscExtractionResolution(s) => click_dispatch!(gesture DiscExtractionResolution, s),
            ActiveView::CorruptFileResolution(s) => click_dispatch!(gesture CorruptFileResolution, s),
            ActiveView::MissingDirectoryResolution(s) => click_dispatch!(gesture MissingDirectoryResolution, s),
            ActiveView::MissingFileResolution(s) => click_dispatch!(gesture MissingFileResolution, s),
            ActiveView::ShitFormatResolution(s) => click_dispatch!(gesture ShitFormatResolution, s),
            ActiveView::DirectoryClusterResolution(s) => click_dispatch!(gesture DirectoryClusterResolution, s),
            ActiveView::Insights(s) => click_dispatch!(void s),
            ActiveView::TagCanonicityResolution { ref mut state, .. } => click_dispatch!(void state),
            ActiveView::CompoundTagSplit { ref mut state, .. } => click_dispatch!(void state),
            ActiveView::UnifiedTagEditor(ref mut s) => click_dispatch!(void s),
            ActiveView::CorpusBrowser(ref mut s) => click_dispatch!(void s),
            ActiveView::History(ref mut s) => click_dispatch!(void s),
            ActiveView::ExternalMatches(ref mut s) => click_dispatch!(void s),
            ActiveView::Inbox(ref mut s) => click_dispatch!(void s),
            ActiveView::TagSearch(ref mut s) => click_dispatch!(void s),
            ActiveView::KnotBrowser(ref mut s) => click_dispatch!(void s),
            _ => None,
        };

        if let Some(action) = action {
            self.dispatch_action(action, true);
        }
    }
}

// =============================================================================
// Inline HandleAction impls for small handlers
// =============================================================================

impl HandleAction for super::config_editor::ConfigEditorAction {
    fn handle(self, app: &mut App, gesture: Option<&witness::ConfirmationGesture>) {
        use mm_meta::mutations::config_edit::ApplyConfigEditsMutation;
        use mm_meta::mutations::Mutation;

        match self {
            super::config_editor::ConfigEditorAction::None => {}
            super::config_editor::ConfigEditorAction::Save => {
                // Build mutation from editor state and stage for transaction review.
                let mutation_data = if let ActiveView::ConfigEditor(ref state) = app.view {
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

                    // Validate config through the Witch before staging
                    if let Err(e) = app.witch.validate_config(&new_config) {
                        app.status_message = Some(format!("Config rejected: {}", e));
                        return;
                    }

                    let mutation = Mutation::ApplyConfigEdits(Box::new(ApplyConfigEditsMutation {
                        original_kdl,
                        old_config,
                        new_config,
                    }));

                    let open_txn = app.open_txn_mode();
                    if !open_txn {
                        let _ = app.witch.start_transaction("Config update");
                    }
                    let decision = g.decide("Apply config changes", vec![mutation]);
                    let _ = super::operator_decisions::stage_decision(
                        &mut app.witch,
                        DecisionKey::ConfigEdit,
                        decision,
                    );

                    app.after_staging_decisions();
                } else {
                    // No edits — just return to health
                    app.start_health_view();
                }
            }
            super::config_editor::ConfigEditorAction::Discard => {
                app.start_health_view();
            }
            super::config_editor::ConfigEditorAction::CycleNext => {
                app.handle_lateral_cycle(widgets::LateralView::Config, true);
            }
            super::config_editor::ConfigEditorAction::CyclePrev => {
                app.handle_lateral_cycle(widgets::LateralView::Config, false);
            }
        }
    }
}

impl HandleAction for insights_view::InsightsAction {
    fn handle(self, app: &mut App, _witness: Option<&witness::ConfirmationGesture>) {
        match self {
            insights_view::InsightsAction::None => {}
            insights_view::InsightsAction::RequestQuit => app.handle_request_quit(),
            insights_view::InsightsAction::CycleNext => {
                app.handle_lateral_cycle(widgets::LateralView::Health, true);
            }
            insights_view::InsightsAction::CyclePrev => {
                app.handle_lateral_cycle(widgets::LateralView::Health, false);
            }
            insights_view::InsightsAction::Launch => {
                // Use selected_action() to dispatch to appropriate modal
                let selected = if let ActiveView::Insights(ref v) = app.view {
                    v.selected_action()
                } else {
                    None
                };
                match selected {
                    Some(insights_view::InsightAction::LaunchMissingFileResolution) => {
                        app.start_missing_file_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchTagCanonicityResolution) => {
                        app.start_tag_canonicity_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchCompoundTagSplitSafe) => {
                        let tag_name = if let ActiveView::Insights(ref v) = app.view {
                            v.selected_insight_type().and_then(|t| match t {
                                insights_view::InsightType::CompoundTagValueSafe { tag_name } => {
                                    Some(tag_name)
                                }
                                _ => None,
                            })
                        } else {
                            None
                        };
                        app.start_compound_split_resolution(true, tag_name.as_deref());
                    }
                    Some(insights_view::InsightAction::LaunchCompoundTagSplitReview) => {
                        let tag_name = if let ActiveView::Insights(ref v) = app.view {
                            v.selected_insight_type().and_then(|t| match t {
                                insights_view::InsightType::CompoundTagValueReview { tag_name } => {
                                    Some(tag_name)
                                }
                                _ => None,
                            })
                        } else {
                            None
                        };
                        app.start_compound_split_resolution(false, tag_name.as_deref());
                    }
                    Some(insights_view::InsightAction::LaunchOobTagSync) => {
                        app.start_oob_sync_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchOobTagConflict) => {
                        app.start_oob_conflict_inspection();
                    }
                    Some(insights_view::InsightAction::LaunchMovedFileAcknowledge) => {
                        app.start_moved_file_acknowledge();
                    }
                    Some(insights_view::InsightAction::LaunchCorruptFileResolution) => {
                        app.start_corrupt_file_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchShitFormatTranscode) => {
                        app.start_shit_format_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchIntakeConfirmation) => {
                        app.start_intake_confirmation_from_health();
                    }
                    Some(insights_view::InsightAction::LaunchCrossSourceOverlapResolution) => {
                        app.start_directory_overlap_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchReleaseOverlapResolution) => {
                        app.start_release_overlap_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchSubparDuplicateResolution) => {
                        app.start_subpar_duplicate_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchMissingDirectoryResolution) => {
                        app.start_missing_directory_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchManualReview(kind)) => {
                        app.start_manual_review(kind);
                    }
                    Some(insights_view::InsightAction::LaunchMissingTagResolution) => {
                        app.start_missing_tag_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchMissingAlbumSingleResolution) => {
                        app.start_missing_album_single_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchDiscExtractionResolution) => {
                        app.start_disc_extraction_resolution();
                    }
                    Some(insights_view::InsightAction::LaunchPathTagMismatchResolution) => {
                        app.status_message = Some("Not yet implemented".to_string());
                    }
                    Some(insights_view::InsightAction::NotImplemented) => {
                        app.status_message = Some("Not yet implemented".to_string());
                    }
                    Some(insights_view::InsightAction::Informational) | None => {
                        // Informational entries have no action
                    }
                }
            }
        }
    }
}

impl HandleAction for tag_search::TagSearchAction {
    fn handle(self, app: &mut App, _witness: Option<&witness::ConfirmationGesture>) {
        match self {
            tag_search::TagSearchAction::None => {}
            tag_search::TagSearchAction::Cancel => {
                // Return to Insights view
                app.start_health_view();
            }
            tag_search::TagSearchAction::CycleNext => {
                app.handle_lateral_cycle(widgets::LateralView::Search, true);
            }
            tag_search::TagSearchAction::CyclePrev => {
                app.handle_lateral_cycle(widgets::LateralView::Search, false);
            }
            tag_search::TagSearchAction::ExecuteSearch => {
                // Execute search via cache thread (blocking — fast single query)
                if let ActiveView::TagSearch(ref mut search) = app.view {
                    let all_files = app
                        .witch
                        .query(mm_meta::domain_queries::GetAllAudioFilesWithTags {
                            zone: mm_meta::db_types::Zone::Corpus,
                            include_library: false,
                        });
                    search.execute_search(all_files);
                }
            }
            tag_search::TagSearchAction::EditAudioFile(audio_file) => {
                app.push_current_view();
                app.start_unified_tag_editor_for_audio_file(audio_file);
            }
        }
    }
}
