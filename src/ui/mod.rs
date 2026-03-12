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
use std::any::TypeId;
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

    // Handle to the Witch — She owns the main thread, we command via channels.
    // Also owns cache channels and locally cached periodic data.
    pub(super) witch: crate::witch::WitchHandle,

    // View stack for push/pop navigation (TransactionReview, ProgressiveWork, etc.)
    pub(super) view_stack: Vec<SuspendedView>,

    /// Last lateral view the user was on. Used for returning after modal flows.
    pub(super) last_lateral_view: widgets::LateralView,

    /// Whether startup maintenance (schema reconciliation, vacuum) has completed.
    /// Set to true once WitchStartupState transitions to Ready.
    startup_complete: bool,

    /// Generation counters from last frame — used to detect events by diffing
    /// against the current WitchStatus each frame.
    prev_mutations_generation: u64,
    prev_error_generation: u64,
    prev_config_generation: u64,

    /// Terminal image rendering: picker for protocol detection + image cache.
    pub(super) art_picker: widgets::AlbumArtPicker,
    pub(super) art_cache: widgets::AlbumArtCache,

    /// Click targets for titlebar tabs, populated during render.
    pub(super) tab_click_rects: Vec<(widgets::LateralView, ratatui::layout::Rect)>,

}

impl App {
    /// Create a new App with a Witch handle and shared config.
    fn new(
        shared_config: SharedConfig,
        witch: crate::witch::WitchHandle,
        art_picker: widgets::AlbumArtPicker,
    ) -> Self {
        Self {
            shared_config,
            should_quit: false,
            status_message: None,
            view: ActiveView::Insights(insights_view::InsightsViewState::new()),
            witch,
            view_stack: Vec::new(),
            last_lateral_view: widgets::LateralView::Health,
            startup_complete: false,
            prev_mutations_generation: 0,
            prev_error_generation: 0,
            prev_config_generation: 0,
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
        // Macro for the common pattern: delegate handle_input, wrap in ViewAction
        macro_rules! dispatch_input {
            ($variant:ident, $state:expr) => {
                ViewAction::$variant($state.handle_input(&action))
            };
        }

        // Phase 1: borrow view, produce view action
        let view_action = match &mut self.view {
            ActiveView::StartupMaintenance => ViewAction::None,
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
        self.witch.witch_status().has_pending
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
        state.set_db_queue_depth(self.witch.witch_status().db_queue_depth);
    }

    pub(super) fn start_tag_search(&mut self) {
        self.last_lateral_view = widgets::LateralView::Search;
        self.view = ActiveView::TagSearch(tag_search::TagSearchState::new());
    }

    pub(super) fn start_inbox_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::Inbox;
        // Check for inbox unindexed files — show intake popup if any
        let intake_state = self
            .witch
            .query(crate::db::domain::GetIntakeConfirmation {
                source: startup::IntakeSource::Inbox,
                zone: Some(crate::db::types::Zone::Inbox),
            });

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
        let ws = self.witch.witch_status();
        let fetch_active = ws.is_external_fetch_active;
        let has_api_key = ws.has_acoustid_api_key;
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
        if let Some(data) = self.witch.cached.get::<crate::db::domain::GetExternalMatches>() {
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
        let deploy_status = self.witch.cached.get::<crate::db::domain::GetDeployStatus>().cloned();

        let needs_action = deploy_status.as_ref().is_some_and(|s| s.needs_action);

        if needs_action {
            let data = self
                .witch
                .query(crate::db::domain::GetDeployData {
                    config: crate::config::load_config().ok(),
                });
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
        let _ = self.witch.set_shared_config(shared);
        let _ = self.witch.start_watching();

        // If leave_transactions_open is enabled, open a persistent transaction at startup
        if self.open_txn_mode() {
            let _ = self.witch.start_transaction("Open");
        }

        self.view = ActiveView::Progress {
            screen: progress_screen::ProgressScreen::new_eyeballing(),
            eye: eye::Eye::default(),
        };
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
        app.witch.witch_status().transaction.as_ref().map(|t| {
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
    mut witch: crate::witch::WitchHandle,
) -> Result<()> {
    crate::logging::log_general("=== MM TUI startup ===");

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

    // Check startup state from the Witch
    {
        let status = witch.witch_status();

        if status.startup_state == crate::witch::WitchStartupState::AwaitingSetup {
            let has_config = crate::config::config_exists();

            let root = if has_config {
                // Config exists but DB was deleted — show DB setup dialog, use existing root
                let db_path = crate::config::get_db_path()?;
                startup::handle_db_setup_dialog(&mut terminal, &db_path)?;
                let cfg = crate::config::load_config()?;
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
    }

    // Auth: login to obtain session token
    {
        startup::login::run_login_screen(&mut terminal, &mut witch)?;
    }

    // Config + DB now guaranteed. Load shared config for App.
    let config = crate::config::load_config()?;
    if let Err(e) = config.validate() {
        eprintln!("ERROR: Config validation failed\n");
        eprintln!("{:#}", e);
        std::process::exit(1);
    }
    let shared_config = config.into_shared();

    let mut app = App::new(shared_config, witch, art_picker);

    // Check if the Witch is already Ready (no startup maintenance needed)
    // or if she's running maintenance (Reconciling/Vacuuming)
    {
        let status = app.witch.witch_status();
        if status.startup_state == crate::witch::WitchStartupState::Ready {
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
        crate::logging::log_error(format!("Failed to register signal handlers: {}", e));
    }

    loop {
        if signal_received.swap(false, Ordering::SeqCst) {
            app.handle_input(InputAction::Cancel);
        }

        // Observe startup maintenance completion (Witch auto-runs reconciliation/vacuum)
        if !app.startup_complete {
            let state = app.witch.witch_status().startup_state;
            if state == crate::witch::WitchStartupState::Ready {
                app.startup_complete = true;
                app.witch.reconnect_cache_db();
                app.complete_startup();
                terminal.draw(|f| render(f, app))?;
                continue;
            }
        }

        // (Witch ticks herself on the main thread — no manual tick needed)

        // Detect events via generation counter diffing against WitchStatus
        {
            let status = app.witch.witch_status();

            // Mutations completed → cache invalidation
            if status.mutations_generation != app.prev_mutations_generation {
                app.prev_mutations_generation = status.mutations_generation;
                app.witch.set_cache_stale();
            }

            // New error → show status message
            if status.error_generation != app.prev_error_generation {
                app.prev_error_generation = status.error_generation;
                if let Some(ref err) = status.last_error {
                    app.status_message = Some(format!("Task failed: {}", err));
                }
            }

            // Config updated (already applied via shared_config on Witch side)
            if status.config_generation != app.prev_config_generation {
                app.prev_config_generation = status.config_generation;
            }
        }

        // Drain CacheReady results from cache thread → update local cached data
        let arrivals = app.witch.drain_subscriptions();
        if !arrivals.is_empty() {
            // Post-drain side effect: PackingDirs → tree browser markers
            if arrivals.contains(&TypeId::of::<crate::db::domain::GetPackingDirs>()) {
                if let Some(data) = app.witch.cached.get::<crate::db::domain::GetPackingDirs>() {
                    if let ActiveView::CorpusBrowser(ref mut browser) = app.view {
                        browser
                            .navigator
                            .set_packing_data(&data.file_paths, &data.dir_categories);
                    }
                }
            }
        }

        // Update views with cached data
        if let ActiveView::Insights(ref mut view) = app.view {
            let insights_data = app.witch.cached.get::<crate::db::domain::GetInsights>().cloned();
            let ws = app.witch.witch_status();
            view.update(
                Some(&ws.work),
                insights_data,
                &ws.handled_decision_kinds,
                app.witch.is_cache_stale(),
            );
        }
        if let ActiveView::Inbox(ref mut view) = app.view {
            let inbox_data = app.witch.cached.get::<crate::db::domain::GetInboxOverview>().cloned();
            view.update(inbox_data);
            let ws = app.witch.witch_status();
            view.busy = ws.work.pending > 0
                || app.witch.is_cache_stale();
        }
        if let ActiveView::History(ref mut view) = app.view {
            view.update(app.witch.cached.get::<crate::db::domain::GetEditHistory>().cloned());
        }
        if let ActiveView::ExternalMatches(ref mut view) = app.view {
            if let Some(data) = app.witch.cached.get::<crate::db::domain::GetExternalMatches>() {
                view.update(data.clone());
            }
            let ext_status = {
                let ws = app.witch.witch_status();
                (ws.is_external_fetch_active, ws.external_fetch_progress.clone())
            };
            let new_fetch_active = ext_status.0;
            let fetch_changed = view.fetch_active != new_fetch_active;
            view.fetch_active = new_fetch_active;
            view.fetch_progress = ext_status.1;
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
            if let Some(status) = app.witch.cached.get::<crate::db::domain::GetDeployStatus>() {
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
        app.witch.subscribe::<crate::db::domain::GetDeployStatus>();
        if matches!(app.view, ActiveView::Insights(_)) {
            app.witch.subscribe::<crate::db::domain::GetInsights>();
        }
        if matches!(app.view, ActiveView::Inbox(_)) {
            app.witch.subscribe::<crate::db::domain::GetInboxOverview>();
        }
        if matches!(app.view, ActiveView::History(_)) {
            app.witch.subscribe::<crate::db::domain::GetEditHistory>();
        }
        if matches!(app.view, ActiveView::CorpusBrowser(_)) {
            app.witch.subscribe::<crate::db::domain::GetPackingDirs>();
        }
        if let ActiveView::ExternalMatches(ref view) = app.view {
            if view.fetch_active {
                app.witch.subscribe_urgent::<crate::db::domain::GetExternalMatches>();
            } else {
                app.witch.subscribe::<crate::db::domain::GetExternalMatches>();
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
