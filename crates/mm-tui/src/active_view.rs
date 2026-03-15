//! ActiveView - the single source of truth for which view is active and its state.
//!
//! Replaces the old `UiMode` enum + 25 `Option<State>` fields pattern.
//! Each variant carries its own state, so the type system guarantees that
//! mode and state are always consistent.

use crate::{
    config_editor, corrupt_file_modal, deploy_modal,
    external_match_modal, external_match_view, eye::Eye,
    history_view, inbox_corpus_match_modal, inbox_organize, inbox_view, insights_view,
    missing_directory_modal, missing_file_modal,
    moved_file_modal, oob_conflict_modal, oob_sync_modal, progress_screen, progressive_worker,
    shit_format_modal, startup, subpar_duplicate_modal, tabbed_transaction_review,
    tag_editor, tag_search, transaction_review, tree_browser,
};

// ============================================================================
// ActiveView
// ============================================================================

/// The active view and its state. One variant is active at a time.
#[allow(clippy::large_enum_variant)]
pub(crate) enum ActiveView {
    // Startup maintenance (non-interactive, Witch auto-runs schema reconciliation / vacuum)
    StartupMaintenance,

    // Lateral view ring
    ConfigEditor(config_editor::ConfigEditorState),
    Insights {
        data: insights_view::InsightsViewData,
        interaction: insights_view::HealthInteraction,
    },
    History {
        data: history_view::HistoryViewData,
        interaction: history_view::HistoryInteraction,
    },
    CorpusBrowser(tree_browser::TreeBrowserState),
    TagSearch(tag_search::TagSearchState),
    Inbox {
        data: inbox_view::InboxViewData,
        interaction: inbox_view::InboxInteraction,
    },
    TabbedTransactionReview(tabbed_transaction_review::TabbedTransactionReviewState),
    Deploy {
        data: deploy_modal::DeployViewData,
        interaction: deploy_modal::DeployInteraction,
    },
    ExternalMatches {
        data: external_match_view::ExternalMatchesViewData,
        interaction: external_match_view::ExternalMatchesInteraction,
    },

    // Progress (non-interactive, owns eye animation)
    Progress {
        screen: progress_screen::ProgressScreen,
        eye: Eye,
    },
    ProgressiveWork(progressive_worker::ProgressiveWorkerState),

    // Simple modals
    ExitConfirm(ExitConfirmModalState),
    IntakeConfirmation(startup::IntakeConfirmationState),

    // Editors / previews
    UnifiedTagEditor(tag_editor::UnifiedTagEditorState),

    // Resolution flows (single state)
    MissingFileResolution(missing_file_modal::MissingFilePreviewState),
    MissingDirectoryResolution(missing_directory_modal::MissingDirectoryState),
    CorruptFileResolution(corrupt_file_modal::CorruptFileState),
    ShitFormatResolution(shit_format_modal::ShitFormatPreviewState),
    SubparDuplicateResolution(subpar_duplicate_modal::SubparDuplicateState),
    InboxCorpusMatchResolution(inbox_corpus_match_modal::InboxCorpusMatchState),
    InboxOrganize(inbox_organize::InboxOrganizeState),
    DirectoryClusterResolution {
        data: mm_meta::views::cluster_deploy::DirectoryClusterModalData,
        current_cluster: usize,
        list: mm_ui::standard_list::StandardListState,
        buttons: mm_ui::modal_buttons::ButtonRowState<mm_ui::resolutions::directory_cluster::DirectoryClusterButton>,
        focus: mm_ui::geometry::FocusPane,
    },
    MovedFileAcknowledge(moved_file_modal::MovedFileState),
    OobSyncResolution(oob_sync_modal::OobSyncState),
    OobConflictInspection(oob_conflict_modal::OobConflictState),
    ExternalMatchReview(external_match_modal::ExternalMatchReviewState),

    // Release packing browser (read-only)
    ReleasePackingBrowser(super::release_packing_browser::ReleasePackingBrowserState),

    // Knot browser (read-only)
    KnotBrowser(super::knot_browser::KnotBrowserState),

    // Tag canonicity resolution with packed data + StandardList + DecisionField
    TagCanonicityResolution {
        data: mm_meta::views::canonicity_compound::TagCanonicityResolutionData,
        current_cluster: usize,
        list: mm_ui::standard_list::StandardListState,
        buttons: mm_ui::modal_buttons::ButtonRowState<mm_ui::resolutions::tag_canonicity::CanonicityButton>,
        field: mm_ui::decision_field::DecisionField,
        zone: mm_meta::db_types::Zone,
        focus: mm_ui::geometry::FocusPane,
        /// Which canonicity mode — controls button labels/semantics.
        mode: mm_ui::resolutions::tag_canonicity::CanonicityMode,
    },
    // Compound tag split with packed data + StandardList + DecisionField
    CompoundTagSplitResolution {
        data: mm_meta::views::canonicity_compound::CompoundSplitResolutionData,
        current_group: usize,
        list: mm_ui::standard_list::StandardListState,
        buttons: mm_ui::modal_buttons::ButtonRowState<mm_ui::resolutions::compound_split::CompoundSplitButton>,
        field: mm_ui::decision_field::DecisionField,
        zone: mm_meta::db_types::Zone,
        focus: mm_ui::geometry::FocusPane,
        safe_mode: bool,
    },

    // Missing album singles resolution with StandardList + buttons
    MissingAlbumSingleResolution {
        data: Vec<mm_meta::domain_queries::MissingAlbumSingleSignalWire>,
        current_group: usize,
        list: mm_ui::standard_list::StandardListState,
        buttons: mm_ui::modal_buttons::ButtonRowState<mm_ui::resolutions::missing_album::MissingAlbumButton>,
        focus: mm_ui::geometry::FocusPane,
        suffix: String,
    },

    // Disc extraction resolution with StandardList + buttons
    DiscExtractionResolution {
        data: mm_meta::domain_queries::DiscExtractionModalData,
        current_group: usize,
        list: mm_ui::standard_list::StandardListState,
        buttons: mm_ui::modal_buttons::ButtonRowState<mm_ui::resolutions::disc_extraction::DiscExtractionButton>,
        focus: mm_ui::geometry::FocusPane,
        disc_tag_name: String,
    },

    // Manual review with StandardList + buttons
    ManualReviewResolution {
        data: mm_meta::views::review_match::ManualReviewData,
        review_kind: mm_meta::views::review_match::ReviewKind,
        current_group: usize,
        list: mm_ui::standard_list::StandardListState,
        buttons: mm_ui::modal_buttons::ButtonRowState<mm_ui::resolutions::manual_review::ReviewButton>,
        focus: mm_ui::geometry::FocusPane,
    },

    // Transaction review (view stack holds suspended views)
    TransactionReview(transaction_review::TransactionReviewState),
}

impl ActiveView {
    /// Get a header suffix for the title bar, if applicable.
    pub(crate) fn header_suffix(&self) -> Option<&'static str> {
        match self {
            Self::StartupMaintenance => Some("Startup"),
            Self::ConfigEditor(_) => Some("Config Editor"),
            Self::Insights { .. } => Some("Corpus Insights"),
            Self::History { .. } => Some("Edit History"),
            Self::CorpusBrowser(_) => Some("Corpus Browser"),
            Self::TagSearch(_) => Some("Tag Search"),
            Self::Inbox { .. } => Some("Inbox"),
            Self::TabbedTransactionReview(_) => Some("Transaction"),
            Self::Deploy { .. } => Some("Deploy"),
            Self::ExternalMatches { .. } => Some("External Matches"),
            Self::Progress { .. } => None,
            Self::ProgressiveWork(_) => Some("Processing"),
            Self::ExitConfirm(_) => Some("Exit Confirmation"),
            Self::IntakeConfirmation(_) => Some("Intake Confirmation"),
            Self::UnifiedTagEditor(_) => Some("Tag Editor"),
            Self::MissingFileResolution(_) => Some("Missing File Resolution"),
            Self::MissingDirectoryResolution(_) => Some("Missing Directory Acknowledgment"),
            Self::CorruptFileResolution(_) => Some("Corrupt File Resolution"),
            Self::ShitFormatResolution(_) => Some("Shit Format Resolution"),
            Self::SubparDuplicateResolution(_) => Some("Subpar Duplicate Resolution"),
            Self::InboxCorpusMatchResolution(_) => Some("Inbox Corpus Match Resolution"),
            Self::InboxOrganize(_) => Some("Inbox Organize"),
            Self::DirectoryClusterResolution { .. } => Some("Directory Overlap Resolution"),
            Self::MovedFileAcknowledge(_) => Some("Moved Files"),
            Self::OobSyncResolution(_) => Some("OOB Tag Sync"),
            Self::OobConflictInspection(_) => Some("OOB Tag Conflicts"),
            Self::ExternalMatchReview(_) => Some("External Match Review"),
            Self::ReleasePackingBrowser(_) => Some("Release Packing Browser"),
            Self::KnotBrowser(_) => Some("Knot Browser"),
            Self::TagCanonicityResolution { mode: mm_ui::resolutions::tag_canonicity::CanonicityMode::InconsistentAlbumArtist, .. } => Some("Album Artist"),
            Self::TagCanonicityResolution { .. } => Some("Tag Canonicity"),
            Self::CompoundTagSplitResolution { .. } => Some("Compound Tag Split"),
            Self::MissingAlbumSingleResolution { .. } => Some("Missing Album Singles"),
            Self::DiscExtractionResolution { .. } => Some("Disc Extraction"),
            Self::ManualReviewResolution { review_kind, .. } => Some(review_kind.title()),
            Self::TransactionReview(_) => Some("Transaction Review"),
        }
    }

    /// Path of the currently selected file/directory (for status bar line 1).
    ///
    /// Returns the corpus-relative path of whatever item the cursor is on.
    /// Views without file listings return None.
    pub(crate) fn selected_path(&self) -> Option<&str> {
        match self {
            Self::StartupMaintenance => None,
            Self::CorpusBrowser(browser) => browser.selected_path(),
            Self::TagCanonicityResolution { ref data, current_cluster, ref list, .. } => {
                data.clusters.get(*current_cluster)
                    .and_then(|c| c.variants.get(list.cursor))
                    .and_then(|v| v.files.first())
                    .map(|f| f.display_name.as_str())
            }
            Self::CompoundTagSplitResolution { ref data, current_group, ref list, .. } => {
                data.groups.get(*current_group)
                    .and_then(|g| g.files.get(list.cursor))
                    .map(|f| f.display_name.as_str())
            }
            Self::MissingFileResolution(s) => s.selected_path(),
            Self::MissingDirectoryResolution(s) => s.selected_path(),
            Self::CorruptFileResolution(s) => s.selected_path(),
            Self::ShitFormatResolution(s) => s.selected_path(),
            Self::SubparDuplicateResolution(s) => s.selected_path(),
            Self::InboxCorpusMatchResolution(s) => s.selected_path(),
            Self::InboxOrganize(_) => None,
            Self::DirectoryClusterResolution { ref data, current_cluster, ref list, .. } => {
                data.clusters.get(*current_cluster)
                    .and_then(|c| c.directories.get(list.cursor))
                    .map(|d| d.path_suffix.as_str())
            }
            Self::MovedFileAcknowledge(s) => s.selected_path(),
            Self::OobSyncResolution(s) => s.selected_path(),
            Self::OobConflictInspection(s) => s.selected_path(),
            Self::ExternalMatchReview(s) => s.selected_path(),
            Self::Deploy { ref data, ref interaction } => data.selected_path(interaction),
            Self::UnifiedTagEditor(s) => s.selected_path(),
            Self::MissingAlbumSingleResolution { ref data, current_group, ref list, .. } => {
                data.get(*current_group)
                    .and_then(|s| s.data.tracks.get(list.cursor))
                    .map(|t| t.path.as_str())
            }
            Self::DiscExtractionResolution { ref data, current_group, ref list, .. } => {
                data.groups.get(*current_group)
                    .and_then(|g| g.files.get(list.cursor))
                    .map(|f| f.path.as_str())
            }
            Self::ManualReviewResolution { ref data, current_group, ref list, .. } => {
                data.groups.get(*current_group)
                    .and_then(|g| g.files.get(list.cursor))
                    .map(|f| f.corpus_path.as_str())
            }
            Self::Inbox { ref data, ref interaction } => data.selected_entry(interaction.list.cursor).map(|e| e.label.as_str()),
            _ => None,
        }
    }

    /// Return the LateralView variant for this view, if it's a lateral view.
    pub(crate) fn lateral_view(&self) -> Option<crate::widgets::LateralView> {
        use crate::widgets::LateralView;
        match self {
            Self::ConfigEditor(_) => Some(LateralView::Config),
            Self::TagSearch(_) => Some(LateralView::Search),
            Self::CorpusBrowser(_) => Some(LateralView::Files),
            Self::Insights { .. } => Some(LateralView::Health),
            Self::History { .. } => Some(LateralView::History),
            Self::Inbox { .. } => Some(LateralView::Inbox),
            Self::TabbedTransactionReview(_) => Some(LateralView::Transaction),
            Self::Deploy { .. } => Some(LateralView::Deploy),
            Self::ExternalMatches { .. } => Some(LateralView::ExternalMatches),
            _ => None,
        }
    }

    /// Whether this view needs to handle CycleNext/CyclePrev as domain actions
    /// rather than having them intercepted for automatic lateral cycling.
    ///
    /// ConfigEditor: defers cycle when unsaved edits, Tab means nav_right in button focus.
    /// CorpusBrowser: captures Tab when config panel/search/filter is active.
    pub(crate) fn wants_raw_cycle(&self) -> bool {
        matches!(self, Self::ConfigEditor(_) | Self::CorpusBrowser(_))
    }
}

// ============================================================================
// SuspendedView - for view stack push/pop navigation
// ============================================================================

/// Captures enough context to restore a suspended view from the view stack.
#[allow(clippy::large_enum_variant)]
pub(crate) enum SuspendedView {
    /// Restore view directly (most modals).
    Direct(ActiveView),
}

// ============================================================================
// ViewAction - two-phase key dispatch wrapper
// ============================================================================

/// Wrapper for view-specific actions, produced by Phase 1 (borrow view) and
/// consumed by Phase 2 (dispatch on &mut self).
pub(crate) enum ViewAction {
    None,
    ConfigEditor(config_editor::ConfigEditorAction),
    Insights(insights_view::HealthAction),
    CorpusBrowser(tree_browser::TreeBrowserAction),
    TagSearch(tag_search::TagSearchAction),
    Inbox(inbox_view::InboxInsightAction),
    TabbedTransactionReview(tabbed_transaction_review::TabbedTransactionReviewAction),
    Deploy(deploy_modal::DeployAction),
    ExternalMatches(external_match_view::ExternalMatchesAction),
    ExitConfirm(ExitConfirmAction),
    IntakeConfirmation(startup::IntakeConfirmationAction),
    UnifiedTagEditor(tag_editor::UnifiedTagEditorAction),
    MissingFileResolution(missing_file_modal::MissingFileAction),
    MissingDirectoryResolution(missing_directory_modal::MissingDirectoryAction),
    CorruptFileResolution(corrupt_file_modal::CorruptFileAction),
    ShitFormatResolution(shit_format_modal::ShitFormatAction),
    SubparDuplicateResolution(subpar_duplicate_modal::SubparDuplicateAction),
    InboxCorpusMatchResolution(inbox_corpus_match_modal::InboxCorpusMatchAction),
    InboxOrganize(inbox_organize::InboxOrganizeAction),
    DirectoryClusterResolution(mm_ui::resolutions::directory_cluster::DirectoryClusterAction),
    MovedFileAcknowledge(moved_file_modal::MovedFileAction),
    OobSyncResolution(oob_sync_modal::OobSyncAction),
    OobConflictInspection(oob_conflict_modal::OobConflictAction),
    ExternalMatchReview(external_match_modal::ExternalMatchReviewAction),
    ReleasePackingBrowser(super::release_packing_browser::ReleasePackingBrowserAction),
    KnotBrowser(super::knot_browser::KnotBrowserAction),
    History(history_view::HistoryAction),
    TagCanonicityResolution(mm_ui::resolutions::tag_canonicity::CanonicityAction),
    CompoundTagSplitResolution(mm_ui::resolutions::compound_split::CompoundSplitAction),
    MissingAlbumSingleResolution(mm_ui::resolutions::missing_album::MissingAlbumAction),
    DiscExtractionResolution(mm_ui::resolutions::disc_extraction::DiscExtractionAction),
    ManualReviewResolution(mm_ui::resolutions::manual_review::ReviewAction),
    TransactionReview(transaction_review::TransactionReviewAction),
}

// ============================================================================
// ExitConfirmAction - extracted from inline key handling
// ============================================================================

/// Action from the exit confirmation modal.
pub(crate) enum ExitConfirmAction {
    None,
    Quit,
    QuitAndShutdown,
    Cancel,
}

// ============================================================================
// ExitConfirmModalState
// ============================================================================

/// State for the exit confirmation modal.
/// Default selection is Cancel (stay in application).
pub(crate) struct ExitConfirmModalState {
    /// 0 = Yes/Confirm, 1 = Cancel/No, 2 = Quit + shutdown server
    pub selected: u8,
    /// True if operations are in progress (shows warning)
    pub has_operations: bool,
    /// Click target rects for buttons, populated during render.
    pub button_rects: crate::widgets::ButtonRects,
}

impl Default for ExitConfirmModalState {
    fn default() -> Self {
        Self {
            selected: 1, // Cancel by default
            has_operations: false,
            button_rects: Default::default(),
        }
    }
}


