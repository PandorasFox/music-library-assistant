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

pub(crate) mod active_view;
pub(crate) mod action_handlers;
mod suspended_views;
mod tag_editor_ops;
mod tick;
mod types;

pub mod operator_decisions;
pub mod transaction_review;
pub mod tabbed_transaction_review;

pub mod bulk_selection;
pub mod config_editor;
pub mod compound_split_v2;
pub mod corrupt_file_modal;
pub mod deploy_modal;
pub mod directory_cluster_modal;
pub mod embed_album_art_modal;
pub mod eye;
pub mod manual_review_modal;
pub mod filter_popup;
pub mod helpers;
pub mod inbox_corpus_match_modal;
pub mod inbox_organize;
pub mod inbox_view;
pub mod insights_view;
pub mod missing_album_modal;
pub mod missing_file_modal;
pub mod missing_directory_modal;
pub mod moved_file_modal;
pub mod oob_conflict_modal;
pub mod oob_sync_modal;
pub mod progress_screen;
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
pub mod progressive_worker;

// Re-export for convenience
pub(crate) use active_view::{
    ActiveView, CanonicitySignalKind, ExitConfirmModalState, ExitConfirmAction, FilterOverlay,
    FilterPopupContext, MigrationAction, MigrationApprovalState, MigrationPhase,
    SuspendedView, TagCanonicityClusters, VacuumAction, VacuumPhase, VacuumPromptState,
    ViewAction,
};
use types::ProgressStatsUpdater;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, MouseButton, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Frame, Terminal,
};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

    // Filter popup overlay (Ctrl+F in resolution modals and corpus browser)
    pub(super) filter_overlay: Option<FilterOverlay>,

    // View stack for push/pop navigation (TransactionReview, ProgressiveWork, etc.)
    pub(super) view_stack: Vec<SuspendedView>,

    /// Last lateral view the user was on. Used for returning after modal flows.
    pub(super) last_lateral_view: widgets::LateralView,

    /// Database path, stored for startup flow (vacuum prompt needs it).
    pub(super) db_path: std::path::PathBuf,

    /// Vacuum threshold from config, stored for startup flow.
    pub(super) vacuum_threshold: f64,
}

impl App {
    /// Create a new App with a pre-existing Witch instance and shared config.
    fn new_with_witch(shared_config: SharedConfig, witch: crate::witch::Witch) -> Self {
        Self {
            shared_config,
            should_quit: false,
            status_message: None,
            view: ActiveView::Insights(insights_view::InsightsViewState::new()),
            witch,
            filter_overlay: None,
            view_stack: Vec::new(),
            last_lateral_view: widgets::LateralView::Health,
            db_path: std::path::PathBuf::new(),
            vacuum_threshold: 0.0,
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

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        // Filter popup intercepts keys when active
        if let Some(ref mut overlay) = self.filter_overlay {
            let action = overlay.state.handle_key(key);
            match action {
                filter_popup::FilterPopupAction::None => return,
                filter_popup::FilterPopupAction::Apply => {
                    let condition = overlay.state.condition.clone();
                    let context = overlay.context;
                    self.filter_overlay = None;
                    match context {
                        FilterPopupContext::CorpusBrowser => {
                            if let ActiveView::CorpusBrowser(ref mut browser) = self.view {
                                let read_db = self.witch.read_db();
                                browser.apply_filter(condition, &read_db);
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

        // Phase 1: borrow view, produce action
        let action = match &mut self.view {
            ActiveView::MigrationApproval(s) => {
                let a = match (s.phase, key.code) {
                    (MigrationPhase::Approval, KeyCode::Enter) => MigrationAction::Approve,
                    (MigrationPhase::Approval, KeyCode::Esc) => MigrationAction::Cancel,
                    _ => MigrationAction::None,
                };
                ViewAction::MigrationApproval(a)
            }
            ActiveView::VacuumPrompt(s) => {
                let a = match (s.phase, key.code) {
                    (VacuumPhase::Prompt, KeyCode::Enter) => VacuumAction::Compact,
                    (VacuumPhase::Prompt, KeyCode::Esc) => VacuumAction::Skip,
                    _ => VacuumAction::None,
                };
                ViewAction::VacuumPrompt(a)
            }
            ActiveView::Progress { .. } => ViewAction::None,
            ActiveView::ProgressiveWork(_) => ViewAction::None,
            ActiveView::ConfigEditor(s) => ViewAction::ConfigEditor(s.handle_key(key)),
            ActiveView::Insights(s) => ViewAction::Insights(s.handle_key(key)),
            ActiveView::CorpusBrowser(s) => ViewAction::CorpusBrowser(s.handle_key(key)),
            ActiveView::TagSearch(s) => ViewAction::TagSearch(s.handle_key(key)),
            ActiveView::Inbox(s) => ViewAction::Inbox(s.handle_key(key)),
            ActiveView::TabbedTransactionReview(ref mut state) => {
                ViewAction::TabbedTransactionReview(state.handle_key(key))
            }
            ActiveView::ExitConfirm(state) => {
                let a = match key.code {
                    KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                        state.selected_no = !state.selected_no;
                        ExitConfirmAction::None
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        if state.selected_no { ExitConfirmAction::Cancel } else { ExitConfirmAction::Quit }
                    }
                    KeyCode::Esc => ExitConfirmAction::Cancel,
                    KeyCode::Char('y') | KeyCode::Char('Y') => ExitConfirmAction::Quit,
                    KeyCode::Char('n') | KeyCode::Char('N') => ExitConfirmAction::Cancel,
                    _ => ExitConfirmAction::None,
                };
                ViewAction::ExitConfirm(a)
            }
            ActiveView::IntakeConfirmation(state) => {
                let visible_height = crossterm::terminal::size()
                    .map(|(_, h)| startup::intake_confirmation::compute_list_visible_height(
                        ratatui::layout::Rect::new(0, 0, 80, h)
                    ))
                    .unwrap_or(10);
                ViewAction::IntakeConfirmation(state.handle_key(key, visible_height))
            }
            ActiveView::UnifiedTagEditor(s) => ViewAction::UnifiedTagEditor(s.handle_key(key)),
            ActiveView::Deploy(s) => ViewAction::Deploy(s.handle_key(key)),
            ActiveView::MissingFileResolution(s) => ViewAction::MissingFileResolution(s.handle_key(key)),
            ActiveView::MissingDirectoryResolution(s) => ViewAction::MissingDirectoryResolution(s.handle_key(key)),
            ActiveView::CorruptFileResolution(s) => ViewAction::CorruptFileResolution(s.handle_key(key)),
            ActiveView::ShitFormatResolution(s) => ViewAction::ShitFormatResolution(s.handle_key(key)),
            ActiveView::EmbedAlbumArtResolution(s) => ViewAction::EmbedAlbumArtResolution(s.handle_key(key)),
            ActiveView::SubparDuplicateResolution(s) => ViewAction::SubparDuplicateResolution(s.handle_key(key)),
            ActiveView::InboxCorpusMatchResolution(s) => ViewAction::InboxCorpusMatchResolution(s.handle_key(key)),
            ActiveView::InboxOrganize(s) => ViewAction::InboxOrganize(s.handle_key(key)),
            ActiveView::DirectoryClusterResolution(s) => ViewAction::DirectoryClusterResolution(s.handle_key(key)),
            ActiveView::MovedFileAcknowledge(s) => ViewAction::MovedFileAcknowledge(s.handle_key(key)),
            ActiveView::OobSyncResolution(s) => ViewAction::OobSyncResolution(s.handle_key(key)),
            ActiveView::OobConflictInspection(s) => ViewAction::OobConflictInspection(s.handle_key(key)),
            ActiveView::TagCanonicityResolution { state, .. } => ViewAction::TagCanonicityResolution(state.handle_key(key)),
            ActiveView::CompoundTagSplit { state, .. } => ViewAction::CompoundTagSplit(state.handle_key(key)),
            ActiveView::MissingAlbumSingleResolution(s) => ViewAction::MissingAlbumSingleResolution(s.handle_key(key)),
            ActiveView::ManualReview(s) => ViewAction::ManualReview(s.handle_key(key)),
            ActiveView::TransactionReview(review) => ViewAction::TransactionReview(review.handle_key(key)),
        };

        // Phase 2: dispatch with confirmation flag
        let is_confirmation = matches!(key.code, KeyCode::Enter);
        self.dispatch_action(action, is_confirmation);
    }

    /// Handle exit confirm modal action.
    fn handle_exit_confirm_action(&mut self, action: ExitConfirmAction) {
        match action {
            ExitConfirmAction::None => {}
            ExitConfirmAction::Quit => {
                self.should_quit = true;
            }
            ExitConfirmAction::Cancel => {
                self.start_health_view();
            }
        }
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
        state.set_db_stats(self.witch.db_stats());
        state.set_worker_stats(self.witch.worker_stats());
    }

    pub(super) fn start_tag_search(&mut self) {
        self.last_lateral_view = widgets::LateralView::Search;
        self.view = ActiveView::TagSearch(tag_search::TagSearchState::new());
    }

    pub(super) fn start_inbox_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::Inbox;
        // Check for inbox unindexed files — show intake popup if any
        let intake_state = {
            let read_db = self.witch.read_db();
            startup::IntakeConfirmationState::gather_inbox(&read_db)
        };

        if let Some(state) = intake_state {
            self.view = ActiveView::IntakeConfirmation(state);
        } else {
            self.view = ActiveView::Inbox(inbox_view::InboxViewState::new());
        }
    }

    /// Start the lateral view identified by the given variant.
    pub(super) fn start_lateral_view(&mut self, view: widgets::LateralView) {
        match view {
            widgets::LateralView::Config => self.start_config_editor(),
            widgets::LateralView::Search => self.start_tag_search(),
            widgets::LateralView::Files => self.start_corpus_browser(),
            widgets::LateralView::Health => self.start_health_view(),
            widgets::LateralView::Inbox => self.start_inbox_view(),
            widgets::LateralView::Transaction => self.start_tabbed_transaction_review(),
            widgets::LateralView::Deploy => self.start_deploy_view(),
        }
    }

    /// Start the tabbed transaction review lateral view.
    pub(super) fn start_tabbed_transaction_review(&mut self) {
        self.last_lateral_view = widgets::LateralView::Transaction;
        self.view = ActiveView::TabbedTransactionReview(
            tabbed_transaction_review::TabbedTransactionReviewState::new(),
        );
    }

    /// Start the config editor view.
    pub(super) fn start_config_editor(&mut self) {
        self.last_lateral_view = widgets::LateralView::Config;
        let config = self.config().clone();
        let kdl_content = crate::config::get_config_dir()
            .ok()
            .map(|dir| dir.join("config.kdl"))
            .and_then(|path| std::fs::read_to_string(path).ok());
        self.view = ActiveView::ConfigEditor(
            config_editor::ConfigEditorState::new(&config, kdl_content),
        );
    }

    /// Start the deploy lateral view.
    ///
    /// If there's work to do (deploy signals present), loads full deploy data
    /// and shows the preview. Otherwise shows the "up to date" modal with
    /// per-library file counts.
    pub(super) fn start_deploy_view(&mut self) {
        self.last_lateral_view = widgets::LateralView::Deploy;
        let deploy_status = self.witch.ui_read_cache().deploy_status();

        let needs_action = deploy_status.as_ref().map_or(false, |s| s.needs_action);

        if needs_action {
            let config = crate::config::load_config().ok();
            let read_db = self.witch.read_db();
            let data = deploy_modal::DeployModalData::load(&read_db, config.as_ref())
                .unwrap_or_default();
            let preview = deploy_modal::DeploymentPreviewState::new(data);
            self.view = ActiveView::Deploy(deploy_modal::DeployViewState::Preview(preview));
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
        let deploy_source_paths: Vec<std::path::PathBuf> = config.source_dirs
            .iter()
            .map(|sd| corpus_dir.join(&sd.path))
            .collect();
        let primary_zone_paths = vec![
            corpus_dir.clone(),
            config.inbox_dir(),
            config.stash_dir(),
        ];
        drop(config);
        self.view = ActiveView::CorpusBrowser(tree_browser::TreeBrowserState::corpus_browser(
            archive_root,
            variant_config,
            deploy_source_paths,
            corpus_dir,
            primary_zone_paths,
        ));
    }

    /// Check Witch status and update UI with any failure messages.
    fn check_witch_status(&mut self) {
        let status = self.witch.status();
        if status.failed > 0 {
            self.status_message = Some(format!(
                "Tasks: {} done, {} failed",
                status.completed, status.failed
            ));
        }
    }

    /// Shorthand for read-only database access.
    pub(super) fn read_db(&mut self) -> crate::corpus::db::ReadOnlyDb<'_> {
        self.witch.read_db()
    }

    // =========================================================================
    // Startup Flow
    // =========================================================================

    /// Complete startup: spawn db_thread, start observing, open persistent txn,
    /// and transition to the Progress screen.
    pub(super) fn complete_startup(&mut self) {
        let shared = self.shared_config.clone();
        self.witch.set_shared_config(shared);
        self.witch.start_observing();

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
    pub(super) fn advance_past_migrations(&mut self, db_path: &std::path::Path, vacuum_threshold: f64) {
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
        ).ok()?;

        let page_count: u64 = conn.pragma_query_value(None, "page_count", |row| row.get(0)).ok()?;
        let freelist_count: u64 = conn.pragma_query_value(None, "freelist_count", |row| row.get(0)).ok()?;
        let page_size: u64 = conn.pragma_query_value(None, "page_size", |row| row.get(0)).ok()?;

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
            compacting_rendered: false,
        })
    }
}

// ============================================================================
// Rendering
// ============================================================================

fn render(f: &mut Frame, app: &mut App) {
    // Fetch decision summaries from Witch if transaction review is active
    let transaction_review_decisions = if matches!(app.view, ActiveView::TransactionReview(_)) {
        transaction_review::fetch_decision_summaries(&app.witch)
    } else {
        Vec::new()
    };

    // Build status bar lines
    let status_line_1 = if app.witch.idle_rescan_active() {
        Some("Refreshing corpus...".to_string())
    } else if let Some(ref msg) = app.status_message {
        Some(msg.clone())
    } else {
        app.view.selected_path().map(|s| s.to_string())
    };

    let status_line_2 = app.witch.transaction_summary()
        .map(|(label, dec, mut_)| {
            let pd = if dec == 1 { "" } else { "s" };
            let pm = if mut_ == 1 { "" } else { "s" };
            format!("Transaction \"{label}\": {dec} decision{pd}, {mut_} mutation{pm} staged")
        });

    render::render_app(f, app, transaction_review_decisions, status_line_1, status_line_2);
}

// ============================================================================
// Entry Point
// ============================================================================

pub fn run_menu(config: Config, log_rx: std::sync::mpsc::Receiver<crate::logging::LogOp>) -> Result<()> {
    crate::logging::log_general("=== MM startup ===");

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
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
    let witch = {
        let cfg = crate::config::read_shared_config(&shared_config);
        crate::witch::Witch::with_opinions(&cfg, false, force_check, Some(log_rx))
    };

    let mut app = App::new_with_witch(shared_config, witch);
    app.vacuum_threshold = vacuum_threshold;
    app.db_path = db_path.clone();

    // Determine initial view based on startup state
    if app.witch.needs_migrations() {
        let descriptions = app.witch.pending_migration_descriptions();
        app.view = ActiveView::MigrationApproval(MigrationApprovalState {
            descriptions,
            phase: MigrationPhase::Approval,
        });
    } else {
        app.advance_past_migrations(&db_path, vacuum_threshold);
    }

    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
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
        crate::logging::log_error(format!(
            "Failed to register signal handlers: {}",
            e
        ));
    }

    loop {
        let frame_start = std::time::Instant::now();

        if signal_received.swap(false, Ordering::SeqCst) {
            let esc_key = KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
            app.handle_key(esc_key);
        }

        // Tick startup views first (they have their own Witch tick calls)
        if matches!(app.view, ActiveView::MigrationApproval(_)) {
            app.tick_migration_approval();
            // If tick changed the view away from MigrationApproval, skip the rest of
            // this frame to let the new view render first.
            if !matches!(app.view, ActiveView::MigrationApproval(_)) {
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
        let is_startup_view = matches!(app.view, ActiveView::MigrationApproval(_) | ActiveView::VacuumPrompt(_));

        // Set idle rescan eligibility based on current view (lateral views only)
        let idle_eligible = matches!(app.view,
            ActiveView::Insights(_)
            | ActiveView::CorpusBrowser(_)
            | ActiveView::TagSearch(_)
            | ActiveView::Inbox(_)
            | ActiveView::TabbedTransactionReview(_)
        );
        app.witch.set_idle_rescan_eligible(idle_eligible);

        // Tick the Witch (skip during startup views — they tick internally as needed)
        let (tick_status, tick_duration) = if !is_startup_view {
            let tick_start = std::time::Instant::now();
            let status = app.witch.tick();
            let duration = tick_start.elapsed();
            if duration.as_millis() > 16 {
                crate::logging::log_perf(format!(
                    "[FRAME DEBUG] witch.tick() took {}ms, drained {} results",
                    duration.as_millis(),
                    status.total_processed
                ));
            }
            (status, duration)
        } else {
            (crate::witch::DaemonStatus::default(), std::time::Duration::ZERO)
        };

        app.check_witch_status();

        // Update health view with the Witch's status and cached data
        if let ActiveView::Insights(ref mut view) = app.view {
            let status = app.witch.status();
            let insights_data = app.witch.ui_read_cache().insights_data();
            view.update(Some(&status), insights_data);
        }

        // Update inbox view with cached overview data and busy state
        if let ActiveView::Inbox(ref mut view) = app.view {
            let inbox_data = app.witch.ui_read_cache().inbox_overview();
            view.update(inbox_data);
            view.busy = tick_status.pending > 0 || tick_status.idle_rescan_active;
        }

        // Tick progress screen if active (includes eye animation update)
        if matches!(app.view, ActiveView::Progress { .. }) {
            app.tick_progress_screen();
        }

        // Tick progressive worker if active
        if matches!(app.view, ActiveView::ProgressiveWork(_)) {
            app.tick_progressive_worker();
        }

        // Update deploy UpToDate with fresh counts each frame
        if let ActiveView::Deploy(deploy_modal::DeployViewState::UpToDate { ref mut library_file_counts }) = app.view {
            if let Some(status) = app.witch.ui_read_cache().deploy_status() {
                *library_file_counts = status.library_file_counts;
            }
        }

        // Flag demand for cached UI data
        // Always want deploy status — titlebar needs it for purple indicator
        app.witch.ui_read_cache().want_deploy_status();
        if matches!(app.view, ActiveView::Insights(_)) {
            app.witch.ui_read_cache().want_insights_data();
        }
        if matches!(app.view, ActiveView::Inbox(_)) {
            app.witch.ui_read_cache().want_inbox_overview();
        }

        let draw_start = std::time::Instant::now();
        terminal.draw(|f| render(f, app))?;
        let draw_duration = draw_start.elapsed();
        if draw_duration.as_millis() > 16 {
            crate::logging::log_perf(format!(
                "[FRAME DEBUG] terminal.draw() took {}ms",
                draw_duration.as_millis()
            ));
        }

        let frame_duration = frame_start.elapsed();
        if frame_duration.as_millis() > 16 {
            crate::logging::log_perf(format!(
                "[FRAME DEBUG] SLOW FRAME: total {}ms (tick={}ms, draw={}ms)",
                frame_duration.as_millis(),
                tick_duration.as_millis(),
                draw_duration.as_millis()
            ));
        }

        // Tick tag search for pending bulk edit (after modal has rendered)
        app.tick_tag_search();

        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                    {
                        let esc_key = KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
                        app.handle_key(esc_key);
                    } else {
                        app.handle_key(key);
                    }
                }
                Event::Mouse(mouse) => {
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            let key = KeyEvent::new(KeyCode::Up, crossterm::event::KeyModifiers::NONE);
                            app.handle_key(key);
                        }
                        MouseEventKind::ScrollDown => {
                            let key = KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE);
                            app.handle_key(key);
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            app.handle_click(mouse.column, mouse.row);
                        }
                        _ => {}
                    }
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
