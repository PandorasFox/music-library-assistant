//! TUI Module
//!
//! Modular UI components for the Music Library Assistant.

pub mod app;
pub mod deploy_flow;
pub mod drop_flow;
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
use std::time::Instant;

use crate::config::{self, Config};
use crate::corpus::db::{Database, HealthIssueType};
use crate::flows::background::{log_task_summary, poll_tasks, BackgroundTask};

use app::EyeAnimation;

use crate::corpus::mutations::MigrationRegistry;

// ============================================================================
// Deploy Conflict Resolution
// ============================================================================

/// Load deployment conflict groups from health issues.
///
/// Queries for unresolved `DeployConflict` health issues and converts them to
/// `DuplicateGroupInfo` format for the tag editor.
fn load_deploy_conflict_groups(db: &Database) -> Result<Vec<tag_editor::DuplicateGroupInfo>> {
    let issues = db.get_unresolved_health_issues(Some(HealthIssueType::DeployConflict))?;

    let mut groups = Vec::with_capacity(issues.len());

    for issue in issues {
        let issue_id = match issue.id {
            Some(id) => id,
            None => continue,
        };

        // Get tracks associated with this conflict
        let track_roles = db.get_health_issue_tracks(issue_id)?;
        let tracks: Vec<_> = track_roles.into_iter().map(|(track, _role)| track).collect();

        // Skip if fewer than 2 tracks (not really a conflict)
        if tracks.len() < 2 {
            continue;
        }

        groups.push(tag_editor::DuplicateGroupInfo {
            group_id: issue_id,
            tracks,
            resolved: false,
        });
    }

    Ok(groups)
}

// ============================================================================
// Health Data Version
// ============================================================================

/// Current health data schema version.
/// Increment this when health detection algorithms change significantly.
/// On startup, if DB version differs from this, health index is rebuilt.
pub const HEALTH_DATA_VERSION: u32 = 1;

// ============================================================================
// Application State
// ============================================================================

/// Current UI mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum UiMode {
    TagEditor,
    DirBrowser,
    DropMissingConfirmation,
    DeploymentPreview,
    /// Exit confirmation modal (when operations are in progress)
    ExitConfirmModal,
    /// Corpus browser with directory tree and metadata preview
    CorpusBrowser,
    /// Deploy conflict resolution - review accumulated changes before commit
    DeployConflictReview,
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

/// Re-export from drop_flow module
pub(crate) use drop_flow::DropMissingState;

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

/// Accumulated changes for a single deploy conflict group
#[derive(Debug, Clone)]
pub(crate) struct DeployConflictGroupChanges {
    /// Health issue ID for this conflict group
    pub group_id: i64,
    /// Target deployment path this conflict was about
    pub target_path: String,
    /// Track count in this group
    pub track_count: usize,
    /// Pending decisions for this group
    pub decisions: Vec<crate::flows::PendingDecision>,
}

/// State for the deploy conflict review screen
#[derive(Debug, Clone)]
pub(crate) struct DeployConflictReviewState {
    /// Accumulated changes from all processed groups
    pub groups: Vec<DeployConflictGroupChanges>,
    /// Scroll offset for the list
    pub scroll_offset: usize,
    /// Selected button: 0 = Commit, 1 = Discard
    pub selected_button: usize,
}

/// Main application state
pub(crate) struct App {
    config: Config,
    should_quit: bool,
    status_message: Option<String>,

    // UI mode and state
    mode: UiMode,
    tag_editor: Option<tag_editor::TagEditorState>,
    tag_editor_modal: Option<tag_editor::TagEditorModal>,
    tree_browser: Option<tree_browser::TreeBrowserState>,
    drop_missing_state: Option<DropMissingState>,
    deployment_preview: Option<deploy_flow::DeploymentPreviewState>,
    // Unified tag editor (transaction-based)
    unified_tag_editor: Option<tag_editor::UnifiedTagEditorState>,
    // Exit confirmation modal
    exit_confirm_modal_state: Option<ExitConfirmModalState>,
    // Startup splash screen
    splash_screen: Option<splash_screen::SplashScreen>,
    // Deploy conflict resolution flow
    deploy_conflict_review: Option<DeployConflictReviewState>,
    deploy_conflict_accumulated: Vec<DeployConflictGroupChanges>,
    // Insights view (lateral view ring)
    insights_view: Option<insights_view::InsightsViewState>,
    // Tag search (lateral view ring)
    tag_search: Option<tag_search::TagSearchState>,
    // Intake confirmation modal
    intake_confirmation: Option<startup::IntakeConfirmationState>,

    // Background tasks (supports multiple concurrent)
    pub background_tasks: Vec<BackgroundTask>,

    // Task daemon for mutation execution
    task_daemon: Option<crate::flows::TaskDaemon>,

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
            tag_editor: None,
            tag_editor_modal: None,
            tree_browser: None,
            drop_missing_state: None,
            deployment_preview: None,
            unified_tag_editor: None,
            exit_confirm_modal_state: None,
            splash_screen: None,
            deploy_conflict_review: None,
            deploy_conflict_accumulated: Vec::new(),
            insights_view: None,
            tag_search: None,
            intake_confirmation: None,
            background_tasks: Vec::new(),
            task_daemon: None,
            throughput_samples: VecDeque::with_capacity(100),
            eye: EyeAnimation::default(),
        }
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        match self.mode {
            UiMode::TagEditor => {
                // If there's a modal active, handle modal keys first
                if self.tag_editor_modal.is_some() {
                    self.handle_tag_editor_modal_key(key);
                } else if let Some(ref mut editor) = self.tag_editor {
                    let action = editor.handle_key(key);
                    self.handle_tag_editor_action(action);
                }
            }
            UiMode::DirBrowser => {
                if let Some(ref mut browser) = self.tree_browser {
                    let action = browser.handle_key(key);
                    self.handle_tree_browser_action(action);
                }
            }
            UiMode::DropMissingConfirmation => {
                self.handle_drop_missing_key(key);
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
            UiMode::DeployConflictReview => {
                self.handle_deploy_conflict_review_key(key);
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
                    let db = self.daemon().read_only_db();
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
                // User confirmed - create IndexTrack mutations
                if let Some(ref state) = self.intake_confirmation {
                    let mutations = state.create_index_mutations();
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

                    self.status_message = Some(format!("Indexing {} files...", count));
                }

                // Clean up and transition to Insights
                self.intake_confirmation = None;
                self.start_insights_view();
            }
            startup::IntakeConfirmationAction::Skipped => {
                // User skipped - proceed to Insights without indexing
                // MissingFromIndex signals remain in health_issues for later handling
                let _ = config::log_message("IntakeConfirmation: user skipped indexing");

                self.intake_confirmation = None;
                self.start_insights_view();
            }
        }
    }

    /// Check if there are any pending operations (daemon work, background tasks, etc.)
    fn has_pending_operations(&self) -> bool {
        let daemon_busy = self.task_daemon.as_ref().map(|d| d.has_pending()).unwrap_or(false);
        daemon_busy || !self.background_tasks.is_empty()
    }

    /// Start the insights view.
    ///
    /// Initializes the view with one-dim insights computed immediately,
    /// spawns background computation for multi-dim insights.
    fn start_insights_view(&mut self) {
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                self.status_message = Some(format!("Config error: {}", e));
                return;
            }
        };

        let db = self.daemon().read_only_db();

        // Initialize insights view (queries health_issues table directly)
        let mut view = insights_view::InsightsViewState::new();
        view.initialize(db, db_path.to_str().unwrap_or(""));

        self.insights_view = Some(view);
        self.mode = UiMode::Insights;
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
        // Compute deployment status synchronously (could be made async for large libraries)
        // Clone config to avoid borrow conflict with daemon's db reference
        let config = self.config.clone();
        let db = self.daemon().read_only_db();

        let _ = config::log_message("Computing deployment status...");
        match crate::flows::deploy::compute_full_deployment_status(&config, db) {
            Ok(statuses) => {
                let session_id = uuid::Uuid::new_v4().to_string();
                let total_mutations: usize = statuses
                    .iter()
                    .map(|s| s.to_deploy.len() + s.stale.len() + s.orphans.len())
                    .sum();

                let _ = config::log_message(&format!(
                    "Computed deployment status: {} libraries, {} total mutations",
                    statuses.len(),
                    total_mutations
                ));

                self.deployment_preview =
                    Some(deploy_flow::DeploymentPreviewState::new(statuses, session_id));
                self.mode = UiMode::DeploymentPreview;
            }
            Err(e) => {
                let _ = config::log_message(&format!("ERROR: Failed to compute deployment status: {}", e));
                self.status_message = Some(format!("Deployment status error: {}", e));
            }
        }
    }

    fn rebuild_health_index(&mut self) {
        // TODO: Signals now have stronger guarantees - this is vestigial
        self.status_message = Some("Health rebuild no longer needed - signals are authoritative".to_string());
    }

    fn start_drop_missing_confirmation(&mut self) {
        let db = self.daemon().read_only_db();
        match drop_flow::find_missing_tracks(db) {
            Ok(missing) => {
                if missing.is_empty() {
                    self.status_message = Some("No missing files found in index".to_string());
                } else {
                    self.drop_missing_state = Some(DropMissingState::new(missing));
                    self.mode = UiMode::DropMissingConfirmation;
                }
            }
            Err(e) => {
                self.status_message = Some(e);
            }
        }
    }

    fn handle_drop_missing_key(&mut self, key: crossterm::event::KeyEvent) {
        if let Some(ref mut state) = self.drop_missing_state {
            let action = state.handle_key(key);
            match action {
                drop_flow::DropMissingAction::None => {}
                drop_flow::DropMissingAction::Execute => {
                    self.execute_drop_missing();
                }
                drop_flow::DropMissingAction::Cancel => {
                    self.drop_missing_state = None;
                    self.start_insights_view();
                    self.status_message = Some("Drop cancelled".to_string());
                }
            }
        }
    }

    fn execute_drop_missing(&mut self) {
        // Get the missing tracks from state
        let missing_tracks = match &self.drop_missing_state {
            Some(state) => state.missing_tracks.clone(),
            None => {
                self.start_insights_view();
                self.status_message = Some("No missing tracks to drop".to_string());
                return;
            }
        };

        let db = self.daemon().read_only_db();
        match drop_flow::execute_drop_missing(db, &missing_tracks) {
            Ok(result) => {
                let mut msg = if let Some(log_path) = result.log_path {
                    format!("Dropped {} entries from index. Log: {}", result.dropped_count, log_path)
                } else {
                    format!("Dropped {} entries from index", result.dropped_count)
                };

                if result.orphans_cleaned > 0 {
                    msg = format!("{} (also cleaned {} orphaned scan entries)", msg, result.orphans_cleaned);
                }

                if result.failed_count > 0 {
                    msg = format!("{}. {} failed: {:?}", msg, result.failed_count, result.errors);
                }

                self.status_message = Some(msg);
            }
            Err(e) => {
                self.status_message = Some(e);
            }
        }

        self.drop_missing_state = None;
        self.start_insights_view();
    }

    /// Start tag editor for deploy conflict resolution.
    ///
    /// Loads deployment conflicts from health_issues table and presents them
    /// in the tag editor for resolution (editing tags to create unique deploy paths).
    fn start_tag_editor(&mut self) {
        let db = self.daemon().read_only_db();

        // Load deploy conflict health issues
        let all_groups = match load_deploy_conflict_groups(db) {
            Ok(groups) => groups,
            Err(e) => {
                self.status_message = Some(format!("Error loading conflicts: {}", e));
                return;
            }
        };

        if all_groups.is_empty() {
            self.status_message = Some("No deployment conflicts to resolve".to_string());
            return;
        }

        // Start with first group's tracks
        let first_tracks = all_groups[0].tracks.clone();
        self.tag_editor = Some(tag_editor::TagEditorState::new(first_tracks, all_groups));
        self.mode = UiMode::TagEditor;
    }

    fn start_corpus_browser(&mut self) {
        let config = tree_browser::CorpusBrowserConfig::default();
        self.tree_browser = Some(tree_browser::TreeBrowserState::corpus_browser(
            self.config.corpus_root.clone(),
            config,
        ));
        self.mode = UiMode::CorpusBrowser;
    }

    fn start_directory_selector(&mut self, title: &str) {
        let config = tree_browser::DirectorySelectorConfig::for_sleuthing();
        self.tree_browser = Some(tree_browser::TreeBrowserState::directory_selector(
            self.config.corpus_root.clone(),
            title,
            config,
        ));
        self.mode = UiMode::DirBrowser;
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
        let db = self.daemon().read_only_db();

        // Load tracks from database
        let (tracks, selected_idx) = if recursive {
            // Get all tracks in directory and subdirectories (no fingerprint filter)
            match db.get_tracks_for_tag_editing(path) {
                Ok(t) => (t, 0usize),
                Err(e) => {
                    self.status_message = Some(format!(
                        "Query error for path '{}': {}",
                        path.display(),
                        e
                    ));
                    self.tree_browser = None;
                    self.start_insights_view();
                    return;
                }
            }
        } else {
            // Get all tracks in the same directory for cycling with tab/shift-tab
            let parent_dir = match path.parent() {
                Some(p) => p,
                None => {
                    self.status_message = Some(format!(
                        "Cannot determine parent directory: {}",
                        path.display()
                    ));
                    self.tree_browser = None;
                    self.start_insights_view();
                    return;
                }
            };

            // Load all tracks from parent directory (non-recursive, just this folder)
            let dir_tracks = match db.get_tracks_for_tag_editing(parent_dir) {
                Ok(t) => t,
                Err(e) => {
                    self.status_message = Some(format!(
                        "Query error for directory '{}': {}",
                        parent_dir.display(),
                        e
                    ));
                    self.tree_browser = None;
                    self.start_insights_view();
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
                        self.status_message = Some(format!(
                            "Track not in index: {}",
                            path.display()
                        ));
                        self.tree_browser = None;
                        self.start_insights_view();
                        return;
                    }
                    Err(e) => {
                        self.status_message = Some(format!(
                            "Query error for '{}': {}",
                            path.display(),
                            e
                        ));
                        self.tree_browser = None;
                        self.start_insights_view();
                        return;
                    }
                }
            } else {
                (tracks_in_dir, selected_idx)
            }
        };

        if tracks.is_empty() {
            self.status_message = Some(format!(
                "No indexed tracks at: {}",
                path.display()
            ));
            self.tree_browser = None;
            self.start_insights_view();
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


    fn handle_tag_editor_action(&mut self, action: tag_editor::TagEditorAction) {
        match action {
            tag_editor::TagEditorAction::None => {}
            tag_editor::TagEditorAction::Exit => {
                self.tag_editor = None;
                self.start_insights_view();
            }
            tag_editor::TagEditorAction::SaveAll => {
                self.save_tag_editor_changes(false);
            }
            tag_editor::TagEditorAction::SaveAndNext => {
                self.save_tag_editor_changes(true);
            }
            tag_editor::TagEditorAction::ShowModal(modal) => {
                self.tag_editor_modal = Some(modal);
            }
            tag_editor::TagEditorAction::StatusMessage(msg) => {
                self.status_message = Some(msg);
            }
        }
    }

    fn handle_tag_editor_modal_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        let modal = match self.tag_editor_modal.take() {
            Some(m) => m,
            None => return,
        };

        match modal {
            tag_editor::TagEditorModal::SaveConfirmation { mut selected_button } => {
                match key.code {
                    KeyCode::Left => {
                        selected_button = selected_button.saturating_sub(1);
                        self.tag_editor_modal =
                            Some(tag_editor::TagEditorModal::SaveConfirmation { selected_button });
                    }
                    KeyCode::Right => {
                        selected_button = (selected_button + 1).min(2);
                        self.tag_editor_modal =
                            Some(tag_editor::TagEditorModal::SaveConfirmation { selected_button });
                    }
                    KeyCode::Enter => {
                        match selected_button {
                            0 => {
                                // Save All
                                self.save_tag_editor_changes(false);
                            }
                            1 => {
                                // Save & Next
                                self.save_tag_editor_changes(true);
                            }
                            2 => {
                                // Return to editor
                                self.tag_editor_modal = None;
                            }
                            _ => {}
                        }
                    }
                    KeyCode::Esc => {
                        // Cancel modal, return to editor
                        self.tag_editor_modal = None;
                    }
                    _ => {
                        // Put modal back
                        self.tag_editor_modal =
                            Some(tag_editor::TagEditorModal::SaveConfirmation { selected_button });
                    }
                }
            }
            tag_editor::TagEditorModal::ChangePreview {
                grouped_changes,
                single_changes,
                mut scroll_offset,
                save_and_next,
            } => {
                match key.code {
                    KeyCode::Up => {
                        scroll_offset = scroll_offset.saturating_sub(1);
                        self.tag_editor_modal = Some(tag_editor::TagEditorModal::ChangePreview {
                            grouped_changes,
                            single_changes,
                            scroll_offset,
                            save_and_next,
                        });
                    }
                    KeyCode::Down => {
                        scroll_offset += 1;
                        self.tag_editor_modal = Some(tag_editor::TagEditorModal::ChangePreview {
                            grouped_changes,
                            single_changes,
                            scroll_offset,
                            save_and_next,
                        });
                    }
                    KeyCode::Enter => {
                        // Proceed with save
                        self.save_tag_editor_changes(save_and_next);
                    }
                    KeyCode::Esc => {
                        // Cancel modal
                        self.tag_editor_modal = None;
                    }
                    _ => {
                        self.tag_editor_modal = Some(tag_editor::TagEditorModal::ChangePreview {
                            grouped_changes,
                            single_changes,
                            scroll_offset,
                            save_and_next,
                        });
                    }
                }
            }
        }
    }

    fn save_tag_editor_changes(&mut self, advance_to_next: bool) {
        use std::collections::HashMap;
        use crate::flows::PendingDecision;

        // Step 1: Compute changes from tag editor
        let Some(ref editor) = self.tag_editor else {
            self.status_message = Some("No tag editor state".to_string());
            return;
        };

        let changes = tag_editor::state::compute_changes(
            &editor.original_tag_fields,
            &editor.tag_fields,
        );

        // Check if we're in a deploy conflict workflow (accumulating mode)
        let in_workflow = editor.is_in_duplicate_workflow();

        // For workflow mode with advance_to_next: allow proceeding even with no changes
        // (user might just want to skip a group without editing)
        if changes.is_empty() && !advance_to_next {
            self.tag_editor = None;
            self.start_insights_view();
            self.status_message = Some("No changes to save".to_string());
            return;
        }

        // Step 2: Group changes by track index
        let mut changes_by_track: HashMap<usize, Vec<(String, String)>> = HashMap::new();
        for change in &changes {
            changes_by_track
                .entry(change.track_idx)
                .or_default()
                .push((change.field_name.clone(), change.new_value.clone()));
        }

        // Step 3: Create PendingDecision records for each track
        let mut pending_decisions: Vec<PendingDecision> = Vec::new();

        for (track_idx, track_changes) in &changes_by_track {
            if let Some(track) = editor.tracks.get(*track_idx) {
                let track_id = track.id.unwrap_or(0);

                // Build metadata JSON: { "tags": [["field", "value"], ...], "track_id": N }
                let tags_json: Vec<serde_json::Value> = track_changes
                    .iter()
                    .map(|(k, v)| serde_json::json!([k, v]))
                    .collect();

                let metadata = serde_json::json!({
                    "tags": tags_json,
                    "track_id": track_id,
                });

                pending_decisions.push(PendingDecision::tag_edit(
                    &track.path,
                    metadata,
                ));
            }
        }

        // Step 4: Handle differently based on workflow mode
        if in_workflow && advance_to_next {
            // ACCUMULATING MODE: Store changes for later bulk execution
            // Extract needed data from immutable borrow first
            let current_idx = editor.current_group_idx.unwrap_or(0);
            let total_groups = editor.duplicate_groups.len();

            // Build group info for accumulation
            let group_info = editor.duplicate_groups.get(current_idx).map(|group| {
                // Use path for display since tags are stored separately
                let target_path = group.tracks.first()
                    .map(|t| {
                        std::path::Path::new(&t.path)
                            .file_name()
                            .and_then(|f| f.to_str())
                            .unwrap_or("Unknown")
                            .to_string()
                    })
                    .unwrap_or_else(|| "Unknown".to_string());
                (group.group_id, target_path, group.tracks.len())
            });

            // Drop immutable borrow by ending the else block scope
            // Now we can mutate
            if let Some((group_id, target_path, track_count)) = group_info {
                self.deploy_conflict_accumulated.push(DeployConflictGroupChanges {
                    group_id,
                    target_path,
                    track_count,
                    decisions: pending_decisions,
                });
            }

            // Advance to next group
            let next_idx = current_idx + 1;
            if next_idx < total_groups {
                // Move to next group
                if let Some(ref mut editor) = self.tag_editor {
                    editor.current_group_idx = Some(next_idx);
                    let group = &editor.duplicate_groups[next_idx];
                    editor.tracks = group.tracks.clone();
                    editor.current_track_idx = 0;
                    editor.current_field_idx = 0;
                    // Reload tag fields from disk
                    editor.tag_fields = editor.tracks.iter()
                        .map(tag_editor::state::track_to_tag_fields)
                        .collect();
                    editor.original_tag_fields = editor.tag_fields.clone();
                }

                let groups_remaining = total_groups - next_idx;
                self.status_message = Some(format!(
                    "Group {} decisions saved. {} group(s) remaining.",
                    current_idx + 1,
                    groups_remaining
                ));
            } else {
                // No more groups - transition to review screen
                self.tag_editor = None;
                self.deploy_conflict_review = Some(DeployConflictReviewState {
                    groups: self.deploy_conflict_accumulated.clone(),
                    scroll_offset: 0,
                    selected_button: 0, // Default to Commit
                });
                self.mode = UiMode::DeployConflictReview;
                self.status_message = Some(format!(
                    "All {} groups processed. Review and commit changes.",
                    self.deploy_conflict_accumulated.len()
                ));
            }
            return;
        }

        // IMMEDIATE MODE: Queue changes to daemon for async execution
        use crate::corpus::mutations::{Mutation, TagEdit};
        use std::path::PathBuf;

        // Convert to Mutations and queue to daemon
        let mutations: Vec<Mutation> = changes_by_track
            .iter()
            .filter_map(|(track_idx, track_changes)| {
                let track = editor.tracks.get(*track_idx)?;
                let track_id = track.id.unwrap_or(0);
                let edits: Vec<TagEdit> = track_changes
                    .iter()
                    .map(|(field, value)| TagEdit {
                        tag_name: field.clone(),
                        old_value: None, // We don't track old value here
                        new_value: Some(value.clone()),
                    })
                    .collect();
                Some(Mutation::TagEditAndFlush {
                    track_id,
                    path: PathBuf::from(&track.path),
                    edits,
                })
            })
            .collect();

        let _mutation_count = mutations.len();

        // TODO: Reconnect via DecisionWitness when UI integration is complete.
        // Tag editor saves are user-led decisions and need:
        //   let witness = crate::daemon::confirm_decision();
        //   self.daemon().queue_all_with_label(mutations, Some("Tag edits".to_string()), &witness);
        let _ = mutations;
        todo!("Reconnect: Tag editor save needs DecisionWitness");

        // Status message - execution is now async
        #[allow(unreachable_code)]
        { self.status_message = Some(format!("Queued {} tag edit(s)", _mutation_count)); }

        if advance_to_next {
            // Move to next duplicate group if in duplicate workflow
            if let Some(ref mut editor) = self.tag_editor {
                if editor.current_group_idx.is_some() {
                    let current_idx = editor.current_group_idx.unwrap();
                    editor.current_group_idx = Some(current_idx + 1);

                    // Load the next group if one exists
                    let next_idx = editor.current_group_idx.unwrap();
                    if next_idx < editor.duplicate_groups.len() {
                        let group = &editor.duplicate_groups[next_idx];
                        editor.tracks = group.tracks.clone();
                        editor.current_track_idx = 0;
                        editor.current_field_idx = 0;
                        // Reload tag fields from disk
                        editor.tag_fields = editor.tracks.iter()
                            .map(tag_editor::state::track_to_tag_fields)
                            .collect();
                        editor.original_tag_fields = editor.tag_fields.clone();
                    } else {
                        // No more groups
                        self.tag_editor = None;
                        self.start_insights_view();
                        self.status_message = Some("All conflicts processed.".to_string());
                    }
                }
            }
        } else {
            self.tag_editor = None;
            self.start_insights_view();
        }
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
        let db = self.daemon().read_only_db();

        let tracks = match db.get_tracks_in_directory(directory) {
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

            UnifiedTagEditorAction::StageDecisionAndNext { index, mutations } => {
                // Stage the decision AND navigate to next sibling
                let witness = crate::daemon::confirm_decision();
                if let Some(daemon) = self.task_daemon.as_mut() {
                    let label = self.unified_tag_editor
                        .as_ref()
                        .map(|e| e.current_item_label())
                        .unwrap_or_else(|| "Tag edit".to_string());
                    let _ = daemon.add_decision(index, &witness, label, mutations.clone());
                }
                // Track staged mutations
                if let Some(ref mut editor) = self.unified_tag_editor {
                    editor.set_staged_mutations(mutations);
                }
                self.status_message = Some(format!("Decision staged (item {})", index + 1));
                // Now navigate to next sibling
                self.navigate_to_next_sibling();
            }

            UnifiedTagEditorAction::StageDecisionAndReview { index, mutations } => {
                // Stage the decision AND immediately show transaction review
                // Used for aggregated mode or single-item contexts where "next sibling" is meaningless
                let witness = crate::daemon::confirm_decision();
                if let Some(daemon) = self.task_daemon.as_mut() {
                    let label = self.unified_tag_editor
                        .as_ref()
                        .map(|e| e.current_item_label())
                        .unwrap_or_else(|| "Tag edit".to_string());
                    let _ = daemon.add_decision(index, &witness, label, mutations.clone());
                }
                // Track staged mutations
                if let Some(ref mut editor) = self.unified_tag_editor {
                    editor.set_staged_mutations(mutations);
                }
                self.status_message = Some(format!("Decision staged (item {})", index + 1));

                // Immediately show transaction review modal
                let decisions: Vec<(usize, String, usize)> = if let Some(daemon) = self.task_daemon.as_ref() {
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
                };

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
                        editor.current_field_idx = 0;
                        editor.field_scroll_offset = 0;
                        editor.clear_staged_mutations();
                    }
                }
            }

            UnifiedTagEditorAction::PrevItem => {
                if let Some(ref mut editor) = self.unified_tag_editor {
                    if editor.current_item_idx > 0 {
                        editor.current_item_idx -= 1;
                        editor.current_field_idx = 0;
                        editor.field_scroll_offset = 0;
                        editor.clear_staged_mutations();
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
                        let db = self.daemon().read_only_db();
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
                let decisions: Vec<(usize, String, usize)> = if let Some(daemon) = self.task_daemon.as_ref() {
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
                };

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
                    editor.current_field_idx = 0;
                    editor.field_scroll_offset = 0;
                    editor.clear_staged_mutations();
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
                    editor.current_field_idx = 0;
                    editor.field_scroll_offset = 0;
                    editor.clear_staged_mutations();
                }
            }
        }
    }

    fn handle_deployment_preview_action(&mut self, action: deploy_flow::DeploymentPreviewAction) {
        match action {
            deploy_flow::DeploymentPreviewAction::None => {}
            deploy_flow::DeploymentPreviewAction::Confirm => {
                // Generate mutations from deployment status and spawn background execution
                if let Some(ref preview) = self.deployment_preview {
                    let _ = config::log_message("=== DEPLOYMENT PREVIEW: CONFIRM REQUESTED ===");

                    // Generate decisions for all libraries
                    let all_decisions = crate::flows::deploy::all_deployment_statuses_to_decisions(
                        &preview.statuses,
                        &self.config,
                    );

                    let _ = config::log_message(&format!(
                        "Generated {} deployment decisions",
                        all_decisions.len()
                    ));

                    if all_decisions.is_empty() {
                        self.deployment_preview = None;
                        self.start_insights_view();
                        self.status_message = Some("No deployment changes needed".to_string());
                        return;
                    }

                    // Get library names for status message
                    let library_names: Vec<String> = preview.statuses
                        .iter()
                        .map(|s| s.library_name.clone())
                        .collect();
                    let library_display = if library_names.len() == 1 {
                        library_names[0].clone()
                    } else {
                        format!("{} libraries", library_names.len())
                    };

                    let _decision_count = all_decisions.len();

                    // Convert decisions to mutations and queue to daemon
                    use crate::corpus::mutations::Mutation;
                    use crate::flows::DecisionType;
                    use std::path::PathBuf;

                    let mutations: Vec<Mutation> = all_decisions
                        .iter()
                        .filter_map(|d| {
                            match d.decision_type {
                                DecisionType::Deploy => {
                                    let target = d.target_path.as_ref()?;
                                    Some(Mutation::HardLink {
                                        source: PathBuf::from(&d.source_path),
                                        destination: PathBuf::from(target),
                                    })
                                }
                                DecisionType::Undeploy => {
                                    Some(Mutation::Unlink {
                                        path: PathBuf::from(&d.source_path),
                                    })
                                }
                                DecisionType::Redeploy => {
                                    // Redeploy = remove old + create new
                                    // For now, just create the new link (old will be orphaned)
                                    let target = d.target_path.as_ref()?;
                                    Some(Mutation::HardLink {
                                        source: PathBuf::from(&d.source_path),
                                        destination: PathBuf::from(target),
                                    })
                                }
                                _ => None, // Other decision types not handled here
                            }
                        })
                        .collect();

                    let _mutation_count = mutations.len();
                    let label = format!("Deploy to {}", library_display);

                    // TODO: Reconnect via DecisionWitness when UI integration is complete.
                    // Deployment confirms are user-led decisions and need:
                    //   let witness = crate::daemon::confirm_decision();
                    //   self.daemon().queue_all_with_label(mutations, Some(label), &witness);
                    let _ = (mutations, label);
                    todo!("Reconnect: Deployment confirm needs DecisionWitness");

                    #[allow(unreachable_code)]
                    { self.status_message = Some(format!(
                        "Queued {} deployment operations to {}",
                        _mutation_count, library_display
                    )); }
                }
                // Return to main menu immediately - deployment runs in background
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

    fn update_operation_progress(&mut self) {
        // Poll all tracked background tasks (legacy)
        let completed = poll_tasks(&mut self.background_tasks);
        for (id, label, result) in completed {
            // Log task summary
            log_task_summary(&label, &result);

            // Format completion message
            self.status_message = Some(if result.succeeded == 0 && result.skipped > 0 {
                format!(
                    "[{}] Complete! {} skipped (unchanged) in {:.1}s",
                    id,
                    result.skipped,
                    result.duration.as_secs_f64()
                )
            } else {
                let bytes_str = result
                    .bytes_processed
                    .map(|b| format!(" ({:.2} GB)", b as f64 / 1_000_000_000.0))
                    .unwrap_or_default();
                format!(
                    "[{}] Complete! {} succeeded{} in {:.1}s",
                    id,
                    result.succeeded,
                    bytes_str,
                    result.duration.as_secs_f64()
                )
            });
        }

        // Tick task daemon (advance internal processing)
        if let Some(ref mut daemon) = self.task_daemon {
            let status = daemon.tick();
            if status.failed > 0 {
                self.status_message = Some(format!(
                    "Tasks: {} done, {} failed",
                    status.completed, status.failed
                ));
            }
        }
    }

    /// Get or create the task daemon.
    fn daemon(&mut self) -> &mut crate::flows::TaskDaemon {
        if self.task_daemon.is_none() {
            self.task_daemon = Some(crate::flows::TaskDaemon::new());
        }
        self.task_daemon.as_mut().unwrap()
    }


    /// Tick the splash screen and check for completion.
    ///
    /// Called each frame while splash_screen is Some. When daemon eye state
    /// becomes Awake (eyeballing complete), transitions to Insights view.
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

        // Put it back or transition
        if splash.is_complete() {
            // Don't put it back - check for unindexed files before transitioning
            if let Some(intake_state) = self.check_for_unindexed_files() {
                let _ = config::log_message(&format!(
                    "IntakeConfirmation: showing prompt for {} files",
                    intake_state.file_count
                ));
                self.intake_confirmation = Some(intake_state);
                self.mode = UiMode::IntakeConfirmation;
            } else {
                self.start_insights_view();
            }
        } else {
            self.splash_screen = Some(splash);
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

    /// Check for unindexed files after eyeballing completes.
    ///
    /// Queries MissingFromIndex health issues and gathers file information.
    /// Returns Some if there are unindexed files to confirm, None otherwise.
    fn check_for_unindexed_files(&mut self) -> Option<startup::IntakeConfirmationState> {
        // Clone corpus_root to avoid borrow conflict with daemon's db reference
        let corpus_root = self.config.corpus_root.clone();
        let db = self.daemon().read_only_db();
        startup::IntakeConfirmationState::gather(db, &corpus_root, "corpus")
    }

    // =========================================================================
    // Deploy Conflict Review Handlers
    // =========================================================================

    fn handle_deploy_conflict_review_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        let review = match &mut self.deploy_conflict_review {
            Some(r) => r,
            None => return,
        };

        match key.code {
            KeyCode::Left => {
                review.selected_button = 0; // Commit
            }
            KeyCode::Right => {
                review.selected_button = 1; // Discard
            }
            KeyCode::Enter => {
                if review.selected_button == 0 {
                    // Commit all accumulated changes
                    self.commit_deploy_conflict_changes();
                } else {
                    // Discard - just clear state and return to menu
                    self.discard_deploy_conflict_changes();
                }
            }
            KeyCode::Esc => {
                // Cancel - return to menu without committing
                self.discard_deploy_conflict_changes();
            }
            _ => {}
        }
    }

    fn commit_deploy_conflict_changes(&mut self) {
        // NOTE: This function uses vestigial execute_decisions which always fails.
        // Needs refactoring to use daemon's transaction API (queue_mutations).

        // Gather all pending decisions from accumulated groups (before db borrow)
        let all_decisions: Vec<_> = self.deploy_conflict_accumulated
            .iter()
            .flat_map(|g| g.decisions.clone())
            .collect();

        if all_decisions.is_empty() {
            self.deploy_conflict_review = None;
            self.deploy_conflict_accumulated.clear();
            self.start_insights_view();
            self.status_message = Some("No changes to commit".to_string());
            return;
        }

        // Clone config to avoid borrow conflict with daemon's db reference
        let config = self.config.clone();
        let db = self.daemon().read_only_db();

        // Execute all decisions in bulk
        let report = match crate::flows::changes::execute_decisions(db, &all_decisions, false) {
            Ok(r) => r,
            Err(e) => {
                self.deploy_conflict_review = None;
                self.deploy_conflict_accumulated.clear();
                self.start_insights_view();
                self.status_message = Some(format!("Execution error: {}", e));
                return;
            }
        };

        // Cleanup resolved conflicts
        let resolved_count = crate::corpus::health::cleanup_resolved_deployment_conflicts(&config, db)
            .unwrap_or(0);

        // Build status message
        let groups_count = self.deploy_conflict_accumulated.len();
        let base_msg = if report.failed > 0 {
            format!(
                "Committed {} groups: {} succeeded, {} failed",
                groups_count, report.succeeded, report.failed
            )
        } else {
            format!(
                "Committed {} groups: {} changes applied",
                groups_count, report.succeeded
            )
        };

        let resolved_msg = if resolved_count > 0 {
            format!(". {} conflict(s) resolved", resolved_count)
        } else {
            String::new()
        };

        let status_msg = format!("{}{}", base_msg, resolved_msg);

        // Clear state
        self.deploy_conflict_review = None;
        self.deploy_conflict_accumulated.clear();
        self.start_insights_view();
        self.status_message = Some(status_msg);
    }

    fn discard_deploy_conflict_changes(&mut self) {
        let groups_count = self.deploy_conflict_accumulated.len();
        let status_msg = format!(
            "Discarded changes from {} conflict group(s)",
            groups_count
        );
        self.deploy_conflict_review = None;
        self.deploy_conflict_accumulated.clear();
        self.start_insights_view();
        self.status_message = Some(status_msg);
    }
}

// ============================================================================
// Rendering
// ============================================================================

fn render(f: &mut Frame, app: &mut App) {
    let mut ctx = render::RenderContext {
        mode: app.mode,
        config: &app.config,
        status_message: app.status_message.as_deref(),
        tag_editor: app.tag_editor.as_mut(),
        tag_editor_modal: app.tag_editor_modal.as_ref(),
        tree_browser: app.tree_browser.as_mut(),
        drop_missing_state: app.drop_missing_state.as_ref(),
        deployment_preview: app.deployment_preview.as_mut(),
        exit_confirm_modal_state: app.exit_confirm_modal_state.as_ref(),
        splash_screen: app.splash_screen.as_ref(),
        deploy_conflict_review: app.deploy_conflict_review.as_ref(),
        insights_view: app.insights_view.as_mut(),
        tag_search: app.tag_search.as_ref(),
        intake_confirmation: app.intake_confirmation.as_ref(),
        unified_tag_editor: app.unified_tag_editor.as_mut(),
        eye: &app.eye,
        throughput_samples: &app.throughput_samples,
        background_tasks: &app.background_tasks,
        daemon_status: app.task_daemon.as_mut().and_then(|d| {
            // Tick to advance daemon and get current status (including lingering completed sessions)
            let status = d.tick();
            // Show if there's pending work OR a lingering completed session
            if status.pending > 0 || status.completed_session.is_some() {
                Some(status)
            } else {
                None
            }
        }),
    };
    render::render(f, &mut ctx);
}



// ============================================================================
// Startup Health Version Check
// ============================================================================

/// Check health data version and trigger rebuild if needed.
/// This runs at startup before the main event loop.
fn check_and_maybe_rebuild_health(app: &mut App) {
    // Check if database exists before trying to access
    let db_path = match config::get_db_path() {
        Ok(p) if p.exists() => p,
        _ => return, // No DB yet, nothing to rebuild
    };

    // Use daemon's read-only connection
    let _ = db_path; // Suppress unused warning - daemon handles path internally
    let db = app.daemon().read_only_db();

    // Get stored version
    let stored_version = db.get_health_version().ok().flatten();

    match stored_version {
        None => {
            // No version stored - potential corruption or first run after adding versioning.
            // Rebuild to ensure consistency.
            app.status_message = Some(
                "Health index version missing (potential corruption detected), rebuilding...".to_string()
            );
            app.rebuild_health_index();
        }
        Some(v) if v != HEALTH_DATA_VERSION => {
            // Version mismatch - need to rebuild for upgrade
            app.status_message = Some(format!(
                "Health data version changed ({} → {}), rebuilding...",
                v, HEALTH_DATA_VERSION
            ));
            app.rebuild_health_index();
        }
        Some(_) => {
            // Version matches, no rebuild needed
        }
    }
}

// ============================================================================
// Database Migration Check
// ============================================================================

/// Check for pending database migrations and run them with a blocking UI.
///
/// This runs before the main app loop starts to ensure the database schema
/// is up to date. Shows a blocking dialog during migration execution.
///
/// For first-time setup (no database exists), shows a "Create new database?"
/// dialog and initializes with the latest schema version.
///
/// For existing databases, checks for pending migrations and prompts user
/// to approve them (DecisionWitness pattern).
fn check_and_run_migrations<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
) -> Result<()> {
    use crossterm::event::{self, Event, KeyCode};
    use ratatui::layout::{Alignment, Rect};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};
    use crate::daemon::confirm_decision;

    // Get database path
    let db_path = match config::get_db_path() {
        Ok(p) => p,
        Err(_) => return Ok(()), // No database path configured, skip
    };

    // Check if this is first-time setup (no database file exists)
    let is_first_time = !db_path.exists();

    if is_first_time {
        // First-time setup: prompt to create new database
        return startup::handle_first_time_setup(terminal, &db_path);
    }

    // Existing database: open and check for migrations
    // NOTE: This uses Database::open() (not read_only) because migrations
    // need write access. This runs before daemon exists and is explicitly
    // witnessed via confirm_decision() below.
    let db = match Database::open(&db_path) {
        Ok(d) => d,
        Err(_) => return Ok(()), // Can't open database, skip migrations
    };

    // Check if migrations are needed
    let registry = MigrationRegistry::new();
    if !registry.needs_migration(&db) {
        return Ok(());
    }

    // Get pending migration descriptions
    let pending = registry.pending_descriptions(&db);
    let migration_count = pending.len();

    // Render approval dialog and wait for user input
    loop {
        terminal.draw(|f| {
            let area = f.area();

            // Center the dialog
            let dialog_width = 60.min(area.width.saturating_sub(4));
            let dialog_height = (migration_count as u16 + 12).min(area.height.saturating_sub(4));

            let dialog_area = Rect {
                x: (area.width.saturating_sub(dialog_width)) / 2,
                y: (area.height.saturating_sub(dialog_height)) / 2,
                width: dialog_width,
                height: dialog_height,
            };

            // Clear the area behind the dialog
            f.render_widget(Clear, dialog_area);

            // Build migration list text
            let mut lines = vec![
                ratatui::text::Line::from(""),
                ratatui::text::Line::from("MLA needs to upgrade your database.").style(
                    Style::default().fg(Color::White),
                ),
                ratatui::text::Line::from(""),
                ratatui::text::Line::from("Pending migrations:").style(
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
            ];

            for desc in &pending {
                lines.push(ratatui::text::Line::from(format!("  • {}", desc)));
            }

            lines.push(ratatui::text::Line::from(""));
            lines.push(
                ratatui::text::Line::from("⚠ This cannot be interrupted once started.")
                    .style(Style::default().fg(Color::Yellow)),
            );
            lines.push(ratatui::text::Line::from(""));
            lines.push(
                ratatui::text::Line::from("[Enter] Proceed    [Esc] Exit")
                    .style(Style::default().fg(Color::Cyan)),
            );

            let paragraph = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" Database Migration Required ")
                        .title_alignment(Alignment::Center)
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Yellow)),
                )
                .alignment(Alignment::Center);

            f.render_widget(paragraph, dialog_area);
        })?;

        // Wait for user input
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => {
                    // User approved - create witness and proceed
                    let _witness = confirm_decision();
                    break;
                }
                KeyCode::Esc => {
                    // User declined - exit application
                    return Err(anyhow::anyhow!("Migration cancelled by user"));
                }
                _ => {
                    // Ignore other keys
                }
            }
        }
    }

    // Show "running migrations" status
    terminal.draw(|f| {
        let area = f.area();

        let dialog_width = 60.min(area.width.saturating_sub(4));
        let dialog_height = (migration_count as u16 + 8).min(area.height.saturating_sub(4));

        let dialog_area = Rect {
            x: (area.width.saturating_sub(dialog_width)) / 2,
            y: (area.height.saturating_sub(dialog_height)) / 2,
            width: dialog_width,
            height: dialog_height,
        };

        f.render_widget(Clear, dialog_area);

        let mut lines = vec![
            ratatui::text::Line::from(""),
            ratatui::text::Line::from("Running migrations:").style(
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            ratatui::text::Line::from(""),
        ];

        for desc in &pending {
            lines.push(ratatui::text::Line::from(format!("  • {}", desc)));
        }

        lines.push(ratatui::text::Line::from(""));
        lines.push(
            ratatui::text::Line::from("Please wait...")
                .style(Style::default().fg(Color::Cyan)),
        );

        let paragraph = Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Database Migration ")
                    .title_alignment(Alignment::Center)
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            )
            .alignment(Alignment::Center);

        f.render_widget(paragraph, dialog_area);
    })?;

    // Execute migrations (with witness created above)
    let result = registry.apply_all_pending(&db);

    match result {
        Ok(count) => {
            // Show completion message briefly
            terminal.draw(|f| {
                let area = f.area();
                let dialog_width = 50.min(area.width.saturating_sub(4));
                let dialog_height = 7;

                let dialog_area = Rect {
                    x: (area.width.saturating_sub(dialog_width)) / 2,
                    y: (area.height.saturating_sub(dialog_height)) / 2,
                    width: dialog_width,
                    height: dialog_height,
                };

                f.render_widget(Clear, dialog_area);

                let paragraph = Paragraph::new(vec![
                    ratatui::text::Line::from(""),
                    ratatui::text::Line::from(format!("✓ {} migration(s) completed successfully", count))
                        .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    ratatui::text::Line::from(""),
                    ratatui::text::Line::from("Starting application...")
                        .style(Style::default().fg(Color::DarkGray)),
                ])
                .block(
                    Block::default()
                        .title(" Migration Complete ")
                        .title_alignment(Alignment::Center)
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Green)),
                )
                .alignment(Alignment::Center);

                f.render_widget(paragraph, dialog_area);
            })?;

            // Brief pause to show completion
            std::thread::sleep(std::time::Duration::from_millis(800));
            Ok(())
        }
        Err(e) => {
            // Show error and wait for keypress
            terminal.draw(|f| {
                let area = f.area();
                let dialog_width = 60.min(area.width.saturating_sub(4));
                let dialog_height = 10;

                let dialog_area = Rect {
                    x: (area.width.saturating_sub(dialog_width)) / 2,
                    y: (area.height.saturating_sub(dialog_height)) / 2,
                    width: dialog_width,
                    height: dialog_height,
                };

                f.render_widget(Clear, dialog_area);

                let paragraph = Paragraph::new(vec![
                    ratatui::text::Line::from(""),
                    ratatui::text::Line::from("✗ Migration failed")
                        .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    ratatui::text::Line::from(""),
                    ratatui::text::Line::from(format!("{}", e))
                        .style(Style::default().fg(Color::Red)),
                    ratatui::text::Line::from(""),
                    ratatui::text::Line::from("Press any key to continue...")
                        .style(Style::default().fg(Color::DarkGray)),
                ])
                .block(
                    Block::default()
                        .title(" Migration Error ")
                        .title_alignment(Alignment::Center)
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Red)),
                )
                .alignment(Alignment::Center);

                f.render_widget(paragraph, dialog_area);
            })?;

            // Wait for any keypress
            loop {
                if let Ok(Event::Key(_)) = event::read() {
                    break;
                }
            }

            // Continue anyway - app will handle errors as they arise
            Ok(())
        }
    }
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
    check_and_run_migrations(&mut terminal)?;

    let mut app = App::new(config);

    // Check health data version and trigger rebuild if needed
    check_and_maybe_rebuild_health(&mut app);

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
    loop {
        // Update eye animation - only animate when daemon eye is Awake
        let can_animate = app.daemon().eye_state() == crate::daemon::EyeState::Awake;
        app.eye.update(can_animate);
        app.update_operation_progress();

        // Always tick daemon (advances work state, handles eyeballing completion)
        app.daemon().tick();

        // Tick splash screen if active (startup eyeballing)
        if app.splash_screen.is_some() {
            app.tick_splash_screen();
        }

        terminal.draw(|f| render(f, app))?;

        // Tick tag search for pending bulk edit (after modal has rendered)
        app.tick_tag_search();

        if event::poll(std::time::Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    // Global quit shortcut
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                    {
                        break;
                    }
                    app.handle_key(key);
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
