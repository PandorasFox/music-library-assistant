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
pub(crate) mod packing_colors;
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
    ActiveView, CanonicitySignalKind, ExitConfirmAction, ExitConfirmModalState,
    SuspendedView, TagCanonicityClusters, ViewAction,
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
use std::time::{Duration, Instant};

use mm_meta::config::Config;
use mm_meta::paths::PathResolver;
use mm_meta::witch_types::WitchStatus;

// ============================================================================
// Application State
// ============================================================================

/// Main application state
pub(crate) struct App {
    should_quit: bool,
    pub(crate) status_message: Option<String>,

    /// The active view and its state. One variant is active at a time.
    pub(crate) view: ActiveView,

    // Handle to the Witch — She owns the main thread, we command via channels.
    // Also owns cache channels and locally cached periodic data.
    pub(crate) witch: mm_meta::witch_handle::WitchHandle,

    /// Client-side path resolver for root-relative ↔ absolute path conversion.
    /// Constructed from config after login.
    pub(crate) resolver: PathResolver,

    // View stack for push/pop navigation (TransactionReview, ProgressiveWork, etc.)
    pub(crate) view_stack: Vec<SuspendedView>,

    /// Last lateral view the user was on. Used for returning after modal flows.
    pub(crate) last_lateral_view: widgets::LateralView,

    /// Whether startup maintenance (schema reconciliation, vacuum) has completed.
    /// Set to true once WitchStartupState transitions to Ready.
    startup_complete: bool,

    /// Generation counters from last frame — used to detect events by diffing
    /// against the current WitchStatus each frame.
    prev_mutations_generation: u64,
    prev_error_generation: u64,
    prev_config_generation: u64,

    /// Cached WitchStatus — refreshed once per loop iteration (1s TTL).
    cached_status: WitchStatus,
    cached_status_at: Instant,

    /// Terminal image rendering: picker for protocol detection + image cache.
    pub(crate) art_picker: widgets::AlbumArtPicker,
    pub(crate) art_cache: widgets::AlbumArtCache,

    /// Click targets for titlebar tabs, populated during render.
    pub(crate) tab_click_rects: Vec<(widgets::LateralView, ratatui::layout::Rect)>,

}

impl App {
    /// Create a new App with a Witch handle and config.
    fn new(
        mut witch: mm_meta::witch_handle::WitchHandle,
        art_picker: widgets::AlbumArtPicker,
    ) -> Self {
        let cached_status = witch.witch_status();
        let config = witch.config();
        let resolver = PathResolver::from_config(&config);
        Self {
            should_quit: false,
            status_message: None,
            view: ActiveView::Insights(insights_view::InsightsViewState::new()),
            witch,
            resolver,
            view_stack: Vec::new(),
            last_lateral_view: widgets::LateralView::Health,
            startup_complete: false,
            prev_mutations_generation: 0,
            prev_error_generation: 0,
            prev_config_generation: 0,
            cached_status,
            cached_status_at: Instant::now(),
            art_picker,
            art_cache: widgets::AlbumArtCache::new(),
            tab_click_rects: Vec::new(),
        }
    }

    const STATUS_TTL: Duration = Duration::from_secs(1);

    /// Get a snapshot of the current config from the Witch (cached in handle).
    pub(crate) fn config(&mut self) -> Config {
        self.witch.config()
    }

    /// Return a reference to the cached WitchStatus (refreshed once per loop iteration).
    pub(crate) fn witch_status(&self) -> &WitchStatus {
        &self.cached_status
    }

    /// Refresh the cached WitchStatus if the TTL has elapsed.
    fn refresh_status(&mut self) {
        if self.cached_status_at.elapsed() >= Self::STATUS_TTL {
            self.cached_status = self.witch.witch_status();
            self.cached_status_at = Instant::now();
        }
    }

    /// Whether the Transaction tab should be visible in the lateral view ring.
    fn transactions_open(&mut self) -> bool {
        self.config().opinions.leave_transactions_open
    }

    /// Whether leave-transactions-open mode is active (alias for readability in control flow).
    fn open_txn_mode(&mut self) -> bool {
        self.config().opinions.leave_transactions_open
    }

    /// Return to the last lateral view the user was on.
    fn return_to_last_lateral_view(&mut self) {
        self.start_lateral_view(self.last_lateral_view);
    }

    fn handle_input(&mut self, action: InputAction) {
        // Pre-dispatch: intercept CycleNext/CyclePrev for lateral views that
        // don't need to handle them as domain actions (7 of 9 views).
        if let Some(lv) = self.view.lateral_view() {
            if !self.view.wants_raw_cycle() {
                match action {
                    InputAction::CycleNext => { self.handle_lateral_cycle(lv, true); return; }
                    InputAction::CyclePrev => { self.handle_lateral_cycle(lv, false); return; }
                    _ => {}
                }
            }
        }

        // Macro for views whose handle_input returns Option<DomainAction>:
        // None → ViewAction::None, Some(a) → ViewAction::$variant(a)
        macro_rules! dispatch_input {
            ($variant:ident, $state:expr) => {
                match $state.handle_input(&action) {
                    Some(a) => ViewAction::$variant(a),
                    None => ViewAction::None,
                }
            };
        }

        // Macro for views whose handle_input still returns a concrete action type
        // (ConfigEditor, CorpusBrowser — they keep CycleNext/CyclePrev variants)
        macro_rules! dispatch_input_raw {
            ($variant:ident, $state:expr) => {
                ViewAction::$variant($state.handle_input(&action))
            };
        }

        // Phase 1: borrow view, produce view action
        let view_action = match &mut self.view {
            ActiveView::StartupMaintenance => ViewAction::None,
            ActiveView::Progress { .. } => ViewAction::None,
            ActiveView::ProgressiveWork(_) => ViewAction::None,
            ActiveView::ConfigEditor(s) => dispatch_input!(ConfigEditor, s),
            ActiveView::Insights(s) => dispatch_input!(Insights, s),
            ActiveView::CorpusBrowser(s) => dispatch_input!(CorpusBrowser, s),
            ActiveView::TagSearch(s) => dispatch_input!(TagSearch, s),
            ActiveView::Inbox(s) => dispatch_input!(Inbox, s),
            ActiveView::TabbedTransactionReview(ref mut s) => dispatch_input!(TabbedTransactionReview, s),
            ActiveView::ExitConfirm(state) => {
                let a = match action {
                    InputAction::NavLeft | InputAction::FocusLeft => {
                        state.selected = state.selected.saturating_sub(1);
                        ExitConfirmAction::None
                    }
                    InputAction::NavRight | InputAction::FocusRight => {
                        state.selected = (state.selected + 1).min(2);
                        ExitConfirmAction::None
                    }
                    InputAction::NavDown => {
                        // Jump to shutdown option
                        state.selected = 2;
                        ExitConfirmAction::None
                    }
                    InputAction::NavUp => {
                        // Jump back to top row, preserving left/right
                        if state.selected == 2 {
                            state.selected = 0;
                        }
                        ExitConfirmAction::None
                    }
                    InputAction::Confirm | InputAction::Toggle => {
                        match state.selected {
                            0 => ExitConfirmAction::Quit,
                            2 => ExitConfirmAction::QuitAndShutdown,
                            _ => ExitConfirmAction::Cancel,
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
                ViewAction::IntakeConfirmation(startup::intake_confirmation::handle_input(state, &action, visible_height))
            }
            ActiveView::UnifiedTagEditor(s) => dispatch_input_raw!(UnifiedTagEditor, s),
            ActiveView::Deploy(s) => dispatch_input!(Deploy, s),
            ActiveView::ExternalMatches(s) => dispatch_input!(ExternalMatches, s),
            ActiveView::MissingFileResolution(s) => dispatch_input_raw!(MissingFileResolution, s),
            ActiveView::MissingDirectoryResolution(s) => dispatch_input_raw!(MissingDirectoryResolution, s),
            ActiveView::CorruptFileResolution(s) => dispatch_input_raw!(CorruptFileResolution, s),
            ActiveView::ShitFormatResolution(s) => dispatch_input_raw!(ShitFormatResolution, s),
            ActiveView::SubparDuplicateResolution(s) => dispatch_input_raw!(SubparDuplicateResolution, s),
            ActiveView::InboxCorpusMatchResolution(s) => dispatch_input_raw!(InboxCorpusMatchResolution, s),
            ActiveView::InboxOrganize(s) => dispatch_input_raw!(InboxOrganize, s),
            ActiveView::DirectoryClusterResolution(s) => dispatch_input_raw!(DirectoryClusterResolution, s),
            ActiveView::MovedFileAcknowledge(s) => dispatch_input_raw!(MovedFileAcknowledge, s),
            ActiveView::OobSyncResolution(s) => dispatch_input_raw!(OobSyncResolution, s),
            ActiveView::OobConflictInspection(s) => dispatch_input_raw!(OobConflictInspection, s),
            ActiveView::ExternalMatchReview(s) => dispatch_input_raw!(ExternalMatchReview, s),
            ActiveView::ReleasePackingBrowser(s) => dispatch_input_raw!(ReleasePackingBrowser, s),
            ActiveView::KnotBrowser(s) => dispatch_input_raw!(KnotBrowser, s),
            ActiveView::History(s) => dispatch_input!(History, s),
            ActiveView::TagCanonicityResolution { state, .. } => dispatch_input_raw!(TagCanonicityResolution, state),
            ActiveView::CompoundTagSplit { state, .. } => dispatch_input_raw!(CompoundTagSplit, state),
            ActiveView::MissingAlbumSingleResolution(s) => dispatch_input_raw!(MissingAlbumSingleResolution, s),
            ActiveView::DiscExtractionResolution(s) => dispatch_input_raw!(DiscExtractionResolution, s),
            ActiveView::ManualReview(s) => dispatch_input_raw!(ManualReview, s),
            ActiveView::TransactionReview(s) => dispatch_input_raw!(TransactionReview, s),
        };

        // Post-dispatch: if no view produced a domain action and the input was Cancel,
        // treat it as quit for lateral views. Views that handle Cancel as a domain
        // action (ConfigEditor→Discard, CorpusBrowser→Cancel, TagSearch→Cancel)
        // produce Some(action), so the fallback never fires for them.
        if matches!(&view_action, ViewAction::None) && matches!(action, InputAction::Cancel)
            && self.view.lateral_view().is_some()
        {
            self.handle_request_quit();
            return;
        }

        // Phase 2: dispatch with confirmation flag
        let is_confirmation = matches!(action, InputAction::Confirm);
        self.dispatch_action(view_action, is_confirmation);
    }

    /// Check if there are any pending operations (Witch work).
    pub(crate) fn has_pending_operations(&self) -> bool {
        self.witch_status().has_pending
    }

    /// Start the health view.
    pub(crate) fn start_health_view(&mut self) {
        self.clear_view_stack();
        self.last_lateral_view = widgets::LateralView::Health;
        self.view = ActiveView::Insights(insights_view::InsightsViewState::new());
    }

    /// Start the configured default view (post-startup landing screen).
    pub(crate) fn start_default_view(&mut self) {
        let config = self.config();
        let default_view = config.opinions.startup.default_view;
        match default_view {
            mm_meta::config::StartupView::Health => self.start_health_view(),
            mm_meta::config::StartupView::Search => self.start_tag_search(),
            mm_meta::config::StartupView::Browser => self.start_corpus_browser(),
            mm_meta::config::StartupView::Inbox => self.start_inbox_view(),
            mm_meta::config::StartupView::ExternalMatches => self.start_external_matches_view(),
        }
    }

    /// Abort current operation and return to health view with a status message.
    pub(crate) fn abort_to_health(&mut self, message: String) {
        self.status_message = Some(message);
        self.start_health_view();
    }

    /// Update Witch stats on a progress state that implements the stats setter methods.
    pub(crate) fn update_progress_stats<T>(&self, state: &mut T)
    where
        T: ProgressStatsUpdater,
    {
        state.set_db_queue_depth(self.witch_status().db_queue_depth);
    }

    pub(crate) fn start_tag_search(&mut self) {
        self.last_lateral_view = widgets::LateralView::Search;
        self.view = ActiveView::TagSearch(tag_search::TagSearchState::new());
    }

    pub(crate) fn start_inbox_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::Inbox;
        // Check for inbox unindexed files — show intake popup if any
        let intake_state = self
            .witch
            .query(mm_meta::domain_queries::GetIntakeConfirmation {
                source: startup::IntakeSource::Inbox,
                zone: Some(mm_meta::db_types::Zone::Inbox),
            });

        if let Some(state) = intake_state {
            self.view = ActiveView::IntakeConfirmation(state);
        } else {
            self.view = ActiveView::Inbox(inbox_view::InboxViewState::new());
        }
    }

    /// Start the history lateral view.
    pub(crate) fn start_history_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::History;
        self.view = ActiveView::History(history_view::HistoryViewState::new());
    }

    /// Start the external matches lateral view.
    pub(crate) fn start_external_matches_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::ExternalMatches;
        let ws = self.witch_status();
        let fetch_active = ws.is_external_fetch_active;
        let has_api_key = ws.has_acoustid_api_key;
        let singles_before_incompletes = self
            .config()
            .opinions.release_packing.singles_before_incompletes;
        let mut state = external_match_view::ExternalMatchesViewState::new(
            fetch_active,
            has_api_key,
            singles_before_incompletes,
        );
        let data = self.witch.query(mm_meta::domain_queries::GetExternalMatches);
        state.update(data);
        self.view = ActiveView::ExternalMatches(state);
    }

    /// Start the lateral view identified by the given variant.
    pub(crate) fn start_lateral_view(&mut self, view: widgets::LateralView) {
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
    pub(crate) fn start_tabbed_transaction_review(&mut self) {
        self.last_lateral_view = widgets::LateralView::Transaction;
        let mut state = tabbed_transaction_review::TabbedTransactionReviewState::new();
        state.review.refresh_decisions(&self.witch);
        self.view = ActiveView::TabbedTransactionReview(state);
    }

    /// Start the config editor view.
    pub(crate) fn start_config_editor(&mut self) {
        self.last_lateral_view = widgets::LateralView::Config;
        let config = self.config();
        let kdl_content = mm_utils::get_config_dir()
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
    pub(crate) fn start_deploy_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::Deploy;
        let deploy_status = self.witch.query(mm_meta::domain_queries::GetDeployStatus);

        if deploy_status.needs_action {
            let config = self.config();
            let data = self
                .witch
                .query(mm_meta::domain_queries::GetDeployData {
                    config: Some(config),
                });
            let preview = deploy_modal::DeploymentPreviewState::new(data);
            self.view =
                ActiveView::Deploy(deploy_modal::DeployViewState::Preview(Box::new(preview)));
        } else {
            self.view = ActiveView::Deploy(deploy_modal::DeployViewState::UpToDate {
                library_file_counts: deploy_status.library_file_counts,
            });
        }
    }

    pub(crate) fn start_corpus_browser(&mut self) {
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

    /// Complete startup: inject config to Witch, open persistent txn,
    /// and transition to the appropriate view based on Witch state.
    pub(crate) fn complete_startup(&mut self) {
        let config = self.config();
        let _ = self.witch.config_op(mm_meta::protocol::ConfigOp::SetShared(config));

        // If leave_transactions_open is enabled, open a persistent transaction at startup
        if self.open_txn_mode() {
            let _ = self.witch.start_transaction("Open");
        }

        // Pick the right view based on current Witch state:
        // - Full reasoning + idle → go straight to the default view
        // - Full reasoning + busy → show content analysis progress
        // - Not yet Full → show eyeballing progress
        let status = self.witch.witch_status();
        match status.reasoning_level {
            mm_meta::witch_types::ReasoningLevel::Full if !status.has_pending => {
                self.start_default_view();
            }
            mm_meta::witch_types::ReasoningLevel::Full => {
                self.view = ActiveView::Progress {
                    screen: progress_screen::ProgressScreen::new_content_analysis(),
                    eye: eye::Eye::default(),
                };
            }
            _ => {
                self.view = ActiveView::Progress {
                    screen: progress_screen::ProgressScreen::new_eyeballing(),
                    eye: eye::Eye::default(),
                };
            }
        }
    }

}

// ============================================================================
// Rendering
// ============================================================================

fn render(f: &mut Frame, app: &mut App) {
    // Build status bar lines
    let status_line_1 = if let Some(ref msg) = app.status_message {
        Some(msg.clone())
    } else {
        app.view.selected_path().map(|s| s.to_string())
    };

    let status_line_2 = {
        app.witch_status().transaction.as_ref().map(|t| {
            let dec = t.decision_count;
            let mut_ = t.mutation_count;
            let pd = if dec == 1 { "" } else { "s" };
            let pm = if mut_ == 1 { "" } else { "s" };
            format!("Transaction \"{}\": {} decision{}, {} mutation{} staged", t.label, dec, pd, mut_, pm)
        })
    };

    render::render_app(f, app, status_line_1, status_line_2);
}

// ============================================================================
// Entry Point
// ============================================================================

pub fn run_tui(
    mut witch: mm_meta::witch_handle::WitchHandle,
) -> Result<()> {
    mm_meta::logging::log_general("=== MM TUI startup ===");

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

    // Check startup state from the Witch (unauthenticated — before login)
    if witch.needs_setup() {
        let has_config = mm_utils::get_config_dir()
            .map(|dir| dir.join("config.kdl").exists())
            .unwrap_or(false);

        let root = if has_config {
            // Config exists but DB was deleted — show DB setup dialog, use existing root
            let db_path = mm_utils::get_db_path()?;
            startup::handle_db_setup_dialog(&mut terminal, &db_path)?;
            // Ask the Witch for the config root (she parsed it at startup)
            let cfg = witch.config();
            cfg.root
        } else {
            // Fresh install — directory picker
            startup::run_directory_picker(&mut terminal)?
        };

        // Collect first-user credentials
        let first_user = startup::first_time_setup::run_create_account(&mut terminal)?;

        witch
            .complete_setup(root, Some(first_user))
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        // Notify auth thread that DB is now available
        witch.notify_db_ready();
    }

    // Auth: login to obtain session token
    {
        startup::login::run_login_screen(&mut terminal, &mut witch)?;
    }

    // Config + DB now guaranteed. App fetches config via protocol.
    let mut app = App::new(witch, art_picker);

    // Check if the Witch is already Ready (no startup maintenance needed)
    // or if she's running maintenance (Reconciling/Vacuuming)
    {
        let status = app.witch.witch_status();
        if status.startup_state == mm_meta::witch_types::WitchStartupState::Ready {
            app.startup_complete = true;
            app.complete_startup();
        } else {
            // Witch is in Reconciling or Vacuuming — show maintenance view
            app.view = ActiveView::StartupMaintenance;
        }
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
        mm_meta::logging::log_error(format!("Failed to register signal handlers: {}", e));
    }

    loop {
        app.refresh_status();

        if signal_received.swap(false, Ordering::SeqCst) {
            app.handle_input(InputAction::Cancel);
        }

        // Observe startup maintenance completion (Witch auto-runs reconciliation/vacuum)
        if !app.startup_complete {
            let state = app.witch_status().startup_state;
            if state == mm_meta::witch_types::WitchStartupState::Ready {
                app.startup_complete = true;
                app.complete_startup();
                terminal.draw(|f| render(f, app))?;
                continue;
            }
        }

        // (Witch ticks herself on the main thread — no manual tick needed)

        // Detect events via generation counter diffing against WitchStatus
        {
            let status = &app.cached_status;

            // Mutations completed
            if status.mutations_generation != app.prev_mutations_generation {
                app.prev_mutations_generation = status.mutations_generation;
            }

            // New error → show status message
            if status.error_generation != app.prev_error_generation {
                app.prev_error_generation = status.error_generation;
                if let Some(ref err) = status.last_error {
                    app.status_message = Some(format!("Task failed: {}", err));
                }
            }

            // Config updated — invalidate handle cache so next config() re-fetches
            if status.config_generation != app.prev_config_generation {
                app.prev_config_generation = status.config_generation;
                app.witch.invalidate_config_cache();
                // Rebuild path resolver with new root
                let new_config = app.witch.config();
                app.resolver = PathResolver::from_config(&new_config);
            }
        }

        // Update views with query data
        if let ActiveView::Insights(ref mut view) = app.view {
            let insights_data = app.witch.query(mm_meta::domain_queries::GetInsights);
            view.update(
                Some(&app.cached_status.work),
                Some(insights_data),
                &app.cached_status.handled_decision_kinds,
            );
        }
        if let ActiveView::Inbox(ref mut view) = app.view {
            let inbox_data = app.witch.query(mm_meta::domain_queries::GetInboxOverview);
            view.busy = app.cached_status.work.pending > 0;
            view.update(Some(inbox_data));
        }
        if let ActiveView::History(ref mut view) = app.view {
            view.update(Some(app.witch.query(mm_meta::domain_queries::GetEditHistory)));
        }
        if let ActiveView::ExternalMatches(ref mut view) = app.view {
            let data = app.witch.query(mm_meta::domain_queries::GetExternalMatches);
            view.update(data);
            let new_fetch_active = app.cached_status.is_external_fetch_active;
            let fetch_changed = view.fetch_active != new_fetch_active;
            view.fetch_active = new_fetch_active;
            view.fetch_progress = app.cached_status.external_fetch_progress.clone();
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
            let status = app.witch.query(mm_meta::domain_queries::GetDeployStatus);
            *library_file_counts = status.library_file_counts;
        }
        if let ActiveView::CorpusBrowser(ref mut browser) = app.view {
            let data = app.witch.query(mm_meta::domain_queries::GetPackingDirs);
            browser
                .navigator
                .set_packing_data(&data.file_paths, &data.dir_categories);
        }

        // Tick view-specific state machines
        if matches!(app.view, ActiveView::Progress { .. }) {
            app.tick_progress_screen();
        }
        if matches!(app.view, ActiveView::ProgressiveWork(_)) {
            app.tick_progressive_worker();
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
