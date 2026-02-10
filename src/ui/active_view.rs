//! ActiveView - the single source of truth for which view is active and its state.
//!
//! Replaces the old `UiMode` enum + 25 `Option<State>` fields pattern.
//! Each variant carries its own state, so the type system guarantees that
//! mode and state are always consistent.

use crate::ui::{
    compound_split_v2,
    corrupt_file_flow,
    deploy_flow,
    directory_cluster_flow,
    eye::Eye,
    filter_popup,
    insights_view,
    missing_directory_flow,
    missing_file_flow,
    moved_file_flow,
    oob_conflict_flow,
    oob_sync_flow,
    progress_screen,
    progressive_worker,
    shit_format_flow,
    startup,
    subpar_duplicate_flow,
    tag_canonicity_v2,
    tag_editor,
    tag_search,
    transaction_review,
    tree_browser,
};

// ============================================================================
// ActiveView
// ============================================================================

/// The active view and its state. One variant is active at a time.
pub(crate) enum ActiveView {
    // Lateral view ring
    Insights(insights_view::InsightsViewState),
    CorpusBrowser(tree_browser::TreeBrowserState),
    TagSearch(tag_search::TagSearchState),

    // Progress (non-interactive, owns eye animation)
    Progress {
        screen: progress_screen::ProgressScreen,
        eye: Eye,
    },
    ProgressiveWork {
        worker: progressive_worker::ProgressiveWorkerState,
        return_context: Box<SuspendedView>,
    },

    // Simple modals
    ExitConfirm(ExitConfirmModalState),
    IntakeConfirmation(startup::IntakeConfirmationState),

    // Editors / previews
    UnifiedTagEditor(tag_editor::UnifiedTagEditorState),
    DeploymentPreview(deploy_flow::DeploymentPreviewState),

    // Resolution flows (single state)
    MissingFileResolution(missing_file_flow::MissingFilePreviewState),
    MissingDirectoryResolution(missing_directory_flow::MissingDirectoryPreviewState),
    CorruptFileResolution(corrupt_file_flow::CorruptFilePreviewState),
    ShitFormatResolution(shit_format_flow::ShitFormatPreviewState),
    SubparDuplicateResolution(subpar_duplicate_flow::SubparDuplicatePreviewState),
    DirectoryClusterResolution(directory_cluster_flow::DirectoryClusterPreviewState),
    MovedFileAcknowledge(moved_file_flow::MovedFileState),
    OobSyncResolution(oob_sync_flow::OobSyncState),
    OobConflictInspection(oob_conflict_flow::OobConflictState),

    // Resolution flows (companion state bundled)
    TagCanonicityResolution {
        state: tag_canonicity_v2::TagCanonicalityStateV2,
        clusters: TagCanonicityClusters,
    },
    CompoundTagSplit {
        state: compound_split_v2::CompoundSplitStateV2,
        clusters: compound_split_v2::CompoundSplitClustersV2,
        safe_mode: bool,
    },

    // Transaction review (suspends source view)
    TransactionReview {
        review: transaction_review::TransactionReviewState,
        suspended: Box<SuspendedView>,
    },
}

impl ActiveView {
    /// Get a header suffix for the title bar, if applicable.
    pub(crate) fn header_suffix(&self) -> Option<&'static str> {
        match self {
            Self::Insights(_) => Some("Corpus Insights"),
            Self::CorpusBrowser(_) => Some("Corpus Browser"),
            Self::TagSearch(_) => Some("Tag Search"),
            Self::Progress { .. } => None,
            Self::ProgressiveWork { .. } => Some("Processing"),
            Self::ExitConfirm(_) => Some("Exit Confirmation"),
            Self::IntakeConfirmation(_) => Some("Intake Confirmation"),
            Self::UnifiedTagEditor(_) => Some("Tag Editor"),
            Self::DeploymentPreview(_) => Some("Deployment Preview"),
            Self::MissingFileResolution(_) => Some("Missing File Resolution"),
            Self::MissingDirectoryResolution(_) => Some("Missing Directory Acknowledgment"),
            Self::CorruptFileResolution(_) => Some("Corrupt File Resolution"),
            Self::ShitFormatResolution(_) => Some("Shit Format Resolution"),
            Self::SubparDuplicateResolution(_) => Some("Subpar Duplicate Resolution"),
            Self::DirectoryClusterResolution(_) => Some("Directory Overlap Resolution"),
            Self::MovedFileAcknowledge(_) => Some("Moved Files"),
            Self::OobSyncResolution(_) => Some("OOB Tag Sync"),
            Self::OobConflictInspection(_) => Some("OOB Tag Conflicts"),
            Self::TagCanonicityResolution { .. } => Some("Tag Canonicity"),
            Self::CompoundTagSplit { .. } => Some("Compound Tag Split"),
            Self::TransactionReview { .. } => Some("Transaction Review"),
        }
    }

    /// Whether this view uses the unified lateral title bar (no header row).
    pub(crate) fn uses_unified_titlebar(&self) -> bool {
        matches!(
            self,
            Self::Insights(_) | Self::CorpusBrowser(_) | Self::TagSearch(_)
        )
    }
}

// ============================================================================
// SuspendedView - for TransactionReview and ProgressiveWork return navigation
// ============================================================================

/// Captures enough context to restore the previous view when returning from
/// TransactionReview (Cancel) or ProgressiveWork (completion).
pub(crate) enum SuspendedView {
    /// Restore view directly (most modals).
    Direct(ActiveView),
    /// TagCanonicityResolution needs DB reload on restore.
    TagCanonicityReload { clusters: TagCanonicityClusters },
    /// CompoundTagSplit needs DB reload on restore.
    CompoundTagSplitReload {
        clusters: compound_split_v2::CompoundSplitClustersV2,
        safe_mode: bool,
    },
}

// ============================================================================
// ViewAction - two-phase key dispatch wrapper
// ============================================================================

/// Wrapper for view-specific actions, produced by Phase 1 (borrow view) and
/// consumed by Phase 2 (dispatch on &mut self).
pub(crate) enum ViewAction {
    None,
    Insights(insights_view::InsightsAction),
    CorpusBrowser(tree_browser::TreeBrowserAction),
    TagSearch(tag_search::TagSearchAction),
    ExitConfirm(ExitConfirmAction),
    IntakeConfirmation(startup::IntakeConfirmationAction),
    UnifiedTagEditor(tag_editor::UnifiedTagEditorAction),
    DeploymentPreview(deploy_flow::DeploymentPreviewAction),
    MissingFileResolution(missing_file_flow::MissingFilePreviewAction),
    MissingDirectoryResolution(missing_directory_flow::MissingDirectoryPreviewAction),
    CorruptFileResolution(corrupt_file_flow::CorruptFilePreviewAction),
    ShitFormatResolution(shit_format_flow::ShitFormatPreviewAction),
    SubparDuplicateResolution(subpar_duplicate_flow::SubparDuplicatePreviewAction),
    DirectoryClusterResolution(directory_cluster_flow::DirectoryClusterPreviewAction),
    MovedFileAcknowledge(moved_file_flow::MovedFileAction),
    OobSyncResolution(oob_sync_flow::OobSyncAction),
    OobConflictInspection(oob_conflict_flow::OobConflictAction),
    TagCanonicityResolution(tag_canonicity_v2::TagCanonicalityActionV2),
    CompoundTagSplit(compound_split_v2::CompoundSplitActionV2),
    TransactionReview(transaction_review::TransactionReviewAction),
}

// ============================================================================
// ExitConfirmAction - extracted from inline key handling
// ============================================================================

/// Action from the exit confirmation modal.
pub(crate) enum ExitConfirmAction {
    None,
    Quit,
    Cancel,
}

// ============================================================================
// ExitConfirmModalState
// ============================================================================

/// State for the exit confirmation modal.
/// Default selection is "No" (stay in application).
#[derive(Debug, Clone, Default)]
pub(crate) struct ExitConfirmModalState {
    /// True = "No" selected (default), False = "Yes" selected
    pub selected_no: bool,
    /// True if operations are in progress (shows warning)
    pub has_operations: bool,
}

// ============================================================================
// FilterOverlay - bundles filter popup state + context
// ============================================================================

/// Overlay for the filter popup (Ctrl+F). Always paired: state + context.
pub(crate) struct FilterOverlay {
    pub state: filter_popup::FilterPopupState,
    pub context: FilterPopupContext,
}

/// Context for filter popup - determines where to apply filter results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilterPopupContext {
    /// Filter for corpus browser tree
    CorpusBrowser,
    /// Filter for OOB sync resolution flow
    OobSync,
    /// Filter for OOB conflict resolution flow
    OobConflict,
}

// ============================================================================
// TagCanonicityClusters - signal navigation
// ============================================================================

/// Which typed signal table this cluster flow targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanonicitySignalKind {
    /// `signal_tag_canonicity` table (pre-fill enabled)
    TagCanonicity,
    /// `signal_inconsistent_album_artist` table (no pre-fill)
    InconsistentAlbumArtist,
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
        Self { signal_keys, current_index: 0, kind }
    }

    /// Whether the canonical value text field should be pre-filled.
    pub fn pre_fill(&self) -> bool {
        self.kind == CanonicitySignalKind::TagCanonicity
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
