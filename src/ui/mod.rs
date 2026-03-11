//! Terminal User Interface
//!
//! ## Module Organization
//! - `active_view.rs` - ActiveView enum (single source of truth for view + state)
//! - `types.rs` - ProgressStatsUpdater trait
//! - `action_handlers/` - View-specific action processing
//! - `tag_editor_ops.rs` - Tag editor launching and navigation
//! - `tick.rs` - Per-frame update logic for each view
//!
//! ## Adding New Views
//! 1. Add variant to `ActiveView` in `active_view.rs`
//! 2. Add ViewAction variant in `active_view.rs`
//! 3. Add key dispatch + action handler in `action_handlers/`
//! 4. Add render case in `render.rs`

pub(crate) mod action_handlers;
pub(crate) mod active_view;
pub(crate) mod input;
mod suspended_views;
mod tag_editor_ops;
mod tick;
mod types;

pub mod operator_decisions;
pub mod tabbed_transaction_review;
pub mod transaction_review;

pub mod bulk_selection;
pub mod compound_split_v2;
pub mod config_editor;
pub mod corrupt_file_modal;
pub mod deploy_modal;
pub mod directory_cluster_modal;
pub mod disc_extraction_modal;
pub mod external_match_modal;
pub mod external_match_view;
pub mod eye;
pub mod filter_popup;
pub mod helpers;
pub mod history_view;
pub mod inbox_corpus_match_modal;
pub mod inbox_organize;
pub mod inbox_view;
pub mod insights_view;
pub mod knot_browser;
pub mod manual_review_modal;
pub mod missing_album_modal;
pub mod missing_directory_modal;
pub mod missing_file_modal;
pub mod moved_file_modal;
pub mod oob_conflict_modal;
pub mod oob_sync_modal;
pub mod progress_screen;
pub mod progressive_worker;
pub mod release_packing_browser;
pub mod render;
pub mod shit_format_modal;
pub mod startup;
pub mod subpar_duplicate_modal;
pub mod tag_canonicity_v2;
pub mod tag_editor;
pub mod tag_search;
pub mod tree_browser;
pub mod wait_state;
pub mod widgets;

// Re-export for convenience
pub(crate) use active_view::{
    ActiveView, CanonicitySignalKind, ExitConfirmAction, ExitConfirmModalState, FilterOverlay,
    FilterPopupContext, SchemaUpdateAction, SchemaUpdatePhase, SchemaUpdateState, SuspendedView,
    TagCanonicityClusters, VacuumAction, VacuumPhase, VacuumPromptState, ViewAction,
};
use types::ProgressStatsUpdater;

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use input::InputAction;
use ratatui::{backend::CrosstermBackend, Frame, Terminal};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::config::{Config, SharedConfig};

// ============================================================================
// Application State
// ============================================================================

/// Main application state
pub(crate) struct App {
    pub(super) shared_config: SharedConfig,
    should_quit: bool,
    pub(super) status_message: Option<String>,

    /// The active view and its state. One variant is active at a time.
    pub(super) view: ActiveView,

    // The Witch - enforcer of orderliness, handles all mutations and background work
    pub(super) witch: crate::witch::Witch,

    /// Handle to the cache thread for periodic refreshes and one-shot queries.
    pub(super) cache: crate::witch::cache_thread::CacheHandle,

    /// Locally cached periodic data from the cache thread.
    pub(super) cached_insights: Option<crate::meta::views::InsightsData>,
    pub(super) cached_inbox: Option<crate::meta::views::InboxOverviewData>,
    pub(super) cached_deploy: Option<crate::meta::views::DeployStatus>,
    pub(super) cached_history: Option<crate::meta::views::EditHistoryData>,
    pub(super) cached_external_matches: Option<crate::meta::views::ExternalMatchesData>,
    pub(super) cached_packing_dirs: Option<crate::witch::cache_thread::PackingDirsData>,

    // Filter popup overlay (Ctrl+/ in resolution modals and corpus browser)
    pub(super) filter_overlay: Option<FilterOverlay>,

    // View stack for push/pop navigation (TransactionReview, ProgressiveWork, etc.)
    pub(super) view_stack: Vec<SuspendedView>,

    /// Last lateral view the user was on. Used for returning after modal flows.
    pub(super) last_lateral_view: widgets::LateralView,

    /// Database path, stored for startup flow (vacuum prompt needs it).
    pub(super) db_path: std::path::PathBuf,

    /// Vacuum threshold from config, stored for startup flow.
    pub(super) vacuum_threshold: f64,

    /// Receiver for typed Witch → UI notifications.
    notice_rx: std::sync::mpsc::Receiver<crate::witch::WitchNotice>,

    /// Locally cached Witch status from StatusUpdate notices.
    pub(super) cached_status: crate::witch::WorkStatus,

    /// Set when `MutationsCompleted` fires (cache invalidated), cleared when
    /// fresh `CacheReady` results arrive. Views stay greyed-out while true so
    /// the operator never sees stale counts with an interactive overlay.
    cache_stale: bool,

    /// Terminal image rendering: picker for protocol detection + image cache.
    pub(super) art_picker: widgets::AlbumArtPicker,
    pub(super) art_cache: widgets::AlbumArtCache,

    /// Click targets for titlebar tabs, populated during render.
    pub(super) tab_click_rects: Vec<(widgets::LateralView, ratatui::layout::Rect)>,

}

impl App {
    /// Create a new App with a pre-existing Witch instance, cache handle, and shared config.
    fn new_with_witch(
        shared_config: SharedConfig,
        witch: crate::witch::Witch,
        cache: crate::witch::cache_thread::CacheHandle,
        notice_rx: std::sync::mpsc::Receiver<crate::witch::WitchNotice>,
        art_picker: widgets::AlbumArtPicker,
    ) -> Self {
        Self {
            shared_config,
            should_quit: false,
            status_message: None,
            view: ActiveView::Insights(insights_view::InsightsViewState::new()),
            witch,
            cache,
            cached_insights: None,
            cached_inbox: None,
            cached_deploy: None,
            cached_history: None,
            cached_external_matches: None,
            cached_packing_dirs: None,
            filter_overlay: None,
            view_stack: Vec::new(),
            last_lateral_view: widgets::LateralView::Health,
            db_path: std::path::PathBuf::new(),
            vacuum_threshold: 0.0,
            notice_rx,
            cached_status: Default::default(),
            cache_stale: false,
            art_picker,
            art_cache: widgets::AlbumArtCache::new(),
            tab_click_rects: Vec::new(),
        }
    }

    /// Read-lock the shared config for accessing config values.
    pub(super) fn config(&self) -> std::sync::RwLockReadGuard<'_, Config> {
        crate::config::read_shared_config(&self.shared_config)
    }

    /// Whether the Transaction tab should be visible in the lateral view ring.
    fn transactions_open(&self) -> bool {
        self.config().opinions.leave_transactions_open
    }

    /// Whether leave-transactions-open mode is active (alias for readability in control flow).
    fn open_txn_mode(&self) -> bool {
        self.config().opinions.leave_transactions_open
    }

    /// Return to the last lateral view the user was on.
    fn return_to_last_lateral_view(&mut self) {
        self.start_lateral_view(self.last_lateral_view);
    }

    fn handle_input(&mut self, action: InputAction) {
        // Filter popup intercepts when active
        if let Some(ref mut overlay) = self.filter_overlay {
            let action = overlay.state.handle_input(&action);
            match action {
                filter_popup::FilterPopupAction::None => return,
                filter_popup::FilterPopupAction::Apply => {
                    let condition = overlay.state.condition.clone();
                    let context = overlay.context;
                    self.filter_overlay = None;
                    match context {
                        FilterPopupContext::CorpusBrowser => {
                            if !condition.is_active() {
                                if let ActiveView::CorpusBrowser(ref mut browser) = self.view {
                                    browser.clear_filter();
                                }
                            } else {
                                let matching_paths = self
                                    .cache
                                    .query(move |db| {
                                        let audio_files = db
                                            .get_all_audio_files(
                                                crate::db::types::Zone::Corpus,
                                                false,
                                            )
                                            .unwrap_or_default();
                                        let mut paths = Vec::new();
                                        for audio_file in audio_files {
                                            let mut tags: std::collections::HashMap<
                                                String,
                                                Vec<String>,
                                            > = std::collections::HashMap::new();
                                            for t in db
                                                .get_tags::<crate::zones::CorpusZone>(audio_file.inode())
                                                .unwrap_or_default()
                                            {
                                                tags.entry(t.tag_name.to_uppercase())
                                                    .or_default()
                                                    .push(t.tag_value);
                                            }
                                            if condition.matches(
                                                audio_file.path(),
                                                &audio_file.audio.file_type,
                                                audio_file.audio.sample_rate,
                                                audio_file.audio.bitrate_kbps,
                                                audio_file.audio.duration_ms,
                                                &tags,
                                            ) {
                                                paths.push(std::path::PathBuf::from(
                                                    audio_file.path(),
                                                ));
                                            }
                                        }
                                        paths
                                    })
                                    .recv();
                                if let ActiveView::CorpusBrowser(ref mut browser) = self.view {
                                    browser.apply_filter_results(matching_paths);
                                }
                            }
                        }
                        FilterPopupContext::OobSync => {
                            if let ActiveView::OobSyncResolution(ref mut state) = self.view {
                                state.apply_filter(condition);
                            }
                        }
                        FilterPopupContext::OobConflict => {
                            if let ActiveView::OobConflictInspection(ref mut state) = self.view {
                                state.apply_filter(condition);
                            }
                        }
                    }
                    return;
                }
                filter_popup::FilterPopupAction::Clear => {
                    let context = overlay.context;
                    self.filter_overlay = None;
                    match context {
                        FilterPopupContext::CorpusBrowser => {
                            if let ActiveView::CorpusBrowser(ref mut browser) = self.view {
                                browser.clear_filter();
                            }
                        }
                        FilterPopupContext::OobSync => {
                            if let ActiveView::OobSyncResolution(ref mut state) = self.view {
                                state.clear_filter();
                            }
                        }
                        FilterPopupContext::OobConflict => {
                            if let ActiveView::OobConflictInspection(ref mut state) = self.view {
                                state.clear_filter();
                            }
                        }
                    }
                    return;
                }
                filter_popup::FilterPopupAction::Cancel => {
                    self.filter_overlay = None;
                    return;
                }
            }
        }

        // Macro for the common pattern: delegate handle_input, wrap in ViewAction
        macro_rules! dispatch_input {
            ($variant:ident, $state:expr) => {
                ViewAction::$variant($state.handle_input(&action))
            };
        }

        // Phase 1: borrow view, produce view action
        let view_action = match &mut self.view {
            ActiveView::SchemaUpdate(s) => {
                let a = match (s.phase, &action) {
                    (SchemaUpdatePhase::Approval, InputAction::Confirm) => {
                        SchemaUpdateAction::Approve
                    }
                    (SchemaUpdatePhase::Approval, InputAction::Cancel) => {
                        SchemaUpdateAction::Cancel
                    }
                    _ => SchemaUpdateAction::None,
                };
                ViewAction::SchemaUpdate(a)
            }
            ActiveView::VacuumPrompt(s) => {
                let a = match (s.phase, &action) {
                    (VacuumPhase::Prompt, InputAction::Confirm) => VacuumAction::Compact,
                    (VacuumPhase::Prompt, InputAction::Cancel) => VacuumAction::Skip,
                    _ => VacuumAction::None,
                };
                ViewAction::VacuumPrompt(a)
            }
            ActiveView::Progress { .. } => ViewAction::None,
            ActiveView::ProgressiveWork(_) => ViewAction::None,
            ActiveView::TagCanonicityLoading { .. } => ViewAction::None,
            ActiveView::ConfigEditor(s) => dispatch_input!(ConfigEditor, s),
            ActiveView::Insights(s) => dispatch_input!(Insights, s),
            ActiveView::CorpusBrowser(s) => dispatch_input!(CorpusBrowser, s),
            ActiveView::TagSearch(s) => dispatch_input!(TagSearch, s),
            ActiveView::Inbox(s) => dispatch_input!(Inbox, s),
            ActiveView::TabbedTransactionReview(ref mut s) => dispatch_input!(TabbedTransactionReview, s),
            ActiveView::ExitConfirm(state) => {
                let a = match action {
                    InputAction::NavLeft | InputAction::NavRight | InputAction::FocusLeft | InputAction::FocusRight => {
                        state.selected_no = !state.selected_no;
                        ExitConfirmAction::None
                    }
                    InputAction::Confirm | InputAction::Toggle => {
                        if state.selected_no {
                            ExitConfirmAction::Cancel
                        } else {
                            ExitConfirmAction::Quit
                        }
                    }
                    InputAction::Cancel => ExitConfirmAction::Cancel,
                    _ => ExitConfirmAction::None,
                };
                ViewAction::ExitConfirm(a)
            }
            ActiveView::IntakeConfirmation(state) => {
                let visible_height = crossterm::terminal::size()
                    .map(|(_, h)| {
                        startup::intake_confirmation::compute_list_visible_height(
                            ratatui::layout::Rect::new(0, 0, 80, h),
                        )
                    })
                    .unwrap_or(10);
                ViewAction::IntakeConfirmation(state.handle_input(&action, visible_height))
            }
            ActiveView::UnifiedTagEditor(s) => dispatch_input!(UnifiedTagEditor, s),
            ActiveView::Deploy(s) => dispatch_input!(Deploy, s),
            ActiveView::ExternalMatches(s) => dispatch_input!(ExternalMatches, s),
            ActiveView::MissingFileResolution(s) => dispatch_input!(MissingFileResolution, s),
            ActiveView::MissingDirectoryResolution(s) => dispatch_input!(MissingDirectoryResolution, s),
            ActiveView::CorruptFileResolution(s) => dispatch_input!(CorruptFileResolution, s),
            ActiveView::ShitFormatResolution(s) => dispatch_input!(ShitFormatResolution, s),
            ActiveView::SubparDuplicateResolution(s) => dispatch_input!(SubparDuplicateResolution, s),
            ActiveView::InboxCorpusMatchResolution(s) => dispatch_input!(InboxCorpusMatchResolution, s),
            ActiveView::InboxOrganize(s) => dispatch_input!(InboxOrganize, s),
            ActiveView::DirectoryClusterResolution(s) => dispatch_input!(DirectoryClusterResolution, s),
            ActiveView::MovedFileAcknowledge(s) => dispatch_input!(MovedFileAcknowledge, s),
            ActiveView::OobSyncResolution(s) => dispatch_input!(OobSyncResolution, s),
            ActiveView::OobConflictInspection(s) => dispatch_input!(OobConflictInspection, s),
            ActiveView::ExternalMatchReview(s) => dispatch_input!(ExternalMatchReview, s),
            ActiveView::ReleasePackingBrowser(s) => dispatch_input!(ReleasePackingBrowser, s),
            ActiveView::KnotBrowser(s) => dispatch_input!(KnotBrowser, s),
            ActiveView::History(s) => dispatch_input!(History, s),
            ActiveView::TagCanonicityResolution { state, .. } => dispatch_input!(TagCanonicityResolution, state),
            ActiveView::CompoundTagSplit { state, .. } => dispatch_input!(CompoundTagSplit, state),
            ActiveView::MissingAlbumSingleResolution(s) => dispatch_input!(MissingAlbumSingleResolution, s),
            ActiveView::DiscExtractionResolution(s) => dispatch_input!(DiscExtractionResolution, s),
            ActiveView::ManualReview(s) => dispatch_input!(ManualReview, s),
            ActiveView::TransactionReview(s) => dispatch_input!(TransactionReview, s),
        };

        // Phase 2: dispatch with confirmation flag
        let is_confirmation = matches!(action, InputAction::Confirm);
        self.dispatch_action(view_action, is_confirmation);
    }

    /// Check if there are any pending operations (Witch work).
    pub(super) fn has_pending_operations(&self) -> bool {
        self.witch.has_pending()
    }

    /// Start the health view.
    pub(super) fn start_health_view(&mut self) {
        self.clear_view_stack();
        self.last_lateral_view = widgets::LateralView::Health;
        self.view = ActiveView::Insights(insights_view::InsightsViewState::new());
    }

    /// Start the configured default view (post-startup landing screen).
    pub(super) fn start_default_view(&mut self) {
        let default_view = self.config().opinions.startup.default_view;
        match default_view {
            crate::config::StartupView::Health => self.start_health_view(),
            crate::config::StartupView::Search => self.start_tag_search(),
            crate::config::StartupView::Browser => self.start_corpus_browser(),
            crate::config::StartupView::Inbox => self.start_inbox_view(),
            crate::config::StartupView::ExternalMatches => self.start_external_matches_view(),
        }
    }

    /// Abort current operation and return to health view with a status message.
    pub(super) fn abort_to_health(&mut self, message: String) {
        self.status_message = Some(message);
        self.start_health_view();
    }

    /// Update Witch stats on a progress state that implements the stats setter methods.
    pub(super) fn update_progress_stats<T>(&self, state: &mut T)
    where
        T: ProgressStatsUpdater,
    {
        state.set_db_queue_depth(self.witch.db_queue_depth());
    }

    pub(super) fn start_tag_search(&mut self) {
        self.last_lateral_view = widgets::LateralView::Search;
        self.view = ActiveView::TagSearch(tag_search::TagSearchState::new());
    }

    pub(super) fn start_inbox_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::Inbox;
        // Check for inbox unindexed files — show intake popup if any
        let intake_state = self
            .cache
            .query(|db| {
                startup::IntakeConfirmationState::gather_zone::<crate::zones::InboxZone>(db, startup::IntakeSource::Inbox)
            })
            .recv();

        if let Some(state) = intake_state {
            self.view = ActiveView::IntakeConfirmation(state);
        } else {
            self.view = ActiveView::Inbox(inbox_view::InboxViewState::new());
        }
    }

    /// Start the history lateral view.
    pub(super) fn start_history_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::History;
        self.view = ActiveView::History(history_view::HistoryViewState::new());
    }

    /// Start the external matches lateral view.
    pub(super) fn start_external_matches_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::ExternalMatches;
        let fetch_active = self.witch.is_external_fetch_active();
        let has_api_key = self.witch.has_acoustid_api_key();
        let singles_before_incompletes = self
            .config()
            .opinions
            .release_packing
            .singles_before_incompletes;
        let mut state = external_match_view::ExternalMatchesViewState::new(
            fetch_active,
            has_api_key,
            singles_before_incompletes,
        );
        if let Some(ref data) = self.cached_external_matches {
            state.update(data.clone());
        }
        self.view = ActiveView::ExternalMatches(state);
    }

    /// Start the lateral view identified by the given variant.
    pub(super) fn start_lateral_view(&mut self, view: widgets::LateralView) {
        match view {
            widgets::LateralView::Config => self.start_config_editor(),
            widgets::LateralView::Search => self.start_tag_search(),
            widgets::LateralView::Files => self.start_corpus_browser(),
            widgets::LateralView::Health => self.start_health_view(),
            widgets::LateralView::History => self.start_history_view(),
            widgets::LateralView::Inbox => self.start_inbox_view(),
            widgets::LateralView::Transaction => self.start_tabbed_transaction_review(),
            widgets::LateralView::Deploy => self.start_deploy_view(),
            widgets::LateralView::ExternalMatches => self.start_external_matches_view(),
        }
    }

    /// Start the tabbed transaction review lateral view.
    pub(super) fn start_tabbed_transaction_review(&mut self) {
        self.last_lateral_view = widgets::LateralView::Transaction;
        let mut state = tabbed_transaction_review::TabbedTransactionReviewState::new();
        state.review.refresh_decisions(&self.witch);
        self.view = ActiveView::TabbedTransactionReview(state);
    }

    /// Start the config editor view.
    pub(super) fn start_config_editor(&mut self) {
        self.last_lateral_view = widgets::LateralView::Config;
        let config = self.config().clone();
        let kdl_content = crate::config::get_config_dir()
            .ok()
            .map(|dir| dir.join("config.kdl"))
            .and_then(|path| std::fs::read_to_string(path).ok());
        self.view =
            ActiveView::ConfigEditor(config_editor::ConfigEditorState::new(&config, kdl_content));
    }

    /// Start the deploy lateral view.
    ///
    /// If there's work to do (deploy signals present), loads full deploy data
    /// and shows the preview. Otherwise shows the "up to date" modal with
    /// per-library file counts.
    pub(super) fn start_deploy_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::Deploy;
        let deploy_status = self.cached_deploy.clone();

        let needs_action = deploy_status.as_ref().is_some_and(|s| s.needs_action);

        if needs_action {
            let data = self
                .cache
                .domain_query(crate::db::domain::GetDeployData {
                    config: crate::config::load_config().ok(),
                })
                .recv();
            let preview = deploy_modal::DeploymentPreviewState::new(data);
            self.view =
                ActiveView::Deploy(deploy_modal::DeployViewState::Preview(Box::new(preview)));
        } else {
            let counts = deploy_status
                .map(|s| s.library_file_counts)
                .unwrap_or_default();
            self.view = ActiveView::Deploy(deploy_modal::DeployViewState::UpToDate {
                library_file_counts: counts,
            });
        }
    }

    pub(super) fn start_corpus_browser(&mut self) {
        self.last_lateral_view = widgets::LateralView::Files;
        let variant_config = tree_browser::CorpusBrowserConfig::default();
        let config = self.config();
        let archive_root = config.root.clone();
        let corpus_dir = config.corpus_dir();
        let deploy_source_paths: Vec<std::path::PathBuf> = config
            .source_dirs
            .iter()
            .map(|sd| corpus_dir.join(&sd.path))
            .collect();
        let primary_zone_paths = vec![corpus_dir.clone(), config.inbox_dir(), config.stash_dir()];
        drop(config);
        self.view = ActiveView::CorpusBrowser(tree_browser::TreeBrowserState::corpus_browser(
            archive_root,
            variant_config,
            deploy_source_paths,
            corpus_dir,
            primary_zone_paths,
        ));
    }

    // =========================================================================
    // Startup Flow
    // =========================================================================

    /// Complete startup: spawn db_thread, start FS watcher, open persistent txn,
    /// and transition to the Progress screen.
    pub(super) fn complete_startup(&mut self) {
        let shared = self.shared_config.clone();
        self.witch.set_shared_config(shared);
        self.witch.start_watching();

        // If leave_transactions_open is enabled, open a persistent transaction at startup
        if self.open_txn_mode() {
            let _ = self.witch.start_transaction("Open");
        }

        self.view = ActiveView::Progress {
            screen: progress_screen::ProgressScreen::new_eyeballing(),
            eye: eye::Eye::default(),
        };
    }

    /// After migrations complete, check if vacuum is needed, otherwise complete startup.
    pub(super) fn advance_past_migrations(
        &mut self,
        db_path: &std::path::Path,
        vacuum_threshold: f64,
    ) {
        if let Some(prompt_state) = Self::check_vacuum_needed(db_path, vacuum_threshold) {
            self.view = ActiveView::VacuumPrompt(prompt_state);
        } else {
            self.complete_startup();
        }
    }

    /// Check if the database needs vacuuming. Returns Some(state) if so.
    fn check_vacuum_needed(db_path: &std::path::Path, threshold: f64) -> Option<VacuumPromptState> {
        if threshold <= 0.0 || !db_path.exists() {
            return None;
        }

        let conn = rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok()?;

        let page_count: u64 = conn
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .ok()?;
        let freelist_count: u64 = conn
            .pragma_query_value(None, "freelist_count", |row| row.get(0))
            .ok()?;
        let page_size: u64 = conn
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .ok()?;

        drop(conn);

        if page_count == 0 {
            return None;
        }

        let ratio = freelist_count as f64 / page_count as f64;
        if ratio <= threshold {
            return None;
        }

        let pct = (ratio * 100.0).round() as u64;
        let free_bytes = freelist_count * page_size;
        let free_mb = free_bytes as f64 / (1024.0 * 1024.0);

        Some(VacuumPromptState {
            pct,
            free_mb,
            db_path: db_path.to_path_buf(),
            phase: VacuumPhase::Prompt,
        })
    }
}

// ============================================================================
// Rendering
// ============================================================================

fn render(f: &mut Frame, app: &mut App) {
    // Build status bar lines
    let status_line_1 = if app.witch.idle_rescan_active() {
        Some("Refreshing corpus...".to_string())
    } else if let Some(ref msg) = app.status_message {
        Some(msg.clone())
    } else {
        app.view.selected_path().map(|s| s.to_string())
    };

    let status_line_2 = app.witch.transaction_summary().map(|(label, dec, mut_)| {
        let pd = if dec == 1 { "" } else { "s" };
        let pm = if mut_ == 1 { "" } else { "s" };
        format!("Transaction \"{label}\": {dec} decision{pd}, {mut_} mutation{pm} staged")
    });

    render::render_app(f, app, status_line_1, status_line_2);
}

// ============================================================================
// Entry Point
// ============================================================================

pub fn run_menu(
    config: Config,
    log_rx: std::sync::mpsc::Receiver<crate::logging::LogOp>,
) -> Result<()> {
    crate::logging::log_general("=== MM startup ===");

    // Init the image picker (env-based detection, no stdin probing).
    let art_picker = widgets::AlbumArtPicker::init();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let db_path = crate::config::get_db_path()?;
    let initial_state = crate::witch::InitialUiState::determine(&db_path);

    if initial_state == crate::witch::InitialUiState::FirstTimeSetup {
        startup::handle_first_time_setup(&mut terminal, &db_path)?;
    }

    let force_check = config.opinions.startup.force_check_all_files_at_startup;
    let vacuum_threshold = config.opinions.startup.vacuum_threshold;
    let shared_config = config.into_shared();
    let (witch, cache_handle, notice_rx) = {
        let cfg = crate::config::read_shared_config(&shared_config);
        crate::witch::Witch::with_opinions(&cfg, false, force_check, Some(log_rx))
    };

    let mut app = App::new_with_witch(shared_config, witch, cache_handle, notice_rx, art_picker);
    app.vacuum_threshold = vacuum_threshold;
    app.db_path = db_path.clone();
    // Determine initial view based on startup state
    if app.witch.needs_schema_update() {
        let descriptions = app.witch.pending_schema_descriptions();
        app.view = ActiveView::SchemaUpdate(SchemaUpdateState {
            descriptions,
            phase: SchemaUpdatePhase::Approval,
        });
    } else {
        app.advance_past_migrations(&db_path, vacuum_threshold);
    }

    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error: {:?}", err);
    }

    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> io::Result<()> {
    let signal_received = Arc::new(AtomicBool::new(false));

    if let Err(e) = register_signal_handlers(Arc::clone(&signal_received)) {
        crate::logging::log_error(format!("Failed to register signal handlers: {}", e));
    }

    loop {
        if signal_received.swap(false, Ordering::SeqCst) {
            app.handle_input(InputAction::Cancel);
        }

        // Tick startup views first (they have their own Witch tick calls)
        if matches!(app.view, ActiveView::SchemaUpdate(_)) {
            app.tick_schema_update();
            // If tick changed the view away from SchemaUpdate, skip the rest of
            // this frame to let the new view render first.
            if !matches!(app.view, ActiveView::SchemaUpdate(_)) {
                terminal.draw(|f| render(f, app))?;
                continue;
            }
        }
        if matches!(app.view, ActiveView::VacuumPrompt(_)) {
            app.tick_vacuum_prompt();
            if !matches!(app.view, ActiveView::VacuumPrompt(_)) {
                terminal.draw(|f| render(f, app))?;
                continue;
            }
        }

        // Startup views don't interact with the normal Witch tick / idle rescan
        let is_startup_view = matches!(
            app.view,
            ActiveView::SchemaUpdate(_) | ActiveView::VacuumPrompt(_)
        );

        // Set idle rescan eligibility based on current view (lateral views only)
        let idle_eligible = matches!(
            app.view,
            ActiveView::Insights(_)
                | ActiveView::CorpusBrowser(_)
                | ActiveView::TagSearch(_)
                | ActiveView::History(_)
                | ActiveView::Inbox(_)
                | ActiveView::TabbedTransactionReview(_)
        );
        app.witch.set_idle_rescan_eligible(idle_eligible);

        // Tick the Witch (skip during startup views — they tick internally as needed)
        if !is_startup_view {
            app.witch.tick();
        }

        // Drain WitchNotices → update local state
        while let Ok(notice) = app.notice_rx.try_recv() {
            match notice {
                crate::witch::WitchNotice::StatusUpdate(status) => {
                    app.cached_status = status;
                }
                crate::witch::WitchNotice::MutationsCompleted => {
                    app.cache.invalidate_all();
                    app.cache_stale = true;
                }
                crate::witch::WitchNotice::Error(msg) => {
                    app.status_message = Some(format!("Task failed: {}", msg));
                }
                crate::witch::WitchNotice::SafetyLatch(reason) => {
                    app.status_message = Some(format!("Safety latch: {}", reason));
                }
                crate::witch::WitchNotice::ConfigUpdated => {
                    // Config already updated on Witch side via shared_config
                }
            }
        }

        // Drain CacheReady results from cache thread → update local cached data
        let ready_items = app.cache.drain_ready();
        if !ready_items.is_empty() {
            app.cache_stale = false;
        }
        for item in ready_items {
            match item {
                crate::witch::cache_thread::CacheReady::Insights(data) => {
                    app.cached_insights = Some(data);
                }
                crate::witch::cache_thread::CacheReady::InboxOverview(data) => {
                    app.cached_inbox = Some(data);
                }
                crate::witch::cache_thread::CacheReady::DeployStatus(data) => {
                    app.cached_deploy = Some(data);
                }
                crate::witch::cache_thread::CacheReady::EditHistory(data) => {
                    app.cached_history = Some(data);
                }
                crate::witch::cache_thread::CacheReady::ExternalMatches(data) => {
                    app.cached_external_matches = Some(data);
                }
                crate::witch::cache_thread::CacheReady::PackingDirs(data) => {
                    // Update navigator's cached data and refresh markers on all entries
                    if let ActiveView::CorpusBrowser(ref mut browser) = app.view {
                        browser
                            .navigator
                            .set_packing_data(&data.file_paths, &data.dir_categories);
                    }
                    app.cached_packing_dirs = Some(data);
                }
            }
        }

        // Update views with cached data
        if let ActiveView::Insights(ref mut view) = app.view {
            let insights_data = app.cached_insights.clone();
            let handled = app.witch.handled_decision_kinds();
            view.update(
                Some(&app.cached_status),
                insights_data,
                handled,
                app.cache_stale,
            );
        }
        if let ActiveView::Inbox(ref mut view) = app.view {
            let inbox_data = app.cached_inbox.clone();
            view.update(inbox_data);
            view.busy = app.cached_status.pending > 0
                || app.cached_status.idle_rescan_active
                || app.cache_stale;
        }
        if let ActiveView::History(ref mut view) = app.view {
            view.update(app.cached_history.clone());
        }
        if let ActiveView::ExternalMatches(ref mut view) = app.view {
            if let Some(ref data) = app.cached_external_matches {
                view.update(data.clone());
            }
            let new_fetch_active = app.witch.is_external_fetch_active();
            let fetch_changed = view.fetch_active != new_fetch_active;
            view.fetch_active = new_fetch_active;
            view.fetch_progress = app.witch.external_fetch_progress().cloned();
            if view.fetch_active {
                view.tick_count = view.tick_count.wrapping_add(1);
            }
            if fetch_changed {
                view.rebuild_items();
            }
        }
        if let ActiveView::Deploy(deploy_modal::DeployViewState::UpToDate {
            ref mut library_file_counts,
        }) = app.view
        {
            if let Some(ref status) = app.cached_deploy {
                *library_file_counts = status.library_file_counts.clone();
            }
        }

        // Tick view-specific state machines
        if matches!(app.view, ActiveView::Progress { .. }) {
            app.tick_progress_screen();
        }
        if matches!(app.view, ActiveView::ProgressiveWork(_)) {
            app.tick_progressive_worker();
        }
        if matches!(app.view, ActiveView::TagCanonicityLoading { .. }) {
            app.tick_tag_canonicity_loading();
        }

        // Flag demand for cached UI data
        // Always want deploy status — titlebar needs it for purple indicator
        app.cache.want_deploy();
        if matches!(app.view, ActiveView::Insights(_)) {
            app.cache.want_insights();
        }
        if matches!(app.view, ActiveView::Inbox(_)) {
            app.cache.want_inbox();
        }
        if matches!(app.view, ActiveView::History(_)) {
            app.cache.want_history();
        }
        if matches!(app.view, ActiveView::CorpusBrowser(_)) {
            app.cache.want_packing_dirs();
        }
        if let ActiveView::ExternalMatches(ref view) = app.view {
            if view.fetch_active {
                app.cache.want_external_matches_urgent();
            } else {
                app.cache.want_external_matches();
            }
        }

        terminal.draw(|f| render(f, app))?;

        // Tick tag search for pending bulk edit (after modal has rendered)
        app.tick_tag_search();

        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.code == crossterm::event::KeyCode::Char('c')
                        && key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL)
                    {
                        app.handle_input(InputAction::Cancel);
                    } else {
                        app.handle_input(input::map_key(key));
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        app.handle_input(InputAction::NavUp);
                    }
                    MouseEventKind::ScrollDown => {
                        app.handle_input(InputAction::NavDown);
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        app.handle_click(mouse.column, mouse.row);
                    }
                    _ => {}
                },
                Event::Paste(text) => {
                    app.handle_input(InputAction::Paste(text));
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

fn register_signal_handlers(flag: Arc<AtomicBool>) -> Result<(), Box<dyn std::error::Error>> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::flag;

    flag::register(SIGINT, Arc::clone(&flag))?;
    flag::register(SIGTERM, flag)?;

    Ok(())
}
