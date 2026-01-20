//! TUI Module
//!
//! Modular UI components for the Music Library Assistant.

pub mod app;
pub mod cache;
pub mod deploy_flow;
pub mod eye;
pub mod flows;
pub mod helpers;
pub mod insights_view;
pub mod render;
pub mod splash_screen;
pub mod startup;
pub mod tag_editor;
pub mod tag_search;
pub mod tree_browser;
pub mod widgets;

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

// ============================================================================
// Progress Stats Trait
// ============================================================================

/// Trait for progress states that display daemon statistics.
///
/// Allows generic stats update logic in tick functions.
trait ProgressStatsUpdater {
    fn set_db_queue_depth(&mut self, depth: u64);
    fn set_db_stats(&mut self, stats: Option<crate::db_thread::DbThreadStats>);
    fn set_worker_stats(&mut self, stats: Option<crate::daemon::WorkerStats>);
}

impl ProgressStatsUpdater for splash_screen::SplashScreen {
    fn set_db_queue_depth(&mut self, depth: u64) {
        splash_screen::SplashScreen::set_db_queue_depth(self, depth);
    }
    fn set_db_stats(&mut self, stats: Option<crate::db_thread::DbThreadStats>) {
        splash_screen::SplashScreen::set_db_stats(self, stats);
    }
    fn set_worker_stats(&mut self, stats: Option<crate::daemon::WorkerStats>) {
        splash_screen::SplashScreen::set_worker_stats(self, stats);
    }
}

impl ProgressStatsUpdater for startup::ContentAnalysisProgress {
    fn set_db_queue_depth(&mut self, depth: u64) {
        startup::ContentAnalysisProgress::set_db_queue_depth(self, depth);
    }
    fn set_db_stats(&mut self, stats: Option<crate::db_thread::DbThreadStats>) {
        startup::ContentAnalysisProgress::set_db_stats(self, stats);
    }
    fn set_worker_stats(&mut self, stats: Option<crate::daemon::WorkerStats>) {
        startup::ContentAnalysisProgress::set_worker_stats(self, stats);
    }
}

impl ProgressStatsUpdater for startup::IntakeConfirmationState {
    fn set_db_queue_depth(&mut self, depth: u64) {
        startup::IntakeConfirmationState::set_db_queue_depth(self, depth);
    }
    fn set_db_stats(&mut self, stats: Option<crate::db_thread::DbThreadStats>) {
        startup::IntakeConfirmationState::set_db_stats(self, stats);
    }
    fn set_worker_stats(&mut self, stats: Option<crate::daemon::WorkerStats>) {
        startup::IntakeConfirmationState::set_worker_stats(self, stats);
    }
}

// ============================================================================
// Application State
// ============================================================================

/// Current UI mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum UiMode {
    /// Content analysis progress screen - post-intake health signal computation
    ContentAnalysis,
    DirBrowser,
    DeploymentPreview,
    /// Exit confirmation modal (when operations are in progress)
    ExitConfirmModal,
    /// Corpus browser with directory tree and metadata preview
    CorpusBrowser,
    /// Full-screen insights view (part of lateral view ring)
    Insights,
    /// Intake confirmation - prompt to index unindexed files
    IntakeConfirmation,
    /// Loading splash screen - centered eye with status message
    LoadingSplash,
    /// Tag search with query builder and results (part of lateral view ring)
    TagSearch,
    /// Unified tag editor with transaction support (replaces TagEditor and DirectoryTagEditor)
    UnifiedTagEditor,
}

/// State for the exit confirmation modal.
/// Default selection is "No" (stay in application).
/// Pressing Esc/Enter/Space when selected_no=true returns to main menu.
#[derive(Debug, Clone, Default)]
pub(crate) struct ExitConfirmModalState {
    /// True = "No" selected (default), False = "Yes" selected
    pub selected_no: bool,
    /// True if operations are in progress (shows warning), False for simple exit prompt
    pub has_operations: bool,
}

impl ExitConfirmModalState {
    pub fn new(has_operations: bool) -> Self {
        Self {
            selected_no: true,
            has_operations,
        }
    }
}

// LoadingType and SplashScreen are now in splash_screen module

/// Main application state
pub(crate) struct App {
    config: Config,
    should_quit: bool,
    status_message: Option<String>,

    // UI mode and state
    mode: UiMode,
    tree_browser: Option<tree_browser::TreeBrowserState>,
    deployment_preview: Option<deploy_flow::DeploymentPreviewState>,
    // Unified tag editor (transaction-based)
    unified_tag_editor: Option<tag_editor::UnifiedTagEditorState>,
    // Exit confirmation modal
    exit_confirm_modal_state: Option<ExitConfirmModalState>,
    // Startup splash screen
    splash_screen: Option<splash_screen::SplashScreen>,
    // Insights view (lateral view ring)
    insights_view: Option<insights_view::InsightsViewState>,
    // Tag search (lateral view ring)
    tag_search: Option<tag_search::TagSearchState>,
    // Intake confirmation modal
    intake_confirmation: Option<startup::IntakeConfirmationState>,
    // Content analysis progress screen
    content_analysis: Option<startup::ContentAnalysisProgress>,

    // Task daemon for mutation execution
    task_daemon: Option<crate::daemon::TaskDaemon>,

    // Throughput tracking for rolling average (timestamp, bytes_processed)
    throughput_samples: VecDeque<(Instant, u64)>,

    // Eye animation
    eye: EyeAnimation,

    // UI cache - all cached DB results for rendering (never query DB directly in render code)
    ui_cache: cache::UiCache,
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
            splash_screen: None,
            insights_view: None,
            tag_search: None,
            intake_confirmation: None,
            content_analysis: None,
            task_daemon: None,
            throughput_samples: VecDeque::with_capacity(100),
            eye: EyeAnimation::default(),
            ui_cache: cache::UiCache::new(),
        }
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        match self.mode {
            UiMode::ContentAnalysis => {
                // Content analysis ignores keys - can't interact during computation
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
            UiMode::LoadingSplash => {
                // Loading splash ignores most keys - can't interact during loading
                // Could potentially allow Esc to cancel certain operations in the future
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

    fn handle_insights_action(&mut self, action: insights_view::InsightsAction) {
        match action {
            insights_view::InsightsAction::None => {}
            insights_view::InsightsAction::RequestQuit => {
                // Check if operations are pending
                if self.has_pending_operations() {
                    self.status_message = Some("Cannot quit while operations are pending".to_string());
                } else {
                    // Show exit confirmation modal
                    self.exit_confirm_modal_state = Some(ExitConfirmModalState::default());
                    self.mode = UiMode::ExitConfirmModal;
                }
            }
            insights_view::InsightsAction::CycleNext => {
                // Insights → Deploy
                self.insights_view = None;
                self.start_deployment_preview();
            }
            insights_view::InsightsAction::CyclePrev => {
                // Insights → Corpus Browser
                self.insights_view = None;
                self.start_corpus_browser();
            }
            insights_view::InsightsAction::LaunchFlow => {
                // Stub: flows not yet implemented
                self.status_message = Some("Flows not yet implemented".to_string());
            }
        }
    }

    /// Handle tag search actions.
    fn handle_tag_search_action(&mut self, action: tag_search::TagSearchAction) {
        match action {
            tag_search::TagSearchAction::None => {}
            tag_search::TagSearchAction::Cancel => {
                // Return to Insights view
                self.tag_search = None;
                self.start_insights_view();
            }
            tag_search::TagSearchAction::CycleNext => {
                // TagSearch → CorpusBrowser
                self.tag_search = None;
                self.start_corpus_browser();
            }
            tag_search::TagSearchAction::CyclePrev => {
                // TagSearch → Deploy
                self.tag_search = None;
                self.start_deployment_preview();
            }
            tag_search::TagSearchAction::ExecuteSearch => {
                // Execute search with db access - take ownership temporarily to avoid borrow conflict
                if let Some(mut search) = self.tag_search.take() {
                    let db = self.db();
                    search.execute_search(&db);
                    self.tag_search = Some(search);
                }
            }
            tag_search::TagSearchAction::EditTrack(track) => {
                // Open unified tag editor for single track
                self.tag_search = None;
                self.start_unified_tag_editor_for_track(track);
            }
            tag_search::TagSearchAction::EditAllTracks(tracks) => {
                // Open unified tag editor for all result tracks
                self.tag_search = None;
                self.start_unified_tag_editor_for_tracks(tracks);
            }
        }
    }

    /// Handle intake confirmation dialog actions.
    fn handle_intake_confirmation_action(&mut self, action: startup::IntakeConfirmationAction) {
        use crate::daemon::confirm_decision;

        match action {
            startup::IntakeConfirmationAction::None => {}
            startup::IntakeConfirmationAction::Confirmed => {
                // User confirmed - create IndexTrack mutations and start processing
                // Extract mutations first to avoid borrow conflicts
                let mutations = self.intake_confirmation
                    .as_ref()
                    .map(|s| s.create_index_mutations())
                    .unwrap_or_default();

                if !mutations.is_empty() {
                    let count = mutations.len();

                    let _ = config::log_message(&format!(
                        "IntakeConfirmation: user confirmed, queuing {} IndexTrack mutations",
                        count
                    ));

                    // Use the transaction API to queue mutations
                    let daemon = self.daemon();
                    if daemon.start_transaction("Intake indexing").is_ok() {
                        let witness = confirm_decision();
                        let _ = daemon.add_decision(0, &witness, "Index unindexed files", mutations);
                        let _ = daemon.confirm_transaction(&witness);
                    }

                    // Start processing mode - stay on this screen until complete
                    if let Some(ref mut state) = self.intake_confirmation {
                        state.start_processing();
                    }
                }
            }
            startup::IntakeConfirmationAction::Skipped => {
                // User skipped - proceed to metadata analysis without indexing
                // UnindexedFile signals remain for later handling
                let _ = config::log_message("IntakeConfirmation: user skipped indexing");

                self.intake_confirmation = None;
                self.start_content_analysis();
            }
            startup::IntakeConfirmationAction::ProcessingComplete => {
                // Indexing complete - proceed to metadata analysis
                let _ = config::log_message("IntakeConfirmation: indexing complete, proceeding to metadata analysis");

                self.intake_confirmation = None;
                self.start_content_analysis();
            }
        }
    }

    /// Check if there are any pending operations (daemon work).
    fn has_pending_operations(&self) -> bool {
        self.task_daemon.as_ref().map(|d| d.has_pending()).unwrap_or(false)
    }

    /// Start the insights view.
    fn start_insights_view(&mut self) {
        self.insights_view = Some(insights_view::InsightsViewState::new());
        self.mode = UiMode::Insights;
    }

    /// Stage a decision to the daemon's transaction and update editor state.
    ///
    /// Helper for StageDecision, StageDecisionAndNext, and StageDecisionAndReview actions.
    fn stage_decision(&mut self, index: usize, mutations: Vec<crate::corpus::mutations::Mutation>) {
        let witness = crate::daemon::confirm_decision();
        if let Some(daemon) = self.task_daemon.as_mut() {
            let label = self.unified_tag_editor
                .as_ref()
                .map(|e| e.current_item_label())
                .unwrap_or_else(|| "Tag edit".to_string());
            let _ = daemon.add_decision(index, &witness, label, mutations.clone());
        }
        // Track staged mutations for redundant confirmation skipping
        if let Some(ref mut editor) = self.unified_tag_editor {
            editor.set_staged_mutations(mutations);
        }
        self.status_message = Some(format!("Decision staged (item {})", index + 1));
    }

    /// Gather transaction decisions from daemon for review modal.
    ///
    /// Returns list of (decision_index, label, mutation_count) for all staged decisions.
    fn gather_transaction_decisions(&self) -> Vec<(usize, String, usize)> {
        if let Some(daemon) = self.task_daemon.as_ref() {
            daemon.decision_indices()
                .iter()
                .filter_map(|&idx| {
                    daemon.get_decision(idx).map(|d| {
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
    fn abort_to_insights(&mut self, message: String) {
        self.status_message = Some(message);
        self.tree_browser = None;
        self.start_insights_view();
    }

    /// Update daemon stats on a progress state that implements the stats setter methods.
    ///
    /// Helper for updating queue depth, db stats, and worker stats from daemon.
    fn update_progress_stats<T>(&self, state: &mut T)
    where
        T: ProgressStatsUpdater,
    {
        if let Some(daemon) = &self.task_daemon {
            state.set_db_queue_depth(daemon.db_queue_depth());
            state.set_db_stats(daemon.db_stats());
            state.set_worker_stats(daemon.worker_stats());
        }
    }

    /// Start content analysis phase (after intake).
    ///
    /// Queues content analysis computations and shows the progress screen.
    fn start_content_analysis(&mut self) {
        let _ = config::log_message("Starting content analysis phase");

        // Queue content analysis computations
        self.daemon().queue_content_analysis();

        // Create progress screen and transition
        self.content_analysis = Some(startup::ContentAnalysisProgress::new());
        self.mode = UiMode::ContentAnalysis;
    }

    fn start_tag_search(&mut self) {
        self.tag_search = Some(tag_search::TagSearchState::new());
        self.mode = UiMode::TagSearch;
    }

    /// Start unified tag editor for a single track from tag search results
    fn start_unified_tag_editor_for_track(&mut self, track: crate::corpus::db::Track) {
        self.open_unified_tag_editor_single(
            track,
            tag_editor::TagEditorSource::TagSearch,
            None,
        );
    }

    /// Start unified tag editor for aggregated bulk editing from tag search results
    fn start_unified_tag_editor_for_tracks(&mut self, tracks: Vec<crate::corpus::db::Track>) {
        // Start transaction
        if let Some(daemon) = self.task_daemon.as_mut() {
            let _ = daemon.start_transaction("Tag search bulk edit");
        }

        // Use aggregated mode - all tracks edited as one unit
        self.unified_tag_editor = Some(tag_editor::UnifiedTagEditorState::aggregated_bulk(
            tracks,
            tag_editor::TagEditorSource::TagSearch,
        ));
        self.mode = UiMode::UnifiedTagEditor;
    }

    fn start_deployment_preview(&mut self) {
        // TODO: Reconnect when corpus::deploy is re-enabled
        // This function requires compute_full_deployment_status from the disabled deploy module.
        self.status_message = Some("Deployment preview disabled - deploy module being updated".to_string());
    }

    fn start_corpus_browser(&mut self) {
        let config = tree_browser::CorpusBrowserConfig::default();
        self.tree_browser = Some(tree_browser::TreeBrowserState::corpus_browser(
            self.config.corpus_root.clone(),
            config,
        ));
        self.mode = UiMode::CorpusBrowser;
    }

    fn handle_tree_browser_action(&mut self, action: tree_browser::TreeBrowserAction) {
        match action {
            tree_browser::TreeBrowserAction::None => {}
            tree_browser::TreeBrowserAction::Cancel => {
                self.tree_browser = None;
                self.start_insights_view();
            }
            tree_browser::TreeBrowserAction::EditDirectory(path) => {
                // Load tracks from directory and open unified tag editor
                self.open_unified_tag_editor_for_directory(&path);
            }
            tree_browser::TreeBrowserAction::EditFile(path) => {
                // Load single track for editing
                self.start_tag_editor_for_path(&path, false);
            }
            tree_browser::TreeBrowserAction::CycleNext => {
                // Corpus Browser → Insights
                self.tree_browser = None;
                self.start_insights_view();
            }
            tree_browser::TreeBrowserAction::CyclePrev => {
                // Corpus Browser → TagSearch
                self.tree_browser = None;
                self.start_tag_search();
            }
            tree_browser::TreeBrowserAction::SelectPaths(paths) => {
                // Directory selector completed - currently unused, placeholder for dedup flows
                let _ = crate::config::log_message(&format!(
                    "Directory selector returned {} paths (flow not yet wired)",
                    paths.len()
                ));
                self.tree_browser = None;
                self.start_insights_view();
            }
        }
    }

    fn start_tag_editor_for_path(&mut self, path: &std::path::Path, recursive: bool) {
        let db = self.db();

        // Load tracks from database
        let (tracks, selected_idx) = if recursive {
            // Get all tracks in directory and subdirectories (no fingerprint filter)
            match db.get_tracks_for_tag_editing(path) {
                Ok(t) => (t, 0usize),
                Err(e) => {
                    self.abort_to_insights(format!(
                        "Query error for path '{}': {}",
                        path.display(),
                        e
                    ));
                    return;
                }
            }
        } else {
            // Get all tracks in the same directory for cycling with tab/shift-tab
            let parent_dir = match path.parent() {
                Some(p) => p,
                None => {
                    self.abort_to_insights(format!(
                        "Cannot determine parent directory: {}",
                        path.display()
                    ));
                    return;
                }
            };

            // Load all tracks from parent directory (non-recursive, just this folder)
            let dir_tracks = match db.get_tracks_for_tag_editing(parent_dir) {
                Ok(t) => t,
                Err(e) => {
                    self.abort_to_insights(format!(
                        "Query error for directory '{}': {}",
                        parent_dir.display(),
                        e
                    ));
                    return;
                }
            };

            // Filter to only tracks directly in this directory (not subdirectories)
            let path_str = path.to_string_lossy().to_string();
            let parent_str = parent_dir.to_string_lossy().to_string();
            let tracks_in_dir: Vec<_> = dir_tracks
                .into_iter()
                .filter(|t| {
                    // Check if track is directly in parent_dir (no additional path separators)
                    if let Some(rel) = t.path.strip_prefix(&parent_str) {
                        let rel = rel.trim_start_matches(std::path::MAIN_SEPARATOR);
                        !rel.contains(std::path::MAIN_SEPARATOR)
                    } else {
                        false
                    }
                })
                .collect();

            // Find the index of the selected track
            let selected_idx = tracks_in_dir
                .iter()
                .position(|t| t.path == path_str)
                .unwrap_or(0);

            if tracks_in_dir.is_empty() {
                // Fallback: try to get just the single track
                let path_str = path.to_string_lossy();
                match db.get_track_by_path(&path_str) {
                    Ok(Some(track)) => (vec![track], 0),
                    Ok(None) => {
                        self.abort_to_insights(format!(
                            "Track not in index: {}",
                            path.display()
                        ));
                        return;
                    }
                    Err(e) => {
                        self.abort_to_insights(format!(
                            "Query error for '{}': {}",
                            path.display(),
                            e
                        ));
                        return;
                    }
                }
            } else {
                (tracks_in_dir, selected_idx)
            }
        };

        if tracks.is_empty() {
            self.abort_to_insights(format!(
                "No indexed tracks at: {}",
                path.display()
            ));
            return;
        }

        // Use the unified tag editor for single-file editing
        self.tree_browser = None;
        if tracks.len() == 1 {
            // Single track - use single file mode
            self.open_unified_tag_editor_single(
                tracks.into_iter().next().unwrap(),
                tag_editor::TagEditorSource::CorpusBrowser,
                None,
            );
        } else {
            // Multiple tracks in same directory - use bulk mode with CorpusBrowser source
            // This allows cycling through sibling files with tab/shift-tab
            self.open_unified_tag_editor_bulk(
                tracks,
                tag_editor::TagEditorSource::CorpusBrowser,
                None,
            );
            // Position on the selected track
            if let Some(editor) = self.unified_tag_editor.as_mut() {
                editor.current_item_idx = selected_idx;
            }
        }
        self.status_message = Some(format!("Editing tags for {}", path.display()));
    }

    // =========================================================================
    // Unified Tag Editor (Transaction-Based)
    // =========================================================================

    /// Open the unified tag editor with a single track
    fn open_unified_tag_editor_single(
        &mut self,
        track: crate::corpus::db::Track,
        source: tag_editor::TagEditorSource,
        group_context: Option<tag_editor::GroupContext>,
    ) {
        // Start transaction
        if let Some(daemon) = self.task_daemon.as_mut() {
            let label = match source {
                tag_editor::TagEditorSource::CorpusBrowser => "Tag edits",
                tag_editor::TagEditorSource::DirectoryEdit => "Directory tag edits",
                tag_editor::TagEditorSource::DuplicateResolution => "Duplicate resolution",
                tag_editor::TagEditorSource::DeployConflict => "Deploy conflict resolution",
                tag_editor::TagEditorSource::TagSearch => "Tag search edits",
            };
            let _ = daemon.start_transaction(label);
        }

        self.unified_tag_editor = Some(tag_editor::UnifiedTagEditorState::single_file(
            track,
            source,
            group_context,
        ));
        self.mode = UiMode::UnifiedTagEditor;
    }

    /// Open the unified tag editor with multiple tracks
    fn open_unified_tag_editor_bulk(
        &mut self,
        tracks: Vec<crate::corpus::db::Track>,
        source: tag_editor::TagEditorSource,
        group_context: Option<tag_editor::GroupContext>,
    ) {
        // Start transaction
        if let Some(daemon) = self.task_daemon.as_mut() {
            let label = match source {
                tag_editor::TagEditorSource::CorpusBrowser => "Bulk tag edits",
                tag_editor::TagEditorSource::DirectoryEdit => "Directory tag edits",
                tag_editor::TagEditorSource::DuplicateResolution => "Duplicate resolution",
                tag_editor::TagEditorSource::DeployConflict => "Deploy conflict resolution",
                tag_editor::TagEditorSource::TagSearch => "Tag search edits",
            };
            let _ = daemon.start_transaction(label);
        }

        self.unified_tag_editor = Some(tag_editor::UnifiedTagEditorState::bulk_from_tracks(
            tracks,
            source,
            group_context,
        ));
        self.mode = UiMode::UnifiedTagEditor;
    }

    /// Open the unified tag editor for a directory path
    fn open_unified_tag_editor_for_directory(&mut self, directory: &std::path::Path) {
        // Query database for tracks in this directory
        let db = self.db();

        let tracks = match db.get_tracks_for_tag_editing(directory) {
            Ok(tracks) => tracks,
            Err(e) => {
                self.status_message = Some(format!("Failed to query tracks: {}", e));
                return;
            }
        };

        if tracks.is_empty() {
            self.status_message = Some(format!(
                "No indexed tracks found in {}",
                directory.display()
            ));
            return;
        }

        // Find sibling directories (other directories at the same level)
        let sibling_directories = if let Some(parent) = directory.parent() {
            std::fs::read_dir(parent)
                .ok()
                .map(|entries| {
                    let mut dirs: Vec<std::path::PathBuf> = entries
                        .filter_map(|e| e.ok())
                        .filter(|e| e.path().is_dir())
                        .map(|e| e.path())
                        .collect();
                    dirs.sort();
                    dirs
                })
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        self.tree_browser = None;

        // Start transaction for directory edits
        if let Some(daemon) = self.task_daemon.as_mut() {
            let _ = daemon.start_transaction("Directory tag edits");
        }

        // Use directory_aggregated for aggregated tag view across all files
        let mut editor = tag_editor::UnifiedTagEditorState::directory_aggregated(tracks, None);
        editor.set_sibling_directories(directory.to_path_buf(), sibling_directories);

        self.unified_tag_editor = Some(editor);
        self.mode = UiMode::UnifiedTagEditor;
    }

    fn handle_unified_tag_editor_action(&mut self, action: tag_editor::UnifiedTagEditorAction) {
        use tag_editor::UnifiedTagEditorAction;

        match action {
            UnifiedTagEditorAction::None => {}
            UnifiedTagEditorAction::CloseModal => {}

            UnifiedTagEditorAction::StageDecision { index, mutations } => {
                // User confirmed changes for this item - stage to transaction
                self.stage_decision(index, mutations);
            }

            UnifiedTagEditorAction::StageDecisionAndNext { index, mutations } => {
                // Stage the decision AND navigate to next sibling
                self.stage_decision(index, mutations);
                self.navigate_to_next_sibling();
            }

            UnifiedTagEditorAction::StageDecisionAndReview { index, mutations } => {
                // Stage the decision AND immediately show transaction review
                // Used for aggregated mode or single-item contexts where "next sibling" is meaningless
                self.stage_decision(index, mutations);

                // Immediately show transaction review modal
                let decisions = self.gather_transaction_decisions();

                if let Some(ref mut editor) = self.unified_tag_editor {
                    editor.modal = Some(tag_editor::UnifiedTagEditorModal::TransactionReview {
                        decisions,
                        scroll: 0,
                        selected_button: tag_editor::TransactionReviewButton::CommitAll,
                    });
                }
            }

            UnifiedTagEditorAction::CommitTransaction => {
                // Commit all staged decisions
                let witness = crate::daemon::confirm_decision();
                let commit_message = if let Some(daemon) = self.task_daemon.as_mut() {
                    match daemon.confirm_transaction(&witness) {
                        Ok(summary) => {
                            format!(
                                "Committed {} decisions ({} mutations)",
                                summary.decision_count,
                                summary.mutation_count
                            )
                        }
                        Err(e) => {
                            format!("Commit failed: {}", e)
                        }
                    }
                } else {
                    "No daemon available".to_string()
                };
                self.unified_tag_editor = None;
                self.start_insights_view();
                self.status_message = Some(commit_message);
            }

            UnifiedTagEditorAction::DiscardTransaction => {
                // Discard all staged decisions
                let witness = crate::daemon::confirm_decision();
                if let Some(daemon) = self.task_daemon.as_mut() {
                    let _ = daemon.discard_transaction(&witness);
                }
                self.unified_tag_editor = None;
                self.start_insights_view();
                self.status_message = Some("Edits discarded".to_string());
            }

            UnifiedTagEditorAction::NextItem => {
                if let Some(ref mut editor) = self.unified_tag_editor {
                    if editor.current_item_idx < editor.total_items.saturating_sub(1) {
                        editor.current_item_idx += 1;
                        editor.reset_field_state();
                    }
                }
            }

            UnifiedTagEditorAction::PrevItem => {
                if let Some(ref mut editor) = self.unified_tag_editor {
                    if editor.current_item_idx > 0 {
                        editor.current_item_idx -= 1;
                        editor.reset_field_state();
                    }
                }
            }

            UnifiedTagEditorAction::NextSibling => {
                self.navigate_to_next_sibling();
            }

            UnifiedTagEditorAction::PrevSibling => {
                self.navigate_to_prev_sibling();
            }

            UnifiedTagEditorAction::ShowModal(modal) => {
                if let Some(ref mut editor) = self.unified_tag_editor {
                    editor.modal = Some(modal);
                }
            }

            UnifiedTagEditorAction::StatusMessage(msg) => {
                self.status_message = Some(msg);
            }

            UnifiedTagEditorAction::RequestFillFromDb { track_id } => {
                match track_id {
                    Some(id) => {
                        let db = self.db();
                        match db.get_track_tags(id) {
                            Ok(tags) => {
                                // Convert TrackTag to (name, value) pairs
                                let tag_pairs: Vec<(String, String)> = tags
                                    .into_iter()
                                    .map(|t| (t.tag_name, t.tag_value))
                                    .collect();

                                if let Some(ref mut editor) = self.unified_tag_editor {
                                    editor.fill_from_db_result(tag_pairs);
                                }
                                self.status_message = Some("Tags loaded from database".to_string());
                            }
                            Err(e) => {
                                self.status_message = Some(format!("Error loading tags: {}", e));
                            }
                        }
                    }
                    None => {
                        self.status_message = Some("Track not indexed - no database tags available".to_string());
                    }
                }
            }

            UnifiedTagEditorAction::RequestTransactionReview => {
                // Query daemon for staged decisions and populate the review modal
                let decisions = self.gather_transaction_decisions();

                if let Some(ref mut editor) = self.unified_tag_editor {
                    editor.modal = Some(tag_editor::UnifiedTagEditorModal::TransactionReview {
                        decisions,
                        scroll: 0,
                        selected_button: tag_editor::TransactionReviewButton::CommitAll,
                    });
                }
            }
        }
    }

    /// Navigate to the next sibling in the tag editor.
    /// For DirectoryEdit mode: next sibling directory.
    /// For bulk edit mode: next track.
    fn navigate_to_next_sibling(&mut self) {
        let is_directory_edit = self.unified_tag_editor
            .as_ref()
            .map(|e| e.is_directory_edit())
            .unwrap_or(false);

        if is_directory_edit {
            // Navigate to next sibling directory
            if let Some(ref editor) = self.unified_tag_editor {
                let next_idx = editor.current_sibling_idx + 1;
                if next_idx < editor.sibling_directories.len() {
                    let next_dir = editor.sibling_directories[next_idx].clone();
                    // Re-open the tag editor for the new directory
                    self.open_unified_tag_editor_for_directory(&next_dir);
                }
            }
        } else {
            // Navigate to next track (same as NextItem)
            if let Some(ref mut editor) = self.unified_tag_editor {
                if editor.current_item_idx < editor.total_items.saturating_sub(1) {
                    editor.current_item_idx += 1;
                    editor.reset_field_state();
                }
            }
        }
    }

    /// Navigate to the previous sibling in the tag editor.
    /// For DirectoryEdit mode: previous sibling directory.
    /// For bulk edit mode: previous track.
    fn navigate_to_prev_sibling(&mut self) {
        let is_directory_edit = self.unified_tag_editor
            .as_ref()
            .map(|e| e.is_directory_edit())
            .unwrap_or(false);

        if is_directory_edit {
            // Navigate to previous sibling directory
            if let Some(ref editor) = self.unified_tag_editor {
                if editor.current_sibling_idx > 0 {
                    let prev_idx = editor.current_sibling_idx - 1;
                    let prev_dir = editor.sibling_directories[prev_idx].clone();
                    // Re-open the tag editor for the new directory
                    self.open_unified_tag_editor_for_directory(&prev_dir);
                }
            }
        } else {
            // Navigate to previous track (same as PrevItem)
            if let Some(ref mut editor) = self.unified_tag_editor {
                if editor.current_item_idx > 0 {
                    editor.current_item_idx -= 1;
                    editor.reset_field_state();
                }
            }
        }
    }

    fn handle_deployment_preview_action(&mut self, action: deploy_flow::DeploymentPreviewAction) {
        match action {
            deploy_flow::DeploymentPreviewAction::None => {}
            deploy_flow::DeploymentPreviewAction::Confirm => {
                // TODO: Reconnect when corpus::deploy is re-enabled
                // This function requires all_deployment_statuses_to_decisions from the disabled deploy module.
                self.status_message = Some("Deployment confirm disabled - deploy module being updated".to_string());
                self.deployment_preview = None;
                self.start_insights_view();
            }
            deploy_flow::DeploymentPreviewAction::Cancel => {
                let _ = config::log_message("Deployment preview cancelled");
                self.deployment_preview = None;
                self.start_insights_view();
                self.status_message = Some("Deployment cancelled".to_string());
            }
            deploy_flow::DeploymentPreviewAction::CycleNext => {
                // Deploy → TagSearch
                self.deployment_preview = None;
                self.start_tag_search();
            }
            deploy_flow::DeploymentPreviewAction::CyclePrev => {
                // Deploy → Insights
                self.deployment_preview = None;
                self.start_insights_view();
            }
        }
    }

    /// Check daemon status and update UI with any failure messages.
    /// Note: Actual daemon ticking happens in run_app via daemon().tick()
    fn check_daemon_status(&mut self) {
        if let Some(ref daemon) = self.task_daemon {
            let status = daemon.status();
            if status.failed > 0 {
                self.status_message = Some(format!(
                    "Tasks: {} done, {} failed",
                    status.completed, status.failed
                ));
            }
        }
    }

    /// Get or create the task daemon.
    fn daemon(&mut self) -> &mut crate::daemon::TaskDaemon {
        if self.task_daemon.is_none() {
            self.task_daemon = Some(crate::daemon::TaskDaemon::new());
        }
        self.task_daemon.as_mut().unwrap()
    }

    /// Shorthand for read-only database access.
    fn db(&mut self) -> &crate::corpus::db::Database {
        self.daemon().read_only_db()
    }

    /// Tick the splash screen and check for completion.
    ///
    /// Called each frame while splash_screen is Some. When daemon eye state
    /// becomes Awake (eyeballing complete), checks for unindexed files and
    /// proceeds to intake confirmation or content analysis.
    fn tick_splash_screen(&mut self) {
        // Take splash_screen temporarily to avoid borrow conflicts
        let mut splash = match self.splash_screen.take() {
            Some(s) => s,
            None => return,
        };

        // Tick splash screen - it checks daemon.eye_state() for completion
        let completed = splash.tick(self.daemon());
        if completed {
            let status = self.daemon().status();
            let _ = config::log_message(&format!(
                "Startup eyeballing complete: {} processed",
                status.total_processed
            ));
        }

        // Update stats on splash screen (for optional display)
        self.update_progress_stats(&mut splash);

        // Put it back or transition
        if splash.is_complete() {
            // Don't put it back - check for unindexed files before transitioning
            let transition_start = std::time::Instant::now();
            if let Some(intake_state) = self.check_for_unindexed_files() {
                let check_duration = transition_start.elapsed();
                let _ = config::log_message(&format!(
                    "[TRANSITION] check_for_unindexed_files took {}ms, found {} files",
                    check_duration.as_millis(),
                    intake_state.file_count
                ));
                self.intake_confirmation = Some(intake_state);
                self.mode = UiMode::IntakeConfirmation;
            } else {
                let check_duration = transition_start.elapsed();
                let _ = config::log_message(&format!(
                    "[TRANSITION] check_for_unindexed_files took {}ms, no unindexed files",
                    check_duration.as_millis()
                ));
                // No unindexed files - skip intake, proceed to content analysis
                let analysis_start = std::time::Instant::now();
                self.start_content_analysis();
                let _ = config::log_message(&format!(
                    "[TRANSITION] start_content_analysis took {}ms",
                    analysis_start.elapsed().as_millis()
                ));
            }
        } else {
            self.splash_screen = Some(splash);
        }
    }

    /// Tick content analysis progress and check for completion.
    ///
    /// Called each frame while content_analysis is Some. When all content
    /// analysis computations complete, transitions to Insights view.
    fn tick_content_analysis(&mut self) {
        // Take content_analysis temporarily to avoid borrow conflicts
        let mut progress = match self.content_analysis.take() {
            Some(p) => p,
            None => return,
        };

        // Tick progress screen - it checks daemon state for completion
        let completed = progress.tick(self.daemon());
        if completed {
            let status = self.daemon().status();
            let _ = config::log_message(&format!(
                "Metadata analysis complete: {} processed",
                status.total_processed
            ));
            // Transition to Insights view
            self.start_insights_view();
        } else {
            // Update stats on progress screen (for optional display)
            self.update_progress_stats(&mut progress);
            self.content_analysis = Some(progress);
        }
    }

    /// Tick intake confirmation while processing.
    ///
    /// Called each frame while intake_confirmation is in processing mode.
    /// When indexing completes, triggers transition to metadata analysis.
    fn tick_intake_confirmation(&mut self) {
        // Take intake_confirmation temporarily to avoid borrow conflicts
        let mut state = match self.intake_confirmation.take() {
            Some(s) => s,
            None => return,
        };

        // Tick progress - it checks daemon state for completion
        let completed = state.tick(self.daemon());
        if completed {
            let status = self.daemon().status();
            let _ = config::log_message(&format!(
                "Intake indexing complete: {} processed",
                status.total_processed
            ));
            // Handle completion via action
            self.intake_confirmation = Some(state);
            self.handle_intake_confirmation_action(startup::IntakeConfirmationAction::ProcessingComplete);
        } else {
            // Update stats for display
            self.update_progress_stats(&mut state);
            self.intake_confirmation = Some(state);
        }
    }

    /// Tick tag search - checks for pending bulk edit after modal has rendered.
    fn tick_tag_search(&mut self) {
        if let Some(ref mut search) = self.tag_search {
            if let Some(tracks) = search.take_pending_bulk_edit() {
                self.tag_search = None;
                self.start_unified_tag_editor_for_tracks(tracks);
            }
        }
    }

    /// Check for unindexed files after Awakening completes.
    ///
    /// Queries UnindexedFile signals (computed during second-level derivation).
    /// Returns Some if there are unindexed files to confirm, None otherwise.
    fn check_for_unindexed_files(&mut self) -> Option<startup::IntakeConfirmationState> {
        // Clone corpus_root to avoid borrow conflict with daemon's db reference
        let corpus_root = self.config.corpus_root.clone();

        let eye_state = self.daemon().eye_state();
        let _ = config::log_message(&format!(
            "check_for_unindexed_files: eye_state={:?}",
            eye_state
        ));

        let db = self.db();

        // Query signal count - this can be slow with many signals
        let signals_start = std::time::Instant::now();
        let signal_count = db.get_health_signals(None)
            .map(|s| s.len())
            .unwrap_or(0);
        let _ = config::log_message(&format!(
            "[TRANSITION] get_health_signals(None) took {}ms, {} signals",
            signals_start.elapsed().as_millis(),
            signal_count
        ));

        let gather_start = std::time::Instant::now();
        let result = startup::IntakeConfirmationState::gather(db, &corpus_root, "corpus");
        let _ = config::log_message(&format!(
            "[TRANSITION] IntakeConfirmationState::gather took {}ms",
            gather_start.elapsed().as_millis()
        ));

        result
    }
}

// ============================================================================
// Rendering
// ============================================================================

fn render(f: &mut Frame, app: &mut App) {
    // Use cached corpus summary to avoid DB queries every frame.
    // The cache is refreshed in the event loop via ui_cache.refresh().
    let corpus_summary = app.ui_cache.corpus_summary();

    // Fetch DB thread stats (cheap - just reads cached atomic values)
    // Returns None if timing instrumentation is disabled
    let db_stats = app.task_daemon.as_ref().and_then(|d| d.db_stats());

    // Get daemon status WITHOUT ticking again - status() just reads current state
    let daemon_status = app.task_daemon.as_ref().and_then(|d| {
        let status = d.status();
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
        splash_screen: app.splash_screen.as_ref(),
        content_analysis: app.content_analysis.as_ref(),
        insights_view: app.insights_view.as_mut(),
        tag_search: app.tag_search.as_ref(),
        intake_confirmation: app.intake_confirmation.as_ref(),
        unified_tag_editor: app.unified_tag_editor.as_mut(),
        eye: &app.eye,
        throughput_samples: &app.throughput_samples,
        daemon_status,
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

    // Eyeballing ALWAYS runs at startup (only paranoid mode is configurable)
    // Create splash screen and queue initial eyeballing via daemon
    let splash = splash_screen::SplashScreen::new();
    let corpus_root = app.config.corpus_root.clone();
    let legacy_library = app.config.legacy_library.clone();
    let paranoid = app.config.opinions.startup.paranoid_tag_verification;

    // Start eyeballing via daemon - this sets observation_state and queues work
    if paranoid {
        app.daemon().start_paranoid_eyeball(&corpus_root, legacy_library.as_deref());
    } else {
        app.daemon().start_lazy_eyeball(&corpus_root, legacy_library.as_deref());
    }

    app.splash_screen = Some(splash);
    app.mode = UiMode::LoadingSplash;

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

        // Tick daemon first - she is the driving system
        let tick_start = std::time::Instant::now();
        let tick_status = app.daemon().tick();
        let tick_duration = tick_start.elapsed();
        if tick_duration.as_millis() > 16 {
            let _ = config::log_message(&format!(
                "[FRAME DEBUG] daemon.tick() took {}ms, drained {} results",
                tick_duration.as_millis(),
                tick_status.total_processed
            ));
        }

        app.check_daemon_status();

        // Update eye animation - only animate when daemon eye is Awake
        let can_animate = app.daemon().eye_state() == crate::daemon::EyeState::Awake;
        app.eye.update(can_animate);

        // Update insights view with daemon status
        if let Some(ref mut view) = app.insights_view {
            let status = app.task_daemon.as_ref().map(|d| d.status());
            view.update(status.as_ref());
        }

        // Tick splash screen if active (startup eyeballing)
        if app.splash_screen.is_some() {
            app.tick_splash_screen();
        }

        // Tick intake confirmation if processing
        if app.intake_confirmation.as_ref().map(|s| s.is_processing()).unwrap_or(false) {
            app.tick_intake_confirmation();
        }

        // Tick content analysis if active (post-intake computation)
        if app.content_analysis.is_some() {
            app.tick_content_analysis();
        }

        // Refresh UI cache periodically (avoids per-frame DB queries in render code)
        if let Some(ref mut daemon) = app.task_daemon {
            app.ui_cache.refresh(daemon);
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
