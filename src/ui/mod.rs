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
mod action_handlers;
mod tag_editor_ops;
mod tick;
mod types;

pub mod operator_decisions;
pub mod transaction_review;

pub mod bulk_selection;
pub mod compound_split_v2;
pub mod corrupt_file_modal;
pub mod deploy_modal;
pub mod directory_cluster_modal;
pub mod eye;
pub mod filter_popup;
pub mod helpers;
pub mod insights_view;
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
    FilterPopupContext, SuspendedView, TagCanonicityClusters, ViewAction,
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

use crate::config::Config;

// ============================================================================
// Application State
// ============================================================================

/// Main application state
pub(crate) struct App {
    pub(super) config: Config,
    should_quit: bool,
    pub(super) status_message: Option<String>,

    /// The active view and its state. One variant is active at a time.
    pub(super) view: ActiveView,

    // The Witch - enforcer of orderliness, handles all mutations and background work
    pub(super) witch: Option<crate::witch::Witch>,

    // Log channel receiver, held until the Witch takes ownership
    log_rx: Option<std::sync::mpsc::Receiver<crate::logging::LogOp>>,

    // Filter popup overlay (Ctrl+F in resolution modals and corpus browser)
    pub(super) filter_overlay: Option<FilterOverlay>,
}

impl App {
    /// Create a new App with a pre-existing Witch instance.
    fn new_with_witch(config: Config, witch: crate::witch::Witch) -> Self {
        Self {
            config,
            should_quit: false,
            status_message: None,
            view: ActiveView::Insights(insights_view::InsightsViewState::new()),
            witch: Some(witch),
            log_rx: None,
            filter_overlay: None,
        }
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
                                if let Some(ref mut witch) = self.witch {
                                    let read_db = witch.read_db();
                                    browser.apply_filter(condition, &read_db);
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

        // Phase 1: borrow view, produce action
        let action = match &mut self.view {
            ActiveView::Progress { .. } => ViewAction::None,
            ActiveView::ProgressiveWork { .. } => ViewAction::None,
            ActiveView::Insights(s) => ViewAction::Insights(s.handle_key(key)),
            ActiveView::CorpusBrowser(s) => ViewAction::CorpusBrowser(s.handle_key(key)),
            ActiveView::TagSearch(s) => ViewAction::TagSearch(s.handle_key(key)),
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
            ActiveView::DeploymentPreview(s) => ViewAction::DeploymentPreview(s.handle_key(key)),
            ActiveView::MissingFileResolution(s) => ViewAction::MissingFileResolution(s.handle_key(key)),
            ActiveView::MissingDirectoryResolution(s) => ViewAction::MissingDirectoryResolution(s.handle_key(key)),
            ActiveView::CorruptFileResolution(s) => ViewAction::CorruptFileResolution(s.handle_key(key)),
            ActiveView::ShitFormatResolution(s) => ViewAction::ShitFormatResolution(s.handle_key(key)),
            ActiveView::SubparDuplicateResolution(s) => ViewAction::SubparDuplicateResolution(s.handle_key(key)),
            ActiveView::DirectoryClusterResolution(s) => ViewAction::DirectoryClusterResolution(s.handle_key(key)),
            ActiveView::MovedFileAcknowledge(s) => ViewAction::MovedFileAcknowledge(s.handle_key(key)),
            ActiveView::OobSyncResolution(s) => ViewAction::OobSyncResolution(s.handle_key(key)),
            ActiveView::OobConflictInspection(s) => ViewAction::OobConflictInspection(s.handle_key(key)),
            ActiveView::TagCanonicityResolution { state, .. } => ViewAction::TagCanonicityResolution(state.handle_key(key)),
            ActiveView::CompoundTagSplit { state, .. } => ViewAction::CompoundTagSplit(state.handle_key(key)),
            ActiveView::TransactionReview { review, .. } => ViewAction::TransactionReview(review.handle_key(key)),
        };

        // Phase 2: dispatch action (borrows self freely)
        self.dispatch_action(action);
    }

    /// Dispatch a view action to the appropriate handler.
    fn dispatch_action(&mut self, action: ViewAction) {
        match action {
            ViewAction::None => {}
            ViewAction::Insights(a) => self.handle_insights_action(a),
            ViewAction::CorpusBrowser(a) => self.handle_tree_browser_action(a),
            ViewAction::TagSearch(a) => self.handle_tag_search_action(a),
            ViewAction::ExitConfirm(a) => self.handle_exit_confirm_action(a),
            ViewAction::IntakeConfirmation(a) => self.handle_intake_confirmation_action(a),
            ViewAction::UnifiedTagEditor(a) => self.handle_unified_tag_editor_action(a),
            ViewAction::DeploymentPreview(a) => self.handle_deployment_preview_action(a),
            ViewAction::MissingFileResolution(a) => self.handle_missing_file_preview_action(a),
            ViewAction::MissingDirectoryResolution(a) => self.handle_missing_directory_preview_action(a),
            ViewAction::CorruptFileResolution(a) => self.handle_corrupt_file_preview_action(a),
            ViewAction::ShitFormatResolution(a) => self.handle_shit_format_preview_action(a),
            ViewAction::SubparDuplicateResolution(a) => self.handle_subpar_duplicate_preview_action(a),
            ViewAction::DirectoryClusterResolution(a) => self.handle_directory_cluster_preview_action(a),
            ViewAction::MovedFileAcknowledge(a) => self.handle_moved_file_action(a),
            ViewAction::OobSyncResolution(a) => self.handle_oob_sync_action(a),
            ViewAction::OobConflictInspection(a) => self.handle_oob_conflict_action(a),
            ViewAction::TagCanonicityResolution(a) => self.handle_tag_canonicity_action(a),
            ViewAction::CompoundTagSplit(a) => self.handle_compound_split_action(a),
            ViewAction::TransactionReview(a) => self.handle_transaction_review_action(a),
        }
    }

    /// Handle exit confirm modal action.
    fn handle_exit_confirm_action(&mut self, action: ExitConfirmAction) {
        match action {
            ExitConfirmAction::None => {}
            ExitConfirmAction::Quit => {
                self.should_quit = true;
            }
            ExitConfirmAction::Cancel => {
                self.start_insights_view();
            }
        }
    }

    /// Check if there are any pending operations (Witch work).
    pub(super) fn has_pending_operations(&self) -> bool {
        self.witch.as_ref().map(|d| d.has_pending()).unwrap_or(false)
    }

    /// Start the insights view.
    pub(super) fn start_insights_view(&mut self) {
        self.view = ActiveView::Insights(insights_view::InsightsViewState::new());
    }

    /// Stage a decision to the Witch's transaction and update editor state.
    pub(super) fn stage_decision(&mut self, index: usize, mutations: Vec<crate::meta::mutations::Mutation>) {
        if let Some(the_witch) = self.witch.as_mut() {
            let label = if let ActiveView::UnifiedTagEditor(ref editor) = self.view {
                editor.current_item_label()
            } else {
                "Tag edit".to_string()
            };
            let _ = operator_decisions::stage_decision(the_witch, index, &label, mutations.clone());
        }
        if let ActiveView::UnifiedTagEditor(ref mut editor) = self.view {
            editor.set_staged_mutations(mutations);
        }
        self.status_message = Some(format!("Decision staged (item {})", index + 1));
    }

    /// Abort current operation and return to insights view with a status message.
    pub(super) fn abort_to_insights(&mut self, message: String) {
        self.status_message = Some(message);
        self.start_insights_view();
    }

    /// Update Witch stats on a progress state that implements the stats setter methods.
    pub(super) fn update_progress_stats<T>(&self, state: &mut T)
    where
        T: ProgressStatsUpdater,
    {
        if let Some(the_witch) = &self.witch {
            state.set_db_queue_depth(the_witch.db_queue_depth());
            state.set_db_stats(the_witch.db_stats());
            state.set_worker_stats(the_witch.worker_stats());
        }
    }

    pub(super) fn start_tag_search(&mut self) {
        self.view = ActiveView::TagSearch(tag_search::TagSearchState::new());
    }

    /// Start the lateral view identified by the given variant.
    pub(super) fn start_lateral_view(&mut self, view: widgets::LateralView) {
        match view {
            widgets::LateralView::TagSearch => self.start_tag_search(),
            widgets::LateralView::CorpusBrowser => self.start_corpus_browser(),
            widgets::LateralView::Insights => self.start_insights_view(),
        }
    }

    pub(super) fn start_corpus_browser(&mut self) {
        let config = tree_browser::CorpusBrowserConfig::default();
        self.view = ActiveView::CorpusBrowser(tree_browser::TreeBrowserState::corpus_browser(
            self.config.corpus_dir(),
            config,
        ));
    }

    /// Check Witch status and update UI with any failure messages.
    fn check_witch_status(&mut self) {
        if let Some(ref the_witch) = self.witch {
            let status = the_witch.status();
            if status.failed > 0 {
                self.status_message = Some(format!(
                    "Tasks: {} done, {} failed",
                    status.completed, status.failed
                ));
            }
        }
    }

    /// Get or create the Witch.
    pub(super) fn witch(&mut self) -> &mut crate::witch::Witch {
        if self.witch.is_none() {
            let force_freshen = self.config.opinions.startup.freshen_last_stage_at_startup;
            let force_check = self.config.opinions.startup.force_check_all_files_at_startup;
            let log_rx = self.log_rx.take();
            self.witch = Some(crate::witch::Witch::with_opinions(&self.config, false, force_freshen, force_check, log_rx));
        }
        self.witch.as_mut().unwrap()
    }

    /// Shorthand for read-only database access.
    pub(super) fn read_db(&mut self) -> crate::corpus::db::ReadOnlyDb<'_> {
        self.witch().read_db()
    }
}

// ============================================================================
// Rendering
// ============================================================================

fn render(f: &mut Frame, app: &mut App) {
    // Fetch decision summaries from Witch if transaction review is active
    let transaction_review_decisions = if matches!(app.view, ActiveView::TransactionReview { .. }) {
        app.witch.as_ref()
            .map(transaction_review::fetch_decision_summaries)
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    // Build status bar lines
    let status_line_1 = if let Some(ref msg) = app.status_message {
        Some(msg.clone())
    } else {
        app.view.selected_path().map(|s| s.to_string())
    };

    let status_line_2 = app.witch.as_ref()
        .and_then(|w| w.transaction_summary())
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

    let force_freshen = config.opinions.startup.freshen_last_stage_at_startup;
    let force_check = config.opinions.startup.force_check_all_files_at_startup;
    let mut witch = crate::witch::Witch::with_opinions(&config, false, force_freshen, force_check, Some(log_rx));

    if witch.needs_migrations() {
        startup::run_migrations(&mut terminal, &mut witch)?;
    }

    witch.spawn_db_thread();

    let mut app = App::new_with_witch(config, witch);

    app.witch().start_observing();

    app.view = ActiveView::Progress {
        screen: progress_screen::ProgressScreen::new_eyeballing(),
        eye: eye::Eye::default(),
    };

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

        // Tick the Witch first
        let tick_start = std::time::Instant::now();
        let tick_status = app.witch().tick();
        let tick_duration = tick_start.elapsed();
        if tick_duration.as_millis() > 16 {
            crate::logging::log_perf(format!(
                "[FRAME DEBUG] witch.tick() took {}ms, drained {} results",
                tick_duration.as_millis(),
                tick_status.total_processed
            ));
        }

        app.check_witch_status();

        // Update insights view with the Witch's status and cached data
        if let ActiveView::Insights(ref mut view) = app.view {
            let status = app.witch.as_ref().map(|d| d.status());
            let insights_data = app.witch.as_ref().and_then(|w| w.ui_read_cache().insights_data());
            view.update(status.as_ref(), insights_data);
        }

        // Tick progress screen if active (includes eye animation update)
        if matches!(app.view, ActiveView::Progress { .. }) {
            app.tick_progress_screen();
        }

        // Tick progressive worker if active
        if matches!(app.view, ActiveView::ProgressiveWork { .. }) {
            app.tick_progressive_worker();
        }

        // Flag demand for cached UI data
        if let Some(ref the_witch) = app.witch {
            if matches!(app.view, ActiveView::Insights(_)) {
                the_witch.ui_read_cache().want_insights_data();
            }
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
