//! ActiveView - the single source of truth for which view is active and its state.
//!
//! Replaces the old `UiMode` enum + 25 `Option<State>` fields pattern.
//! Each variant carries its own state, so the type system guarantees that
//! mode and state are always consistent.

use crate::ui::{
    compound_split_v2, config_editor, corrupt_file_modal, deploy_modal, directory_cluster_modal,
    disc_extraction_modal, external_match_modal, external_match_view, eye::Eye,
    history_view, inbox_corpus_match_modal, inbox_organize, inbox_view, insights_view,
    manual_review_modal, missing_album_modal, missing_directory_modal, missing_file_modal,
    moved_file_modal, oob_conflict_modal, oob_sync_modal, progress_screen, progressive_worker,
    shit_format_modal, startup, subpar_duplicate_modal, tabbed_transaction_review,
    tag_canonicity_v2, tag_editor, tag_search, transaction_review, tree_browser,
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
    Insights(insights_view::InsightsViewState),
    History(history_view::HistoryViewState),
    CorpusBrowser(tree_browser::TreeBrowserState),
    TagSearch(tag_search::TagSearchState),
    Inbox(inbox_view::InboxViewState),
    TabbedTransactionReview(tabbed_transaction_review::TabbedTransactionReviewState),
    Deploy(deploy_modal::DeployViewState),
    ExternalMatches(external_match_view::ExternalMatchesViewState),

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
    MissingDirectoryResolution(missing_directory_modal::MissingDirectoryPreviewState),
    CorruptFileResolution(corrupt_file_modal::CorruptFilePreviewState),
    ShitFormatResolution(shit_format_modal::ShitFormatPreviewState),
    SubparDuplicateResolution(subpar_duplicate_modal::SubparDuplicatePreviewState),
    InboxCorpusMatchResolution(inbox_corpus_match_modal::InboxCorpusMatchPreviewState),
    InboxOrganize(inbox_organize::InboxOrganizeState),
    DirectoryClusterResolution(directory_cluster_modal::DirectoryClusterPreviewState),
    MovedFileAcknowledge(moved_file_modal::MovedFileState),
    OobSyncResolution(oob_sync_modal::OobSyncState),
    OobConflictInspection(oob_conflict_modal::OobConflictState),
    ExternalMatchReview(external_match_modal::ExternalMatchReviewState),

    // Release packing browser (read-only)
    ReleasePackingBrowser(super::release_packing_browser::ReleasePackingBrowserState),

    // Knot browser (read-only)
    KnotBrowser(super::knot_browser::KnotBrowserState),

    // Resolution flows (companion state bundled)
    TagCanonicityResolution {
        state: tag_canonicity_v2::TagCanonicalityStateV2,
        clusters: TagCanonicityClusters,
    },
    CompoundTagSplit {
        state: compound_split_v2::CompoundSplitStateV2,
        clusters: compound_split_v2::CompoundSplitClustersV2,
        safe_mode: bool,
        zone: crate::db::types::Zone,
    },

    // Missing album singles resolution
    MissingAlbumSingleResolution(missing_album_modal::MissingAlbumState),

    // Disc extraction resolution (ALBUM or TRACKNUMBER → DISCNUMBER)
    DiscExtractionResolution(disc_extraction_modal::DiscExtractionState),

    // Manual review (iterate through groups, stash/edit files)
    ManualReview(manual_review_modal::ManualReviewState),

    // Transaction review (view stack holds suspended views)
    TransactionReview(transaction_review::TransactionReviewState),
}

impl ActiveView {
    /// Get a header suffix for the title bar, if applicable.
    pub(crate) fn header_suffix(&self) -> Option<&'static str> {
        match self {
            Self::StartupMaintenance => Some("Startup"),
            Self::ConfigEditor(_) => Some("Config Editor"),
            Self::Insights(_) => Some("Corpus Insights"),
            Self::History(_) => Some("Edit History"),
            Self::CorpusBrowser(_) => Some("Corpus Browser"),
            Self::TagSearch(_) => Some("Tag Search"),
            Self::Inbox(_) => Some("Inbox"),
            Self::TabbedTransactionReview(_) => Some("Transaction"),
            Self::Deploy(_) => Some("Deploy"),
            Self::ExternalMatches(_) => Some("External Matches"),
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
            Self::DirectoryClusterResolution(_) => Some("Directory Overlap Resolution"),
            Self::MovedFileAcknowledge(_) => Some("Moved Files"),
            Self::OobSyncResolution(_) => Some("OOB Tag Sync"),
            Self::OobConflictInspection(_) => Some("OOB Tag Conflicts"),
            Self::ExternalMatchReview(_) => Some("External Match Review"),
            Self::ReleasePackingBrowser(_) => Some("Release Packing Browser"),
            Self::KnotBrowser(_) => Some("Knot Browser"),
            Self::TagCanonicityResolution { .. } => Some("Tag Canonicity"),
            Self::CompoundTagSplit { .. } => Some("Compound Tag Split"),
            Self::MissingAlbumSingleResolution(_) => Some("Missing Album Singles"),
            Self::DiscExtractionResolution(_) => Some("Disc Extraction"),
            Self::ManualReview(s) => Some(s.header_suffix()),
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
            Self::CorpusBrowser(browser) => browser.selected_path().and_then(|p| p.to_str()),
            Self::TagCanonicityResolution { state, .. } => state.selected_path(),
            Self::CompoundTagSplit { state, .. } => state.selected_path(),
            Self::MissingFileResolution(s) => s.selected_path(),
            Self::MissingDirectoryResolution(s) => s.selected_path(),
            Self::CorruptFileResolution(s) => s.selected_path(),
            Self::ShitFormatResolution(s) => s.selected_path(),
            Self::SubparDuplicateResolution(s) => s.selected_path(),
            Self::InboxCorpusMatchResolution(s) => s.selected_path(),
            Self::InboxOrganize(_) => None,
            Self::DirectoryClusterResolution(s) => s.selected_path(),
            Self::MovedFileAcknowledge(s) => s.selected_path(),
            Self::OobSyncResolution(s) => s.selected_path(),
            Self::OobConflictInspection(s) => s.selected_path(),
            Self::ExternalMatchReview(s) => s.selected_path(),
            Self::Deploy(s) => s.selected_path(),
            Self::UnifiedTagEditor(s) => s.selected_path(),
            Self::MissingAlbumSingleResolution(s) => s.selected_path(),
            Self::DiscExtractionResolution(s) => s.selected_path(),
            Self::ManualReview(s) => s.selected_path(),
            Self::Inbox(s) => s.selected_entry().map(|e| e.label.as_str()),
            _ => None,
        }
    }

    /// Return the LateralView variant for this view, if it's a lateral view.
    pub(crate) fn lateral_view(&self) -> Option<crate::ui::widgets::LateralView> {
        use crate::ui::widgets::LateralView;
        match self {
            Self::ConfigEditor(_) => Some(LateralView::Config),
            Self::TagSearch(_) => Some(LateralView::Search),
            Self::CorpusBrowser(_) => Some(LateralView::Files),
            Self::Insights(_) => Some(LateralView::Health),
            Self::History(_) => Some(LateralView::History),
            Self::Inbox(_) => Some(LateralView::Inbox),
            Self::TabbedTransactionReview(_) => Some(LateralView::Transaction),
            Self::Deploy(_) => Some(LateralView::Deploy),
            Self::ExternalMatches(_) => Some(LateralView::ExternalMatches),
            _ => None,
        }
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
    /// TagCanonicityResolution needs DB reload on restore.
    TagCanonicityReload { clusters: TagCanonicityClusters },
    /// CompoundTagSplit needs DB reload on restore.
    CompoundTagSplitReload {
        clusters: compound_split_v2::CompoundSplitClustersV2,
        safe_mode: bool,
        zone: crate::db::types::Zone,
    },
}

// ============================================================================
// ViewAction - two-phase key dispatch wrapper
// ============================================================================

/// Wrapper for view-specific actions, produced by Phase 1 (borrow view) and
/// consumed by Phase 2 (dispatch on &mut self).
pub(crate) enum ViewAction {
    None,
    ConfigEditor(config_editor::ConfigEditorAction),
    Insights(insights_view::InsightsAction),
    CorpusBrowser(tree_browser::TreeBrowserAction),
    TagSearch(tag_search::TagSearchAction),
    Inbox(inbox_view::InboxAction),
    TabbedTransactionReview(tabbed_transaction_review::TabbedTransactionReviewAction),
    Deploy(deploy_modal::DeployAction),
    ExternalMatches(external_match_view::ExternalMatchesAction),
    ExitConfirm(ExitConfirmAction),
    IntakeConfirmation(startup::IntakeConfirmationAction),
    UnifiedTagEditor(tag_editor::UnifiedTagEditorAction),
    MissingFileResolution(missing_file_modal::MissingFilePreviewAction),
    MissingDirectoryResolution(missing_directory_modal::MissingDirectoryPreviewAction),
    CorruptFileResolution(corrupt_file_modal::CorruptFilePreviewAction),
    ShitFormatResolution(shit_format_modal::ShitFormatPreviewAction),
    SubparDuplicateResolution(subpar_duplicate_modal::SubparDuplicatePreviewAction),
    InboxCorpusMatchResolution(inbox_corpus_match_modal::InboxCorpusMatchPreviewAction),
    InboxOrganize(inbox_organize::InboxOrganizeAction),
    DirectoryClusterResolution(directory_cluster_modal::DirectoryClusterPreviewAction),
    MovedFileAcknowledge(moved_file_modal::MovedFileAction),
    OobSyncResolution(oob_sync_modal::OobSyncAction),
    OobConflictInspection(oob_conflict_modal::OobConflictAction),
    ExternalMatchReview(external_match_modal::ExternalMatchReviewAction),
    ReleasePackingBrowser(super::release_packing_browser::ReleasePackingBrowserAction),
    KnotBrowser(super::knot_browser::KnotBrowserAction),
    History(history_view::HistoryAction),
    TagCanonicityResolution(tag_canonicity_v2::TagCanonicalityActionV2),
    CompoundTagSplit(compound_split_v2::CompoundSplitActionV2),
    MissingAlbumSingleResolution(missing_album_modal::MissingAlbumAction),
    DiscExtractionResolution(disc_extraction_modal::DiscExtractionAction),
    ManualReview(manual_review_modal::ManualReviewAction),
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
    pub button_rects: crate::ui::widgets::ButtonRects,
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

// ============================================================================
// TagCanonicityClusters - signal navigation
// ============================================================================

/// Which typed signal table this cluster modal targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) enum CanonicitySignalKind {
    /// `signal_tag_canonicity` table (pre-fill enabled)
    TagCanonicity,
    /// `signal_inconsistent_album_artist` table (no pre-fill)
    InconsistentAlbumArtist,
    /// `signal_inbox_tag_canonicity` table (pre-fill enabled, inbox zone)
    InboxTagCanonicity,
}

/// Tracks the list of signals for Tab/Shift-Tab navigation in tag canonicity modal.
#[derive(Debug, Clone)]
pub(crate) struct TagCanonicityClusters {
    /// Signal keys in navigation order
    pub signal_keys: Vec<String>,
    /// Current index into signal_keys
    pub current_index: usize,
    /// Which typed signal table these keys belong to
    pub kind: CanonicitySignalKind,
}

impl TagCanonicityClusters {
    pub fn new(signal_keys: Vec<String>, kind: CanonicitySignalKind) -> Self {
        Self {
            signal_keys,
            current_index: 0,
            kind,
        }
    }

    /// Whether the canonical value text field should be pre-filled.
    pub fn pre_fill(&self) -> bool {
        matches!(
            self.kind,
            CanonicitySignalKind::TagCanonicity | CanonicitySignalKind::InboxTagCanonicity
        )
    }

    pub fn current_signal_key(&self) -> Option<&str> {
        self.signal_keys.get(self.current_index).map(|s| s.as_str())
    }

    pub fn next(&mut self) -> bool {
        if self.current_index + 1 < self.signal_keys.len() {
            self.current_index += 1;
            true
        } else {
            false
        }
    }

    pub fn prev(&mut self) -> bool {
        if self.current_index > 0 {
            self.current_index -= 1;
            true
        } else {
            false
        }
    }

    pub fn is_last(&self) -> bool {
        self.current_index + 1 >= self.signal_keys.len()
    }
}

