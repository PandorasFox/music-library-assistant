//! Terminal User Interface
//!
//! ## Module Organization
//! - `types.rs` - UiMode, traits, modal state types
//! - `action_handlers.rs` - View-specific action processing
//! - `tag_editor_ops.rs` - Tag editor launching and navigation
//! - `tick.rs` - Per-frame update logic for each view
//!
//! ## Adding New Views
//! 1. Add variant to `UiMode` in `types.rs`
//! 2. Add action handler in `action_handlers.rs`
//! 3. Add tick handler in `tick.rs` if needed
//! 4. Add render case in `render.rs`
//!
//! ## Existing submodules (views)
//! - `insights_view/` - Main insights dashboard
//! - `tag_editor/` - Unified tag editing
//! - `tag_search/` - Tag search interface
//! - `tree_browser/` - File tree navigation
//! - `deploy_flow/` - Deployment preview

mod action_handlers;
mod tag_editor_ops;
mod tick;
mod types;

pub mod app;
pub mod deploy_flow;
pub mod eye;
pub mod flows;
pub mod helpers;
pub mod insights_view;
pub mod progress_screen;
pub mod render;
pub mod startup;
pub mod tag_editor;
pub mod tag_search;
pub mod tree_browser;
pub mod wait_state;
pub mod widgets;

// Re-export types for convenience
pub(crate) use types::{UiMode, ExitConfirmModalState};

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, MouseEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Frame, Terminal,
};
use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::config::{self, Config};
use app::EyeAnimation;
use types::ProgressStatsUpdater;

// ============================================================================
// Application State
// ============================================================================

/// Main application state
pub(crate) struct App {
    pub(super) config: Config,
    should_quit: bool,
    pub(super) status_message: Option<String>,

    // UI mode and state
    pub(super) mode: UiMode,
    pub(super) tree_browser: Option<tree_browser::TreeBrowserState>,
    pub(super) deployment_preview: Option<deploy_flow::DeploymentPreviewState>,
    // Unified tag editor (transaction-based)
    pub(super) unified_tag_editor: Option<tag_editor::UnifiedTagEditorState>,
    // Exit confirmation modal
    pub(super) exit_confirm_modal_state: Option<ExitConfirmModalState>,
    // Unified progress screen (startup eyeballing, content analysis, signal refresh)
    pub(super) progress_screen: Option<progress_screen::ProgressScreen>,
    // Insights view (lateral view ring)
    pub(super) insights_view: Option<insights_view::InsightsViewState>,
    // Tag search (lateral view ring)
    pub(super) tag_search: Option<tag_search::TagSearchState>,
    // Intake confirmation modal
    pub(super) intake_confirmation: Option<startup::IntakeConfirmationState>,

    // The Witch - enforcer of orderliness, handles all mutations and background work
    pub(super) witch: Option<crate::witch::Witch>,

    // Throughput tracking for rolling average (timestamp, bytes_processed)
    throughput_samples: VecDeque<(Instant, u64)>,

    // Eye animation
    eye: EyeAnimation,
}

impl App {
    fn new(config: Config) -> Self {
        Self {
            config,
            should_quit: false,
            status_message: None,
            mode: UiMode::Insights,
            tree_browser: None,
            deployment_preview: None,
            unified_tag_editor: None,
            exit_confirm_modal_state: None,
            progress_screen: None,
            insights_view: None,
            tag_search: None,
            intake_confirmation: None,
            witch: None,
            throughput_samples: VecDeque::with_capacity(100),
            eye: EyeAnimation::default(),
        }
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        match self.mode {
            UiMode::Progress => {
                // Progress screen ignores keys - can't interact during loading/computation
            }
            UiMode::DirBrowser => {
                if let Some(ref mut browser) = self.tree_browser {
                    let action = browser.handle_key(key);
                    self.handle_tree_browser_action(action);
                }
            }
            UiMode::DeploymentPreview => {
                if let Some(ref mut preview) = self.deployment_preview {
                    let action = preview.handle_key(key);
                    self.handle_deployment_preview_action(action);
                }
            }
            UiMode::ExitConfirmModal => {
                if let Some(ref mut state) = self.exit_confirm_modal_state {
                    match key.code {
                        KeyCode::Left | KeyCode::Right | KeyCode::Tab => {
                            // Toggle between Yes and No
                            state.selected_no = !state.selected_no;
                        }
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            if state.selected_no {
                                // "No" selected - return to Insights view
                                self.exit_confirm_modal_state = None;
                                self.start_insights_view();
                            } else {
                                // "Yes" selected - actually quit
                                self.should_quit = true;
                            }
                        }
                        KeyCode::Esc => {
                            // Esc returns to Insights view
                            self.exit_confirm_modal_state = None;
                            self.start_insights_view();
                        }
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            // 'y' confirms exit
                            self.should_quit = true;
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') => {
                            // 'n' cancels - return to Insights view
                            self.exit_confirm_modal_state = None;
                            self.start_insights_view();
                        }
                        _ => {}
                    }
                }
            }
            UiMode::CorpusBrowser => {
                if let Some(ref mut browser) = self.tree_browser {
                    let action = browser.handle_key(key);
                    self.handle_tree_browser_action(action);
                }
            }
            UiMode::Insights => {
                if let Some(ref mut view) = self.insights_view {
                    let action = view.handle_key(key);
                    self.handle_insights_action(action);
                }
            }
            UiMode::IntakeConfirmation => {
                if let Some(ref state) = self.intake_confirmation {
                    let action = state.handle_key(key);
                    self.handle_intake_confirmation_action(action);
                }
            }
            UiMode::UnifiedTagEditor => {
                if let Some(ref mut editor) = self.unified_tag_editor {
                    let action = editor.handle_key(key);
                    self.handle_unified_tag_editor_action(action);
                }
            }
            UiMode::TagSearch => {
                if let Some(ref mut search) = self.tag_search {
                    let action = search.handle_key(key);
                    self.handle_tag_search_action(action);
                }
            }
        }
    }


    /// Check if there are any pending operations (Witch work).
    pub(super) fn has_pending_operations(&self) -> bool {
        self.witch.as_ref().map(|d| d.has_pending()).unwrap_or(false)
    }

    /// Start the insights view.
    pub(super) fn start_insights_view(&mut self) {
        self.insights_view = Some(insights_view::InsightsViewState::new());
        self.mode = UiMode::Insights;
    }

    /// Stage a decision to the Witch's transaction and update editor state.
    ///
    /// Helper for StageDecision, StageDecisionAndNext, and StageDecisionAndReview actions.
    pub(super) fn stage_decision(&mut self, index: usize, mutations: Vec<crate::corpus::mutations::Mutation>) {
        let witness = crate::witch::confirm_decision();
        if let Some(the_witch) = self.witch.as_mut() {
            let label = self.unified_tag_editor
                .as_ref()
                .map(|e| e.current_item_label())
                .unwrap_or_else(|| "Tag edit".to_string());
            let _ = the_witch.add_decision(index, &witness, label, mutations.clone());
        }
        // Track staged mutations for redundant confirmation skipping
        if let Some(ref mut editor) = self.unified_tag_editor {
            editor.set_staged_mutations(mutations);
        }
        self.status_message = Some(format!("Decision staged (item {})", index + 1));
    }

    /// Gather transaction decisions from the Witch for review modal.
    ///
    /// Returns list of (decision_index, label, mutation_count) for all staged decisions.
    pub(super) fn gather_transaction_decisions(&self) -> Vec<(usize, String, usize)> {
        if let Some(the_witch) = self.witch.as_ref() {
            the_witch.decision_indices()
                .iter()
                .filter_map(|&idx| {
                    the_witch.get_decision(idx).map(|d| {
                        (idx, d.label.clone(), d.mutations.len())
                    })
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Abort current operation and return to insights view with a status message.
    ///
    /// Helper for error handling in start_tag_editor_for_path.
    pub(super) fn abort_to_insights(&mut self, message: String) {
        self.status_message = Some(message);
        self.tree_browser = None;
        self.start_insights_view();
    }

    /// Update Witch stats on a progress state that implements the stats setter methods.
    ///
    /// Helper for updating queue depth, db stats, and worker stats from the Witch.
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
        self.tag_search = Some(tag_search::TagSearchState::new());
        self.mode = UiMode::TagSearch;
    }


    pub(super) fn start_deployment_preview(&mut self) {
        // TODO: Reconnect when corpus::deploy is re-enabled
        // This function requires compute_full_deployment_status from the disabled deploy module.
        self.status_message = Some("Deployment preview disabled - deploy module being updated".to_string());
    }

    pub(super) fn start_corpus_browser(&mut self) {
        let config = tree_browser::CorpusBrowserConfig::default();
        self.tree_browser = Some(tree_browser::TreeBrowserState::corpus_browser(
            self.config.corpus_root.clone(),
            config,
        ));
        self.mode = UiMode::CorpusBrowser;
    }

    /// Check Witch status and update UI with any failure messages.
    /// Note: Actual Witch ticking happens in run_app via witch().tick()
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
            // Load startup opinions from config
            let (read_only, force_freshen) = crate::config::load_config()
                .map(|c| (false, c.opinions.startup.freshen_last_stage_at_startup))
                .unwrap_or((false, false));
            self.witch = Some(crate::witch::Witch::with_opinions(read_only, force_freshen));
        }
        self.witch.as_mut().unwrap()
    }

    /// Shorthand for read-only database access.
    pub(super) fn db(&mut self) -> &crate::corpus::db::Database {
        self.witch().read_only_db()
    }

}

// ============================================================================
// Rendering
// ============================================================================

fn render(f: &mut Frame, app: &mut App) {
    // Use cached corpus summary from the Witch's background cache.
    // Never queries DB - just reads whatever was last computed.
    let corpus_summary = app
        .witch
        .as_ref()
        .and_then(|d| d.ui_read_cache().corpus_summary());

    // Fetch DB thread stats (cheap - just reads cached atomic values)
    // Returns None if timing instrumentation is disabled
    let db_stats = app.witch.as_ref().and_then(|d| d.db_stats());

    // Get Witch status WITHOUT ticking again - status() just reads current state
    let witch_status = app.witch.as_ref().and_then(|w| {
        let status = w.status();
        // Show if there's pending work OR a lingering completed session
        if status.pending > 0 || status.completed_session.is_some() {
            Some(status)
        } else {
            None
        }
    });

    let mut ctx = render::RenderContext {
        mode: app.mode,
        config: &app.config,
        status_message: app.status_message.as_deref(),
        tree_browser: app.tree_browser.as_mut(),
        deployment_preview: app.deployment_preview.as_mut(),
        exit_confirm_modal_state: app.exit_confirm_modal_state.as_ref(),
        progress_screen: app.progress_screen.as_ref(),
        insights_view: app.insights_view.as_mut(),
        tag_search: app.tag_search.as_ref(),
        intake_confirmation: app.intake_confirmation.as_ref(),
        unified_tag_editor: app.unified_tag_editor.as_mut(),
        eye: &app.eye,
        throughput_samples: &app.throughput_samples,
        witch_status,
        corpus_summary,
        db_stats,
    };
    render::render(f, &mut ctx);
}

// ============================================================================
// Entry Point
// ============================================================================

pub fn run_menu(config: Config) -> Result<()> {
    // Log startup with timestamp
    let _ = config::log_message(&format!(
        "=== MLA startup: {} ===",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    ));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Check for and run database migrations before starting app
    startup::check_and_run_migrations(&mut terminal)?;

    let mut app = App::new(config);

    // Observing ALWAYS runs at startup
    // Create progress screen and queue initial observing via the Witch
    let corpus_root = app.config.corpus_root.clone();
    let legacy_library = app.config.legacy_library.clone();

    // Start observing via the Witch - this sets observation_state and queues work
    app.witch().start_observing(&corpus_root, legacy_library.as_deref());

    app.progress_screen = Some(progress_screen::ProgressScreen::new_eyeballing());
    app.mode = UiMode::Progress;

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
    // Set up signal handling - treat SIGINT/SIGTERM as ESC press
    let signal_received = Arc::new(AtomicBool::new(false));

    // Register signal handlers
    if let Err(e) = register_signal_handlers(Arc::clone(&signal_received)) {
        let _ = crate::config::log_message(&format!(
            "[WARN] Failed to register signal handlers: {}",
            e
        ));
    }

    loop {
        let frame_start = std::time::Instant::now();

        // Check if a signal was received - treat as ESC
        if signal_received.swap(false, Ordering::SeqCst) {
            let esc_key = KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
            app.handle_key(esc_key);
        }

        // Tick the Witch first - She is the driving system
        let tick_start = std::time::Instant::now();
        let tick_status = app.witch().tick();
        let tick_duration = tick_start.elapsed();
        if tick_duration.as_millis() > 16 {
            let _ = config::log_message(&format!(
                "[FRAME DEBUG] witch.tick() took {}ms, drained {} results",
                tick_duration.as_millis(),
                tick_status.total_processed
            ));
        }

        app.check_witch_status();

        // Update eye animation - only animate when the Witch's eye is Awake
        let can_animate = app.witch().eye_state() == crate::witch::EyeState::Awake;
        app.eye.update(can_animate);

        // Update insights view with the Witch's status and cached data
        if let Some(ref mut view) = app.insights_view {
            let status = app.witch.as_ref().map(|d| d.status());
            let insights_data = app.witch.as_ref().and_then(|w| w.ui_read_cache().insights_data());
            view.update(status.as_ref(), insights_data);
        }

        // Tick progress screen if active (startup eyeballing, content analysis, etc.)
        if app.progress_screen.is_some() {
            app.tick_progress_screen();
        }

        // Flag demand for cached UI data (the Witch spawns background refresh if needed)
        if let Some(ref the_witch) = app.witch {
            the_witch.ui_read_cache().want_corpus_summary();
            // Request insights data when viewing insights
            if app.mode == UiMode::Insights {
                the_witch.ui_read_cache().want_insights_data();
            }
        }

        let draw_start = std::time::Instant::now();
        terminal.draw(|f| render(f, app))?;
        let draw_duration = draw_start.elapsed();
        if draw_duration.as_millis() > 16 {
            let _ = config::log_message(&format!(
                "[FRAME DEBUG] terminal.draw() took {}ms",
                draw_duration.as_millis()
            ));
        }

        let frame_duration = frame_start.elapsed();
        if frame_duration.as_millis() > 16 {
            let _ = config::log_message(&format!(
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
                    // Ctrl+C: treat as ESC for graceful exit handling
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
                    // Map scroll wheel to arrow keys
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            let key = KeyEvent::new(KeyCode::Up, crossterm::event::KeyModifiers::NONE);
                            app.handle_key(key);
                        }
                        MouseEventKind::ScrollDown => {
                            let key = KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE);
                            app.handle_key(key);
                        }
                        _ => {} // Ignore other mouse events
                    }
                }
                _ => {} // Ignore other events (resize, etc.)
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Register signal handlers for graceful shutdown.
///
/// SIGINT (Ctrl+C) and SIGTERM are caught and converted to ESC key presses
/// so the app can handle them gracefully (show exit confirmation, etc.).
fn register_signal_handlers(flag: Arc<AtomicBool>) -> Result<(), Box<dyn std::error::Error>> {
    use signal_hook::consts::{SIGINT, SIGTERM};
    use signal_hook::flag;

    // Register both SIGINT and SIGTERM to set the flag
    flag::register(SIGINT, Arc::clone(&flag))?;
    flag::register(SIGTERM, flag)?;

    Ok(())
}
