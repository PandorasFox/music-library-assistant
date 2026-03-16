//! ActiveView - the single source of truth for which view is active and its state.
//!
//! Replaces the old `UiMode` enum + 25 `Option<State>` fields pattern.
//! Each variant carries its own state, so the type system guarantees that
//! mode and state are always consistent.

use crate::{
    acoustid_browse, config_editor, corrupt_file_modal, deploy_modal,
    external_match_view, eye::Eye,
    history_view, inbox_corpus_match_modal, inbox_organize, inbox_view, insights_view,
    lossless_remux_modal, missing_directory_modal, missing_file_modal,
    moved_file_modal, oob_conflict_modal, progress_screen, progressive_worker,
    release_review, startup, subpar_duplicate_modal, tabbed_transaction_review,
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
    MissingDirectoryResolution(missing_directory_modal::MissingDirectoryState),
    CorruptFileResolution(corrupt_file_modal::CorruptFileState),
    LosslessRemuxResolution(lossless_remux_modal::LosslessRemuxPreviewState),
    SubparDuplicateResolution(subpar_duplicate_modal::SubparDuplicateState),
    InboxCorpusMatchResolution(inbox_corpus_match_modal::InboxCorpusMatchState),
    InboxOrganize(inbox_organize::InboxOrganizeState),
    DirectoryClusterResolution(mm_ui::resolutions::directory_cluster::DirectoryClusterState),
    MovedFileAcknowledge(moved_file_modal::MovedFileState),
    OobResolution(oob_conflict_modal::OobResolutionState),
    // Release packing browser (read-only)
    ReleasePackingBrowser(super::release_packing_browser::ReleasePackingBrowserState),

    // Knot browser (read-only)
    KnotBrowser(super::knot_browser::KnotBrowserState),

    // AcoustID browse (read-only, by confidence tier)
    AcoustidBrowse(acoustid_browse::AcoustidBrowseState),

    // Release review (multi-select approval)
    ReleaseReview(release_review::ReleaseReviewState),

    // Tag canonicity resolution with packed data + StandardList + DecisionField
    TagCanonicityResolution(mm_ui::resolutions::tag_canonicity::TagCanonicityViewState),
    // Compound tag split with packed data + StandardList + DecisionField
    CompoundTagSplitResolution(mm_ui::resolutions::compound_split::CompoundSplitViewState),

    MissingAlbumSingleResolution(mm_ui::resolutions::missing_album::MissingAlbumState),
    DiscExtractionResolution(mm_ui::resolutions::disc_extraction::DiscExtractionState),
    ManualReviewResolution(mm_ui::resolutions::manual_review::ManualReviewState),

    // Transaction review (view stack holds suspended views)
    TransactionReview(transaction_review::TransactionReviewState),

    // Login screen (shown on session expiry, re-authenticates in-place)
    LoginScreen(LoginScreenState),
}

/// Focus state for the login form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoginField {
    Username,
    Password,
}

/// State for the in-app login screen (shown on session expiry).
pub(crate) struct LoginScreenState {
    pub username: crate::widgets::TextInputState,
    pub password: crate::widgets::TextInputState,
    pub focused: LoginField,
    pub error_message: Option<String>,
}

impl LoginScreenState {
    pub fn new(message: Option<String>) -> Self {
        let mut username = crate::widgets::TextInputState::new();
        username.focused = true;
        Self {
            username,
            password: crate::widgets::TextInputState::new(),
            focused: LoginField::Username,
            error_message: message,
        }
    }

    pub fn focus_next(&mut self) {
        self.username.focused = false;
        self.password.focused = false;
        match self.focused {
            LoginField::Username => {
                self.focused = LoginField::Password;
                self.password.focused = true;
            }
            LoginField::Password => {
                self.focused = LoginField::Username;
                self.username.focused = true;
            }
        }
    }
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
            Self::LosslessRemuxResolution(_) => Some("Lossless Remux Resolution"),
            Self::SubparDuplicateResolution(_) => Some("Subpar Duplicate Resolution"),
            Self::InboxCorpusMatchResolution(_) => Some("Inbox Corpus Match Resolution"),
            Self::InboxOrganize(_) => Some("Inbox Organize"),
            Self::DirectoryClusterResolution(_) => Some("Directory Overlap Resolution"),
            Self::MovedFileAcknowledge(_) => Some("Moved Files"),
            Self::OobResolution(_) => Some("OOB Resolution"),
            Self::ReleasePackingBrowser(_) => Some("Release Packing Browser"),
            Self::KnotBrowser(_) => Some("Knot Browser"),
            Self::AcoustidBrowse(_) => Some("AcoustID Browse"),
            Self::ReleaseReview(_) => Some("Release Review"),
            Self::TagCanonicityResolution(ref s) => match s.state.data.mode {
                mm_ui::resolutions::tag_canonicity::CanonicityMode::InconsistentAlbumArtist => Some("Album Artist"),
                mm_ui::resolutions::tag_canonicity::CanonicityMode::TagCanonicity => Some("Tag Canonicity"),
            },
            Self::CompoundTagSplitResolution(_) => Some("Compound Tag Split"),
            Self::MissingAlbumSingleResolution(_) => Some("Missing Album Singles"),
            Self::DiscExtractionResolution(_) => Some("Disc Extraction"),
            Self::ManualReviewResolution(ref s) => Some(s.data.review_kind.title()),
            Self::TransactionReview(_) => Some("Transaction Review"),
            Self::LoginScreen(_) => None,
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
            Self::TagCanonicityResolution(ref s) => s.selected_path(),
            Self::CompoundTagSplitResolution(ref s) => s.selected_path(),
            Self::MissingFileResolution(s) => s.selected_path(),
            Self::MissingDirectoryResolution(s) => s.selected_path(),
            Self::CorruptFileResolution(s) => s.selected_path(),
            Self::LosslessRemuxResolution(s) => s.selected_path(),
            Self::SubparDuplicateResolution(s) => s.selected_path(),
            Self::InboxCorpusMatchResolution(s) => s.selected_path(),
            Self::InboxOrganize(_) => None,
            Self::DirectoryClusterResolution(ref s) => s.selected_path(),
            Self::MovedFileAcknowledge(s) => s.selected_path(),
            Self::OobResolution(s) => s.selected_path(),
            Self::AcoustidBrowse(s) => s.selected_path(),
            Self::ReleaseReview(s) => s.selected_path(),
            Self::Deploy(ref s) => s.data.selected_path(&s.interaction),
            Self::UnifiedTagEditor(s) => s.selected_path(),
            Self::MissingAlbumSingleResolution(ref s) => s.selected_path(),
            Self::DiscExtractionResolution(ref s) => s.selected_path(),
            Self::ManualReviewResolution(ref s) => s.selected_path(),
            Self::Inbox(ref s) => s.data.selected_entry(s.interaction.list.cursor).map(|e| e.label.as_str()),
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
            Self::Insights(_) => Some(LateralView::Health),
            Self::History(_) => Some(LateralView::History),
            Self::Inbox(_) => Some(LateralView::Inbox),
            Self::TabbedTransactionReview(_) => Some(LateralView::Transaction),
            Self::Deploy(_) => Some(LateralView::Deploy),
            Self::ExternalMatches(_) => Some(LateralView::ExternalMatches),
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

    /// Extract the current Route for this view, if representable.
    ///
    /// Lateral views always produce a Route. Overlay views (resolution modals,
    /// tag editor, transaction review) return None for now — they carry
    /// in-flight state that isn't URL-encodable yet.
    pub(crate) fn to_route(&self) -> Option<mm_ui::route::Route> {
        use mm_ui::route::Route;
        match self {
            Self::Insights(s) => Some(Route::Health(s.to_route())),
            Self::History(s) => Some(Route::History(s.to_route())),
            Self::Inbox(s) => Some(Route::Inbox(s.to_route())),
            Self::Deploy(s) => Some(Route::Deploy(s.to_route())),
            Self::ExternalMatches(s) => Some(Route::ExternalMatches(s.to_route())),
            Self::ConfigEditor(_) => Some(Route::Config(Default::default())),
            Self::TagSearch(_) => Some(Route::Search(Default::default())),
            Self::CorpusBrowser(_) => Some(Route::Files(Default::default())),
            Self::TabbedTransactionReview(_) => Some(Route::Transaction(Default::default())),
            // Overlay and non-interactive views don't have URL-encodable routes yet
            _ => None,
        }
    }
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
    LosslessRemuxResolution(lossless_remux_modal::LosslessRemuxAction),
    SubparDuplicateResolution(subpar_duplicate_modal::SubparDuplicateAction),
    InboxCorpusMatchResolution(inbox_corpus_match_modal::InboxCorpusMatchAction),
    InboxOrganize(inbox_organize::InboxOrganizeAction),
    DirectoryClusterResolution(mm_ui::resolutions::directory_cluster::DirectoryClusterAction),
    MovedFileAcknowledge(moved_file_modal::MovedFileAction),
    OobResolution(oob_conflict_modal::OobAction),
    ReleasePackingBrowser(super::release_packing_browser::ReleasePackingBrowserAction),
    KnotBrowser(super::knot_browser::KnotBrowserAction),
    AcoustidBrowse(acoustid_browse::AcoustidBrowseAction),
    ReleaseReview(release_review::ReleaseReviewAction),
    History(history_view::HistoryAction),
    TagCanonicityResolution(mm_ui::resolutions::tag_canonicity::CanonicityAction),
    CompoundTagSplitResolution(mm_ui::resolutions::compound_split::CompoundSplitAction),
    MissingAlbumSingleResolution(mm_ui::resolutions::missing_album::MissingAlbumAction),
    DiscExtractionResolution(mm_ui::resolutions::disc_extraction::DiscExtractionAction),
    ManualReviewResolution(mm_ui::resolutions::manual_review::ReviewAction),
    TransactionReview(transaction_review::TransactionReviewAction),
    LoginScreen(LoginAction),
}

pub(crate) enum LoginAction {
    None,
    AttemptLogin,
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


