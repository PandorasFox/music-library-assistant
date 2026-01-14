//! TUI Module
//!
//! Modular UI components for the Music Library Assistant.
//!
//! # Module Structure
//!
//! Flow handlers should NOT be added directly to `impl App` in this file.
//! Instead, create coordinator modules in flow submodules:
//! - `ui/canon_flow/coordinator.rs` - Artist/genre canonicalization
//! - `ui/album_flow/coordinator.rs` - Album tag resolution
//! - `ui/album_artist_flow/coordinator.rs` - Album artist resolution
//! - `ui/dedup_flow/coordinator.rs` - Deduplication workflow
//!
//! App methods should be thin delegation wrappers to these modules.
//! This keeps `mod.rs` focused on core App lifecycle and event dispatch.

pub mod album_artist_flow;
pub mod album_flow;
pub mod app;
pub mod canon_flow;
pub mod corpus_browser;
pub mod dedup_flow;
pub mod deploy_flow;
pub mod dialogue;
pub mod dir_browser;
pub mod drop_flow;
pub mod flows;
pub mod helpers;
pub mod insights_view;
pub mod main_menu;
pub mod render;
pub mod shared;
pub mod tag_editor;
pub mod widgets;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    Frame, Terminal,
};
use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::Instant;

use crate::config::{self, Config};
use crate::corpus::{spawn_heartbeat, HeartbeatResult};
use crate::corpus::db::{Database, HealthIssueType};
use crate::ops::operation::{
    log_operation_summary, OperationManager, OperationMessage, OperationProgress,
    OperationResult, OperationType, ProgressReporter,
};
use crate::ops::{reports, scanner, ScanMessage};

use app::{EyeAnimation, HeartbeatRollResult};
// TODO: main_menu module is deprecated. Insights is now the main view.
// Review main_menu.rs for code that may still be needed (e.g., CommandAction, TransitionTarget).
// See main_menu.rs for details on what functionality lived there.
use main_menu::{BackgroundTask, CommandAction, MainMenuState, MenuAction, ReportType, TransitionTarget};

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
    MainMenu,
    TagEditor,
    DirBrowser,
    Dialogue,
    DialogueSummary,
    ClusterDialogue,
    BulkReviewPrompt,
    SessionReview,
    DropMissingConfirmation,
    DeploymentPreview,
    /// Artist canonicalization - Single-screen three-pane cluster view
    CanonClusterView,
    /// Artist canonicalization - Session review before commit
    CanonSessionReview,
    /// Artist canonicalization - Commit completion modal
    CanonCommitModal,
    /// Exit confirmation modal (when operations are in progress)
    ExitConfirmModal,
    /// Corpus browser with directory tree and metadata preview
    CorpusBrowser,
    /// Album artist resolution - phase selector popup
    AlbumArtistPhaseSelector,
    /// Album artist resolution - canonicalization cluster view
    AlbumArtistClusterView,
    /// Album artist resolution - collation flow
    AlbumArtistCollation,
    /// Album artist resolution - collation review
    AlbumArtistCollationReview,
    /// Album artist resolution - population flow
    AlbumArtistPopulation,
    /// Album artist resolution - population review
    AlbumArtistPopulationReview,
    /// Album artist resolution - session review
    AlbumArtistReview,
    /// Album tag resolution - cluster view
    AlbumClusterView,
    /// Album tag resolution - session review
    AlbumReview,
    /// Directory-based bulk tag editor (aggregated view)
    DirectoryTagEditor,
    /// Deploy conflict resolution - review accumulated changes before commit
    DeployConflictReview,
    /// Full-screen insights view (part of lateral view ring)
    Insights,
    /// Loading splash screen - centered eye with status message
    LoadingSplash,
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

/// Type of loading operation for the splash screen
#[derive(Debug, Clone)]
pub(crate) enum LoadingType {
    /// Initial corpus heartbeat check
    Heartbeat,
    /// Corpus scan in progress
    CorpusScan,
    /// Legacy library scan
    LegacyScan,
    /// General operation with custom message
    Operation(String),
}

impl LoadingType {
    /// Get the display message for this loading type
    pub fn message(&self) -> &str {
        match self {
            LoadingType::Heartbeat => "Checking corpus health...",
            LoadingType::CorpusScan => "Scanning corpus...",
            LoadingType::LegacyScan => "Scanning legacy library...",
            LoadingType::Operation(msg) => msg,
        }
    }
}

/// State for the loading splash screen
#[derive(Debug, Clone)]
pub(crate) struct LoadingSplashState {
    /// Type of loading operation
    pub loading_type: LoadingType,
    /// Optional progress (0.0 to 1.0)
    pub progress: Option<f32>,
    /// Optional progress detail (e.g., "1234 / 5678 files")
    pub progress_detail: Option<String>,
    /// Mode to transition to when loading completes
    pub target_mode: UiMode,
}

impl LoadingSplashState {
    /// Create a new loading splash for heartbeat
    pub fn heartbeat() -> Self {
        Self {
            loading_type: LoadingType::Heartbeat,
            progress: None,
            progress_detail: None,
            target_mode: UiMode::Insights,
        }
    }

    /// Create a loading splash for a scan operation
    pub fn scan(is_corpus: bool) -> Self {
        Self {
            loading_type: if is_corpus {
                LoadingType::CorpusScan
            } else {
                LoadingType::LegacyScan
            },
            progress: None,
            progress_detail: None,
            target_mode: UiMode::Insights,
        }
    }

    /// Update progress
    pub fn set_progress(&mut self, completed: usize, total: usize) {
        if total > 0 {
            self.progress = Some(completed as f32 / total as f32);
            self.progress_detail = Some(format!("{} / {} files", completed, total));
        }
    }
}

/// State for the canon commit modal with two options
#[derive(Debug, Clone)]
pub(crate) struct CanonCommitModalState {
    /// Currently selected option (0 = main menu, 1 = deployment review)
    pub(crate) selected_option: usize,
    /// Number of tracks updated in the database
    pub(crate) tracks_updated: usize,
    /// Number of deployments that are now stale
    pub(crate) stale_deployments: usize,
}

impl Default for CanonCommitModalState {
    fn default() -> Self {
        Self {
            selected_option: 1, // Default to deployment review
            tracks_updated: 0,
            stale_deployments: 0,
        }
    }
}

/// Accumulated changes for a single deploy conflict group
#[derive(Debug, Clone)]
pub(crate) struct DeployConflictGroupChanges {
    /// Health issue ID for this conflict group
    pub group_id: i64,
    /// Target deployment path this conflict was about
    pub target_path: String,
    /// Track count in this group
    pub track_count: usize,
    /// Pending changes for this group
    pub changes: Vec<crate::corpus::db::PendingChange>,
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

/// Context for what operation launched the directory browser.
/// The browser returns paths; this tells us what to do with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserContext {
    /// Fingerprint-based deduplication
    Sleuthing,
}

/// Main application state
pub(crate) struct App {
    config: Config,
    should_quit: bool,
    status_message: Option<String>,

    // UI mode and state
    mode: UiMode,
    main_menu: MainMenuState,
    tag_editor: Option<tag_editor::TagEditorState>,
    tag_editor_modal: Option<tag_editor::TagEditorModal>,
    dir_browser: Option<dir_browser::DirBrowserState>,
    browser_context: Option<BrowserContext>,
    corpus_browser: Option<corpus_browser::CorpusBrowserState>,
    dialogue: Option<dialogue::DialogueState>,
    dialogue_summary: Option<dialogue::DialogueSummaryState>,
    cluster_dialogue: Option<dedup_flow::ClusterDialogueState>,
    bulk_prompt: Option<dedup_flow::BulkPromptState>,
    session_review: Option<dedup_flow::SessionReviewState>,
    drop_missing_state: Option<DropMissingState>,
    deployment_preview: Option<deploy_flow::DeploymentPreviewState>,
    // Artist canonicalization flow
    canon_cluster_view: Option<canon_flow::ClusterViewState>,
    canon_session_review: Option<canon_flow::ReviewState>,
    canon_commit_modal_state: Option<CanonCommitModalState>,
    // Album artist resolution flow
    album_artist_phase_selector: Option<album_artist_flow::PhaseSelectorState>,
    album_artist_cluster_view: Option<album_artist_flow::AlbumArtistClusterState>,
    album_artist_collation: Option<album_artist_flow::CollationState>,
    album_artist_collation_review: Option<album_artist_flow::CollationReviewState>,
    album_artist_population: Option<album_artist_flow::PopulationState>,
    album_artist_population_review: Option<album_artist_flow::PopulationReviewState>,
    album_artist_review: Option<album_artist_flow::AlbumArtistReviewState>,
    album_artist_selected_phases: Vec<album_artist_flow::AlbumArtistPhase>,
    // Album canonicalization flow
    album_cluster_view: Option<album_flow::AlbumClusterState>,
    album_review: Option<album_flow::AlbumReviewState>,
    // Directory tag editor (bulk editing)
    directory_tag_editor: Option<tag_editor::DirectoryTagEditorState>,
    directory_tag_editor_modal: Option<tag_editor::types::DirectoryTagEditorModal>,
    // Exit confirmation modal
    exit_confirm_modal_state: Option<ExitConfirmModalState>,
    // Loading splash screen
    loading_splash_state: Option<LoadingSplashState>,
    // Deploy conflict resolution flow
    deploy_conflict_review: Option<DeployConflictReviewState>,
    deploy_conflict_accumulated: Vec<DeployConflictGroupChanges>,
    // Insights view (lateral view ring)
    insights_view: Option<insights_view::InsightsViewState>,

    // Background operations (supports multiple concurrent)
    operations: OperationManager,
    // Legacy single-operation receiver for scan (uses ScanMessage)
    legacy_scan_receiver: Option<mpsc::Receiver<ScanMessage>>,
    legacy_scan_type: Option<OperationType>,

    // Startup heartbeat
    heartbeat_result: Option<HeartbeatResult>,
    heartbeat_receiver: Option<mpsc::Receiver<HeartbeatResult>>,
    /// Last heartbeat roll result (Normal vs Expensive)
    last_heartbeat_roll: Option<HeartbeatRollResult>,

    // Tag cloud for canonicalization detection (cached, invalidated after mutations)
    tag_cloud: Option<crate::corpus::health::TagCloud>,
    tag_cloud_receiver: Option<mpsc::Receiver<anyhow::Result<crate::corpus::health::TagCloud>>>,

    // Throughput tracking for rolling average (timestamp, bytes_processed)
    throughput_samples: VecDeque<(Instant, u64)>,

    // Eye animation
    eye: EyeAnimation,

    // Tag editing session
    #[allow(dead_code)]
    tag_edit_session_id: String,

    // First-time startup flag (indicates auto-scan was triggered)
    first_time_scan_active: bool,
}

impl App {
    fn new(config: Config) -> Self {
        Self {
            main_menu: MainMenuState::new(&config),
            config,
            should_quit: false,
            status_message: None,
            mode: UiMode::Insights,
            tag_editor: None,
            tag_editor_modal: None,
            dir_browser: None,
            browser_context: None,
            corpus_browser: None,
            dialogue: None,
            dialogue_summary: None,
            cluster_dialogue: None,
            bulk_prompt: None,
            session_review: None,
            drop_missing_state: None,
            deployment_preview: None,
            canon_cluster_view: None,
            canon_session_review: None,
            canon_commit_modal_state: None,
            album_artist_phase_selector: None,
            album_artist_cluster_view: None,
            album_artist_collation: None,
            album_artist_collation_review: None,
            album_artist_population: None,
            album_artist_population_review: None,
            album_artist_review: None,
            album_artist_selected_phases: Vec::new(),
            album_cluster_view: None,
            album_review: None,
            directory_tag_editor: None,
            directory_tag_editor_modal: None,
            exit_confirm_modal_state: None,
            loading_splash_state: None,
            deploy_conflict_review: None,
            deploy_conflict_accumulated: Vec::new(),
            insights_view: None,
            operations: OperationManager::new(),
            legacy_scan_receiver: None,
            legacy_scan_type: None,
            heartbeat_result: None,
            heartbeat_receiver: None,
            last_heartbeat_roll: None,
            tag_cloud: None,
            tag_cloud_receiver: None,
            throughput_samples: VecDeque::with_capacity(100),
            eye: EyeAnimation::default(),
            tag_edit_session_id: uuid::Uuid::new_v4().to_string(),
            first_time_scan_active: false,
        }
    }

    /// Check if this is a first-time startup (no corpus tracks indexed).
    /// If so, automatically start a corpus scan.
    fn check_first_time_startup(&mut self) {
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return,
        };

        let db = match Database::open(&db_path) {
            Ok(d) => d,
            Err(_) => return,
        };

        // Check if there are any corpus tracks
        let corpus_count = db.get_track_count(Some("corpus")).unwrap_or(0);

        if corpus_count == 0 {
            // First time startup - auto-start corpus scan
            // TODO: Implement first-time config-setting flow for new users.
            // Should prompt for: corpus root, library paths, optional legacy library.
            // Write generated config.kdl to XDG config location.
            self.status_message = Some("First-time setup: Scanning corpus...".to_string());
            self.first_time_scan_active = true;
            self.start_scan_corpus();
        }
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        match self.mode {
            UiMode::MainMenu => {
                // TODO: Dead code - MainMenu mode is no longer reachable.
                // Insights is now the main view. See main_menu.rs for details.
                let action = self.main_menu.handle_key(key);
                self.handle_menu_action(action);
            }
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
                if let Some(ref mut browser) = self.dir_browser {
                    let action = browser.handle_key(key);
                    self.handle_dir_browser_action(action);
                }
            }
            UiMode::Dialogue => {
                if let Some(ref mut dialogue) = self.dialogue {
                    let result = dialogue.handle_key(key);
                    self.handle_dialogue_result(result);
                }
            }
            UiMode::DialogueSummary => {
                if let Some(ref mut summary) = self.dialogue_summary {
                    let result = summary.handle_key(key);
                    self.handle_dialogue_result(result);
                }
            }
            UiMode::ClusterDialogue => {
                if let Some(ref mut cluster_dlg) = self.cluster_dialogue {
                    let action = cluster_dlg.handle_key(key);
                    self.handle_cluster_dialogue_action(action);
                }
            }
            UiMode::BulkReviewPrompt => {
                if let Some(ref mut bulk_prompt) = self.bulk_prompt {
                    let action = bulk_prompt.handle_key(key);
                    self.handle_bulk_prompt_action(action);
                }
            }
            UiMode::SessionReview => {
                if let Some(ref mut session_review) = self.session_review {
                    let action = session_review.handle_key(key);
                    self.handle_session_review_action(action);
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
            UiMode::CanonClusterView => {
                if let Some(ref mut cluster_view) = self.canon_cluster_view {
                    let action = cluster_view.handle_key(key);
                    self.handle_canon_cluster_action(action);
                }
            }
            UiMode::CanonSessionReview => {
                if let Some(ref mut review) = self.canon_session_review {
                    let action = review.handle_key(key);
                    self.handle_canon_review_action(action);
                }
            }
            UiMode::CanonCommitModal => {
                if let Some(ref mut state) = self.canon_commit_modal_state {
                    match key.code {
                        KeyCode::Up => {
                            state.selected_option = state.selected_option.saturating_sub(1);
                        }
                        KeyCode::Down => {
                            state.selected_option = (state.selected_option + 1).min(1);
                        }
                        KeyCode::Enter => {
                            match state.selected_option {
                                0 => {
                                    // Return to main menu
                                    self.canon_commit_modal_state = None;
                                    self.mode = UiMode::Insights;
                                }
                                1 => {
                                    // Proceed to deployment review
                                    self.canon_commit_modal_state = None;
                                    self.start_deployment_preview();
                                }
                                _ => {}
                            }
                        }
                        KeyCode::Esc => {
                            // Esc also returns to main menu
                            self.canon_commit_modal_state = None;
                            self.mode = UiMode::Insights;
                        }
                        _ => {}
                    }
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
                                self.mode = UiMode::Insights;
                            } else {
                                // "Yes" selected - actually quit
                                self.should_quit = true;
                            }
                        }
                        KeyCode::Esc => {
                            // Esc returns to Insights view
                            self.exit_confirm_modal_state = None;
                            self.mode = UiMode::Insights;
                        }
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            // 'y' confirms exit
                            self.should_quit = true;
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') => {
                            // 'n' cancels - return to Insights view
                            self.exit_confirm_modal_state = None;
                            self.mode = UiMode::Insights;
                        }
                        _ => {}
                    }
                }
            }
            UiMode::CorpusBrowser => {
                if let Some(ref mut browser) = self.corpus_browser {
                    let action = browser.handle_key(key);
                    self.handle_corpus_browser_action(action);
                }
            }
            UiMode::AlbumArtistPhaseSelector => {
                if let Some(ref mut selector) = self.album_artist_phase_selector {
                    let action = selector.handle_key(key);
                    self.handle_album_artist_phase_action(action);
                }
            }
            UiMode::AlbumArtistClusterView => {
                if let Some(ref mut cluster_view) = self.album_artist_cluster_view {
                    let action = cluster_view.handle_key(key);
                    self.handle_album_artist_cluster_action(action);
                }
            }
            UiMode::AlbumArtistReview => {
                if let Some(ref mut review) = self.album_artist_review {
                    let action = review.handle_key(key);
                    self.handle_album_artist_review_action(action);
                }
            }
            UiMode::AlbumArtistCollation => {
                if let Some(ref mut collation) = self.album_artist_collation {
                    let action = collation.handle_key(key);
                    self.handle_album_artist_collation_action(action);
                }
            }
            UiMode::AlbumArtistCollationReview => {
                if let Some(ref mut review) = self.album_artist_collation_review {
                    let action = review.handle_key(key);
                    self.handle_album_artist_collation_review_action(action);
                }
            }
            UiMode::AlbumArtistPopulation => {
                if let Some(ref mut population) = self.album_artist_population {
                    let action = population.handle_key(key);
                    self.handle_album_artist_population_action(action);
                }
            }
            UiMode::AlbumArtistPopulationReview => {
                if let Some(ref mut review) = self.album_artist_population_review {
                    let action = review.handle_key(key);
                    self.handle_album_artist_population_review_action(action);
                }
            }
            UiMode::AlbumClusterView => {
                if let Some(ref mut cluster_view) = self.album_cluster_view {
                    let action = cluster_view.handle_key(key);
                    self.handle_album_cluster_action(action);
                }
            }
            UiMode::AlbumReview => {
                if let Some(ref mut review) = self.album_review {
                    let action = review.handle_key(key);
                    self.handle_album_review_action(action);
                }
            }
            UiMode::DirectoryTagEditor => {
                // Handle modal keys first if modal is active
                if self.directory_tag_editor_modal.is_some() {
                    self.handle_directory_tag_editor_modal_key(key);
                } else if let Some(ref mut editor) = self.directory_tag_editor {
                    let action = editor.handle_key(key);
                    self.handle_directory_tag_editor_action(action);
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

    /// Check if there are any pending operations (heartbeat, scans, etc.)
    fn has_pending_operations(&self) -> bool {
        self.heartbeat_receiver.is_some()
            || self.legacy_scan_receiver.is_some()
            || !self.operations.is_empty()
    }

    /// Start the insights view with current heartbeat data.
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

        let db = match Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                self.status_message = Some(format!("Database error: {}", e));
                return;
            }
        };

        // Get current heartbeat result (must have completed at least one)
        let heartbeat = match &self.heartbeat_result {
            Some(h) => h.clone(),
            None => {
                self.status_message = Some("Heartbeat not yet completed - please wait".to_string());
                return;
            }
        };

        // Initialize insights view
        let mut view = insights_view::InsightsViewState::new();
        view.initialize(&db, db_path.to_str().unwrap_or(""), heartbeat);

        self.insights_view = Some(view);
        self.mode = UiMode::Insights;
    }

    fn handle_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::None => {}
            MenuAction::Quit => {
                self.should_quit = true;
            }
            MenuAction::Execute(cmd_action) => {
                self.execute_command(cmd_action);
            }
        }
    }

    fn execute_command(&mut self, action: CommandAction) {
        match action {
            CommandAction::Background(task) => {
                self.start_background_task(task);
            }
            CommandAction::Transition(target) => {
                self.transition_to(target);
            }
            CommandAction::Message(msg) => {
                self.status_message = Some(msg);
            }
            CommandAction::Quit => {
                // If operations are running, show warning confirmation modal
                if !self.operations.is_empty() {
                    self.exit_confirm_modal_state = Some(ExitConfirmModalState::new(true));
                    self.mode = UiMode::ExitConfirmModal;
                } else {
                    self.should_quit = true;
                }
            }
        }
    }

    fn start_background_task(&mut self, task: BackgroundTask) {
        match task {
            BackgroundTask::ScanCorpus => {
                self.start_scan_corpus();
            }
            BackgroundTask::ScanLegacy => {
                self.start_scan_legacy();
            }
            BackgroundTask::GenerateReport { report_type } => {
                self.generate_report(report_type);
            }
            BackgroundTask::Deploy => {
                self.start_deployment_preview();
            }
            BackgroundTask::Stub => {
                self.status_message = Some("Feature not yet implemented".to_string());
            }
        }
    }

    fn start_scan_corpus(&mut self) {
        let path = self.config.corpus_root.to_string_lossy().to_string();
        self.start_scan_source("corpus", &path);
    }

    fn start_scan_legacy(&mut self) {
        if let Some(ref legacy_path) = self.config.legacy_library {
            let path = legacy_path.to_string_lossy().to_string();
            self.start_scan_source("legacy", &path);
        } else {
            self.status_message = Some("No legacy library configured".to_string());
        }
    }

    fn start_deployment_preview(&mut self) {
        // Compute deployment status synchronously (could be made async for large libraries)
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                self.status_message = Some(format!("Config error: {}", e));
                return;
            }
        };

        let db = match Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                self.status_message = Some(format!("Database error: {}", e));
                return;
            }
        };

        let _ = config::log_message("Computing deployment status...");
        match crate::ops::deploy::compute_full_deployment_status(&self.config, &db) {
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

    fn start_scan_source(&mut self, name: &str, path: &str) {
        let (tx, rx) = mpsc::channel();
        let source_path = path.to_string();
        let source_name = name.to_string();
        let cancel_flag = Arc::new(AtomicBool::new(false));

        // Store scan type for progress display
        let op_type = OperationType::Scanning {
            source_name: source_name.clone(),
        };
        self.legacy_scan_type = Some(op_type.clone());
        self.legacy_scan_receiver = Some(rx);
        // Register with operation manager for unified display
        self.operations.set_legacy_operation(op_type);

        let cancel = cancel_flag.clone();
        std::thread::spawn(move || {
            let result = scanner::scan_directory_with_progress(
                Path::new(&source_path),
                &source_name,
                Some(tx.clone()),
                cancel,
            );
            if let Err(e) = result {
                let _ = tx.send(ScanMessage::Error(e.to_string()));
            }
        });

        self.status_message = Some(format!("Scanning: {}", name));
    }

    fn generate_report(&mut self, report_type: ReportType) {
        // Get report type name for display
        let report_name = match report_type {
            ReportType::GenerateAll => "all",
            ReportType::Legacy => "legacy",
            ReportType::Deployment => "deployment",
            ReportType::Health => "health",
            ReportType::KnownVariants => "known_variants",
        };

        // Add operation to manager
        let (id, tx, cancel_flag) = self.operations.add(OperationType::GeneratingReport {
            report_type: report_name.to_string(),
        });

        self.status_message = Some(format!("Generating {} report... [{}]", report_name, id));

        // Spawn background thread
        std::thread::spawn(move || {
            use std::time::Instant;

            let start = Instant::now();
            let mut reporter = ProgressReporter::new(tx.clone(), cancel_flag.clone());

            // Get reports directory
            let reports_dir = match config::get_data_dir() {
                Ok(dir) => dir.join("reports"),
                Err(e) => {
                    reporter.error(format!("Failed to get data dir: {}", e));
                    return;
                }
            };

            // Ensure reports directory exists
            if let Err(e) = std::fs::create_dir_all(&reports_dir) {
                reporter.error(format!("Failed to create reports dir: {}", e));
                return;
            }

            // Determine which reports to generate
            let report_tasks: Vec<(&str, std::path::PathBuf)> = match report_type {
                ReportType::GenerateAll => vec![
                    ("health", reports_dir.join("health.txt")),
                    ("deployment", reports_dir.join("deployment.txt")),
                    ("known_variants", reports_dir.join("known_variants.txt")),
                ],
                ReportType::Legacy => vec![
                    ("legacy", reports_dir.join("legacy.txt")),
                ],
                ReportType::Deployment => vec![
                    ("deployment", reports_dir.join("deployment.txt")),
                ],
                ReportType::Health => vec![
                    ("health", reports_dir.join("health.txt")),
                ],
                ReportType::KnownVariants => vec![
                    ("known_variants", reports_dir.join("known_variants.txt")),
                ],
            };

            let total = report_tasks.len();
            let mut progress = OperationProgress::new(total);
            reporter.force_update(progress.clone());

            let mut succeeded = 0;
            let mut errors = Vec::new();

            for (name, path) in report_tasks {
                if reporter.is_cancelled() {
                    reporter.cancelled();
                    return;
                }

                progress.current_item = Some(format!("Generating {} report", name));
                reporter.update(progress.clone());

                let result = match name {
                    "deployment" => reports::generate_deployment_report(&path),
                    "legacy" => reports::generate_legacy_report(&path),
                    "health" => reports::generate_health_report(&path),
                    "known_variants" => reports::generate_known_variants_report(&path),
                    _ => Err(anyhow::anyhow!("Unknown report type")),
                };

                match result {
                    Ok(_) => succeeded += 1,
                    Err(e) => errors.push(format!("{}: {}", name, e)),
                }

                progress.completed_items += 1;
                reporter.update(progress.clone());
            }

            let result = OperationResult {
                succeeded,
                skipped: 0,
                failed: errors.len(),
                duration: start.elapsed(),
                bytes_processed: None,
                errors,
                data: None,
            };
            reporter.complete(result);
        });
    }

    fn rebuild_health_index(&mut self) {
        // Add operation to manager
        let (id, tx, cancel_flag) = self.operations.add(OperationType::RebuildingHealth);

        self.status_message = Some(format!("Rebuilding health index... [{}]", id));

        // Spawn background thread
        std::thread::spawn(move || {
            use crate::corpus::detect_fingerprint_issues;
            use std::time::Instant;

            let start = Instant::now();
            let mut reporter = ProgressReporter::new(tx.clone(), cancel_flag.clone());

            // Open database
            let db_path = match config::get_db_path() {
                Ok(p) => p,
                Err(e) => {
                    reporter.error(format!("Config error: {}", e));
                    return;
                }
            };
            let db = match Database::open(&db_path) {
                Ok(db) => db,
                Err(e) => {
                    reporter.error(format!("Database error: {}", e));
                    return;
                }
            };

            // Get all tracks with fingerprints
            let tracks = match db.get_all_tracks(Some("corpus")) {
                Ok(t) => t,
                Err(e) => {
                    reporter.error(format!("Error getting tracks: {}", e));
                    return;
                }
            };

            let with_fingerprints: Vec<_> = tracks
                .into_iter()
                .filter(|t| t.fingerprint.is_some())
                .collect();

            let mut progress = OperationProgress::new(with_fingerprints.len());
            reporter.force_update(progress.clone());

            // Run health detection on each track
            let mut _issues_found = 0;
            for track in &with_fingerprints {
                if reporter.is_cancelled() {
                    reporter.cancelled();
                    return;
                }

                progress.current_item = track.title.clone().or_else(|| {
                    std::path::Path::new(&track.path)
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                });

                if let Ok(issues) = detect_fingerprint_issues(&db, track) {
                    _issues_found += issues.len();
                }

                progress.completed_items += 1;
                reporter.update(progress.clone());
            }

            // Detect and store artist canonicalization issues
            if let Ok(canon_count) = crate::corpus::detect_and_store_canonicalizations(&db) {
                if canon_count > 0 {
                    let _ = config::log_message(&format!(
                        "Health rebuild: detected {} new canonicalization issues",
                        canon_count
                    ));
                }
            }

            // Update health data version on successful completion
            if let Err(e) = db.set_health_version(crate::ui::HEALTH_DATA_VERSION) {
                let _ = config::log_message(&format!(
                    "WARN: Failed to set health version: {}", e
                ));
            }

            let result = OperationResult {
                succeeded: progress.completed_items,
                skipped: 0,
                failed: 0,
                duration: start.elapsed(),
                bytes_processed: None,
                errors: vec![],
                data: None,
            };
            reporter.complete(result);
        });
    }

    fn start_drop_missing_confirmation(&mut self) {
        match drop_flow::find_missing_tracks() {
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
                    self.mode = UiMode::Insights;
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
                self.status_message = Some("No missing tracks to drop".to_string());
                self.mode = UiMode::Insights;
                return;
            }
        };

        match drop_flow::execute_drop_missing(&missing_tracks) {
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
        self.mode = UiMode::Insights;
    }

    fn transition_to(&mut self, target: TransitionTarget) {
        match target {
            TransitionTarget::TagEditor => {
                self.start_tag_editor();
            }
            TransitionTarget::DecisionFlow => {
                self.start_decision_flow();
            }
            TransitionTarget::DirBrowser { context } => {
                self.start_dir_browser(context);
            }
            TransitionTarget::PendingChangesView => {
                self.status_message = Some("Pending changes view (TODO)".to_string());
            }
            TransitionTarget::DropMissingConfirm => {
                self.start_drop_missing_confirmation();
            }
            TransitionTarget::CanonFlow => {
                self.start_canon_flow();
            }
            TransitionTarget::GenreCanonFlow => {
                self.start_genre_canon_flow();
            }
            TransitionTarget::CorpusBrowser => {
                self.start_corpus_browser();
            }
            TransitionTarget::AlbumArtistFlow => {
                self.start_album_artist_flow();
            }
            TransitionTarget::AlbumFlow => {
                self.start_album_flow();
            }
            TransitionTarget::Insights => {
                self.start_insights_view();
            }
        }
    }

    /// Start tag editor for deploy conflict resolution.
    ///
    /// Loads deployment conflicts from health_issues table and presents them
    /// in the tag editor for resolution (editing tags to create unique deploy paths).
    fn start_tag_editor(&mut self) {
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                self.status_message = Some(format!("Config error: {}", e));
                return;
            }
        };
        let db = match Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                self.status_message = Some(format!("Database error: {}", e));
                return;
            }
        };

        // Load deploy conflict health issues
        let all_groups = match load_deploy_conflict_groups(&db) {
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

    fn start_decision_flow(&mut self) {
        // Use fingerprint-based deduplication with directory set clustering
        use crate::corpus::deduplication::find_fingerprint_duplicates;

        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                self.status_message = Some(format!("Config error: {}", e));
                return;
            }
        };
        let db = match Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                self.status_message = Some(format!("Database error: {}", e));
                return;
            }
        };

        let corpus_root = self.config.corpus_root.clone();
        let stash_root = match &self.config.stash_dir {
            Some(path) => path.clone(),
            None => {
                self.status_message = Some("stash-dir not configured".to_string());
                return;
            }
        };

        // Find all fingerprint duplicates
        let conflict_sets = match find_fingerprint_duplicates(
            &db,
            std::slice::from_ref(&corpus_root),
            &corpus_root,
        ) {
            Ok(sets) => sets,
            Err(e) => {
                self.status_message = Some(format!("Error finding duplicates: {}", e));
                return;
            }
        };

        if conflict_sets.is_empty() {
            self.status_message = Some("No fingerprint duplicates found".to_string());
            return;
        }

        // Create session ID
        let session_id = uuid::Uuid::new_v4().to_string();

        // Create cluster dialogue state
        self.cluster_dialogue = Some(dedup_flow::ClusterDialogueState::new(
            conflict_sets,
            session_id,
            corpus_root.to_string_lossy().to_string(),
            stash_root.to_string_lossy().to_string(),
        ));
        self.mode = UiMode::ClusterDialogue;
    }

    // TODO: Uses main_menu::DirBrowserContext - review if this flow is still accessible.
    // Currently reachable via TransitionTarget::DirBrowser from execute_command.
    // See main_menu.rs for details.
    fn start_dir_browser(&mut self, context: main_menu::DirBrowserContext) {
        let config = match context {
            main_menu::DirBrowserContext::Sleuthing => dir_browser::DirBrowserConfig::for_sleuthing(),
        };

        self.dir_browser = Some(dir_browser::DirBrowserState::new(
            self.config.corpus_root.clone(),
            config,
        ));
        self.browser_context = Some(match context {
            main_menu::DirBrowserContext::Sleuthing => BrowserContext::Sleuthing,
        });
        self.mode = UiMode::DirBrowser;
    }

    fn start_corpus_browser(&mut self) {
        let config = corpus_browser::CorpusBrowserConfig::default();
        self.corpus_browser = Some(corpus_browser::CorpusBrowserState::new(
            self.config.corpus_root.clone(),
            config,
        ));
        self.mode = UiMode::CorpusBrowser;
    }

    fn handle_corpus_browser_action(&mut self, action: corpus_browser::CorpusBrowserAction) {
        match action {
            corpus_browser::CorpusBrowserAction::None => {}
            corpus_browser::CorpusBrowserAction::Cancel => {
                self.corpus_browser = None;
                self.mode = UiMode::Insights;
            }
            corpus_browser::CorpusBrowserAction::EditDirectory(path) => {
                // Start directory tag editor with aggregated view
                self.start_directory_tag_editor(&path);
            }
            corpus_browser::CorpusBrowserAction::EditFile(path) => {
                // Load single track for editing
                self.start_tag_editor_for_path(&path, false);
            }
            corpus_browser::CorpusBrowserAction::CycleNext => {
                // Corpus Browser → Insights
                self.corpus_browser = None;
                self.start_insights_view();
            }
            corpus_browser::CorpusBrowserAction::CyclePrev => {
                // Corpus Browser → Deploy
                self.corpus_browser = None;
                self.start_deployment_preview();
            }
        }
    }

    fn start_tag_editor_for_path(&mut self, path: &std::path::Path, recursive: bool) {
        let db_path = match crate::config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                self.status_message = Some(format!("Config error: {}", e));
                self.corpus_browser = None;
                self.mode = UiMode::Insights;
                return;
            }
        };

        let db = match crate::corpus::db::Database::open(&db_path) {
            Ok(d) => d,
            Err(e) => {
                self.status_message = Some(format!("Database error: {}", e));
                self.corpus_browser = None;
                self.mode = UiMode::Insights;
                return;
            }
        };

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
                    self.corpus_browser = None;
                    self.mode = UiMode::Insights;
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
                    self.corpus_browser = None;
                    self.mode = UiMode::Insights;
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
                    self.corpus_browser = None;
                    self.mode = UiMode::Insights;
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
                        self.corpus_browser = None;
                        self.mode = UiMode::Insights;
                        return;
                    }
                    Err(e) => {
                        self.status_message = Some(format!(
                            "Query error for '{}': {}",
                            path.display(),
                            e
                        ));
                        self.corpus_browser = None;
                        self.mode = UiMode::Insights;
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
            self.corpus_browser = None;
            self.mode = UiMode::Insights;
            return;
        }

        // Create tag editor with tracks (no duplicate groups for corpus browser)
        let mut editor = tag_editor::TagEditorState::new(tracks.clone(), Vec::new());
        // Position on the selected track
        editor.current_track_idx = selected_idx;
        self.tag_editor = Some(editor);
        self.status_message = Some(format!(
            "Loaded {} track(s) from {}",
            tracks.len(),
            path.display()
        ));
        self.corpus_browser = None;
        self.mode = UiMode::TagEditor;
    }

    fn handle_dir_browser_action(&mut self, action: dir_browser::DirBrowserAction) {
        match action {
            dir_browser::DirBrowserAction::None => {}
            dir_browser::DirBrowserAction::Cancel => {
                self.dir_browser = None;
                self.browser_context = None;
                self.mode = UiMode::Insights;
            }
            dir_browser::DirBrowserAction::Proceed(paths) => {
                let context = self.browser_context.take();
                self.dir_browser = None;

                match context {
                    Some(BrowserContext::Sleuthing) => {
                        self.start_sleuthing_with_paths(paths);
                    }
                    None => {
                        self.status_message = Some("No browser context set".to_string());
                        self.mode = UiMode::Insights;
                    }
                }
            }
        }
    }

    fn start_sleuthing_with_paths(&mut self, paths: Vec<std::path::PathBuf>) {
        use crate::corpus::deduplication::find_duplicates_between_directories;

        let _ = config::log_message("=== start_sleuthing_with_paths called ===");
        for (i, path) in paths.iter().enumerate() {
            let _ = config::log_message(&format!("  path[{}]: {}", i, path.display()));
        }

        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                let _ = config::log_message(&format!("ERROR: Failed to get db path: {}", e));
                self.status_message = Some(format!("Config error: {}", e));
                self.mode = UiMode::Insights;
                return;
            }
        };
        let _ = config::log_message(&format!("Database path: {}", db_path.display()));

        let db = match Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                let _ = config::log_message(&format!("ERROR: Failed to open database: {}", e));
                self.status_message = Some(format!("Database error: {}", e));
                self.mode = UiMode::Insights;
                return;
            }
        };

        let corpus_root = self.config.corpus_root.clone();
        let _ = config::log_message(&format!("Corpus root: {}", corpus_root.display()));

        let stash_root = match &self.config.stash_dir {
            Some(path) => {
                let _ = config::log_message(&format!("Stash root: {}", path.display()));
                path.clone()
            }
            None => {
                let _ = config::log_message("ERROR: stash-dir not configured");
                self.status_message = Some("stash-dir not configured".to_string());
                self.mode = UiMode::Insights;
                return;
            }
        };

        // Find duplicates BETWEEN selected directories (directory-set based)
        let _ = config::log_message("Calling find_duplicates_between_directories...");
        let clusters = match find_duplicates_between_directories(&db, &paths) {
            Ok(c) => {
                let _ = config::log_message(&format!("Found {} clusters", c.len()));
                c
            }
            Err(e) => {
                let _ = config::log_message(&format!("ERROR: find_duplicates failed: {}", e));
                self.status_message = Some(format!("Error finding duplicates: {}", e));
                self.mode = UiMode::Insights;
                return;
            }
        };

        if clusters.is_empty() {
            let _ = config::log_message("No duplicates found - returning to main menu");
            self.status_message = Some("No duplicates found between selected directories".to_string());
            self.mode = UiMode::Insights;
            return;
        }

        // Create session ID
        let session_id = uuid::Uuid::new_v4().to_string();
        let _ = config::log_message(&format!("Created session ID: {}", session_id));

        // Create cluster dialogue state with clusters directly
        let _ = config::log_message(&format!(
            "Creating ClusterDialogueState with {} clusters, corpus_root={}, stash_root={}",
            clusters.len(),
            corpus_root.display(),
            stash_root.display()
        ));

        self.cluster_dialogue = Some(dedup_flow::ClusterDialogueState::new_from_clusters(
            clusters,
            session_id,
            corpus_root.to_string_lossy().to_string(),
            stash_root.to_string_lossy().to_string(),
        ));
        self.mode = UiMode::ClusterDialogue;

        let _ = config::log_message("=== start_sleuthing_with_paths complete ===");
    }

    fn handle_tag_editor_action(&mut self, action: tag_editor::TagEditorAction) {
        match action {
            tag_editor::TagEditorAction::None => {}
            tag_editor::TagEditorAction::Exit => {
                self.tag_editor = None;
                self.mode = UiMode::Insights;
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
        use crate::corpus::db::PendingChange;

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
            self.status_message = Some("No changes to save".to_string());
            self.tag_editor = None;
            self.mode = UiMode::Insights;
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

        // Step 3: Create PendingChange records for each track
        let session_id = self.tag_edit_session_id.clone();
        let mut pending_changes: Vec<PendingChange> = Vec::new();

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

                pending_changes.push(PendingChange::tag_edit(
                    &session_id,
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
                let target_path = group.tracks.first()
                    .map(|t| {
                        format!("{}/{}/{}",
                            t.album_artist.as_deref()
                                .or(t.artist.as_deref())
                                .unwrap_or("Unknown Artist"),
                            t.album.as_deref().unwrap_or("Unknown Album"),
                            t.title.as_deref().unwrap_or("Unknown")
                        )
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
                    changes: pending_changes,
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

        // IMMEDIATE MODE: Execute changes now (non-workflow or SaveAll action)
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                self.status_message = Some(format!("Config error: {}", e));
                return;
            }
        };

        let db = match Database::open(&db_path) {
            Ok(d) => d,
            Err(e) => {
                self.status_message = Some(format!("Database error: {}", e));
                return;
            }
        };

        let report = match crate::ops::changes::execute_changes(&db, &pending_changes, false) {
            Ok(r) => r,
            Err(e) => {
                self.status_message = Some(format!("Execution error: {}", e));
                return;
            }
        };

        // Step 5: Check for stale deployments (if any path-affecting tags changed)
        let path_affecting_fields = ["artist", "album", "album_artist"];
        let has_path_changes = changes.iter().any(|c|
            path_affecting_fields.contains(&c.field_name.as_str())
        );

        let mut stale_count = 0;
        let mut resolved_conflicts = 0;
        if has_path_changes {
            // Cleanup any deployment conflicts that were resolved by these tag edits
            if let Ok(count) = crate::corpus::health::cleanup_resolved_deployment_conflicts(&self.config, &db) {
                resolved_conflicts = count;
            }

            if let Ok(statuses) = crate::ops::deploy::compute_full_deployment_status(&self.config, &db) {
                stale_count = statuses.iter().map(|s| s.stale.len()).sum();
            }
        }

        // Build status message
        let base_msg = if report.failed > 0 {
            format!(
                "Saved {} track(s), {} failed: {}",
                report.succeeded,
                report.failed,
                report.errors.first().unwrap_or(&String::new())
            )
        } else {
            format!("Saved {} track(s)", report.succeeded)
        };

        let stale_msg = if stale_count > 0 {
            format!(" ({} deployments now stale)", stale_count)
        } else {
            String::new()
        };

        let resolved_msg = if resolved_conflicts > 0 {
            format!(" ({} conflict(s) resolved)", resolved_conflicts)
        } else {
            String::new()
        };

        self.status_message = Some(format!("{}{}{}", base_msg, resolved_msg, stale_msg));

        if advance_to_next {
            // Move to next duplicate group if in duplicate workflow
            if let Some(ref mut editor) = self.tag_editor {
                if editor.current_group_idx.is_some() {
                    // If conflicts were resolved, refresh the groups list from DB
                    if resolved_conflicts > 0 {
                        if let Ok(fresh_groups) = load_deploy_conflict_groups(&db) {
                            editor.duplicate_groups = fresh_groups;
                            // Reset to first group in refreshed list
                            editor.current_group_idx = Some(0);
                        }
                    } else {
                        // No resolution, just advance the index
                        let current_idx = editor.current_group_idx.unwrap();
                        editor.current_group_idx = Some(current_idx + 1);
                    }

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
                        self.mode = UiMode::Insights;
                        self.status_message = Some(format!(
                            "{}{}{}. All conflicts processed.",
                            base_msg, resolved_msg, stale_msg
                        ));
                    }
                }
            }
        } else {
            self.tag_editor = None;
            self.mode = UiMode::Insights;
        }
    }

    fn handle_dialogue_result(&mut self, result: dialogue::DialogueResult) {
        match result {
            dialogue::DialogueResult::None => {}
            dialogue::DialogueResult::Continue => {}
            dialogue::DialogueResult::Finish(summary) => {
                self.dialogue = None;
                self.dialogue_summary = Some(summary);
                self.mode = UiMode::DialogueSummary;
            }
            dialogue::DialogueResult::Commit => {
                // Execute pending changes from accepted decisions
                if let Some(ref summary) = self.dialogue_summary {
                    if summary.pending_changes.is_empty() {
                        self.status_message = Some("No changes to commit".to_string());
                    } else {
                        let db_path = match crate::config::get_db_path() {
                            Ok(p) => p,
                            Err(e) => {
                                self.status_message = Some(format!("Config error: {}", e));
                                self.dialogue_summary = None;
                                self.mode = UiMode::Insights;
                                return;
                            }
                        };
                        match crate::corpus::db::Database::open(&db_path) {
                            Ok(db) => {
                                match crate::ops::changes::execute_changes(
                                    &db,
                                    &summary.pending_changes,
                                    false, // dry_run = false
                                ) {
                                    Ok(report) => {
                                        let msg = if report.failed > 0 {
                                            format!(
                                                "Committed {} changes ({} failed, {} skipped)",
                                                report.succeeded, report.failed, report.skipped
                                            )
                                        } else {
                                            format!("Committed {} changes", report.succeeded)
                                        };
                                        self.status_message = Some(msg);
                                        self.invalidate_tag_cloud();
                                    }
                                    Err(e) => {
                                        self.status_message =
                                            Some(format!("Commit error: {}", e));
                                    }
                                }
                            }
                            Err(e) => {
                                self.status_message = Some(format!("Database error: {}", e));
                            }
                        }
                    }
                } else {
                    self.status_message = Some("No summary state".to_string());
                }
                self.dialogue_summary = None;
                self.mode = UiMode::Insights;
            }
            dialogue::DialogueResult::Revert => {
                self.status_message = Some("Changes reverted".to_string());
                self.dialogue_summary = None;
                self.mode = UiMode::Insights;
            }
            dialogue::DialogueResult::Cancel => {
                self.dialogue = None;
                self.dialogue_summary = None;
                self.mode = UiMode::Insights;
            }
        }
    }

    fn handle_cluster_dialogue_action(&mut self, action: dedup_flow::ClusterDialogueAction) {
        match action {
            dedup_flow::ClusterDialogueAction::None => {}
            dedup_flow::ClusterDialogueAction::Continue => {}
            dedup_flow::ClusterDialogueAction::ShowBulkPrompt => {
                // Transition to bulk prompt - take ownership of session
                if let Some(cluster_dlg) = self.cluster_dialogue.take() {
                    let session = cluster_dlg.into_session();
                    self.bulk_prompt = Some(dedup_flow::BulkPromptState::new(session));
                    self.mode = UiMode::BulkReviewPrompt;
                }
            }
            dedup_flow::ClusterDialogueAction::ShowSessionReview => {
                // Transition to session review
                if let Some(cluster_dlg) = self.cluster_dialogue.take() {
                    let session = cluster_dlg.into_session();
                    self.session_review = Some(dedup_flow::SessionReviewState::new(session));
                    self.mode = UiMode::SessionReview;
                }
            }
            dedup_flow::ClusterDialogueAction::Cancel => {
                self.cluster_dialogue = None;
                self.mode = UiMode::Insights;
                self.status_message = Some("Deduplication cancelled".to_string());
            }
            dedup_flow::ClusterDialogueAction::StatusMessage(msg) => {
                self.status_message = Some(msg);
            }
        }
    }

    fn handle_bulk_prompt_action(&mut self, action: dedup_flow::BulkPromptAction) {
        match action {
            dedup_flow::BulkPromptAction::None => {}
            dedup_flow::BulkPromptAction::CommitBulk => {
                // Transition to session review
                if let Some(bulk_prompt) = self.bulk_prompt.take() {
                    let session = bulk_prompt.into_session();
                    self.session_review = Some(dedup_flow::SessionReviewState::new(session));
                    self.mode = UiMode::SessionReview;
                }
            }
            dedup_flow::BulkPromptAction::ContinueIndividual => {
                // Return to cluster dialogue to process remaining 2-file conflicts
                if let Some(bulk_prompt) = self.bulk_prompt.take() {
                    let session = bulk_prompt.into_session();
                    // Recreate cluster dialogue with current session state
                    // For now, mark bulk phase complete and continue
                    let corpus_root = self.config.corpus_root.to_string_lossy().to_string();
                    let stash_root = self.config.stash_dir
                        .as_ref()
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();

                    let mut new_state = dedup_flow::ClusterDialogueState::new(
                        session.conflict_sets.clone(),
                        session.session_id.clone(),
                        corpus_root,
                        stash_root,
                    );
                    // Restore session state
                    new_state.session = session;
                    new_state.session.bulk_phase_complete = true;

                    self.cluster_dialogue = Some(new_state);
                    self.mode = UiMode::ClusterDialogue;
                }
            }
            dedup_flow::BulkPromptAction::Cancel => {
                self.bulk_prompt = None;
                self.mode = UiMode::Insights;
                self.status_message = Some("Deduplication cancelled".to_string());
            }
        }
    }

    fn handle_session_review_action(&mut self, action: dedup_flow::SessionReviewAction) {
        match action {
            dedup_flow::SessionReviewAction::None => {}
            dedup_flow::SessionReviewAction::Commit => {
                // Execute all pending changes
                if let Some(ref session_review) = self.session_review {
                    let _ = config::log_message("=== SESSION REVIEW: COMMIT REQUESTED ===");

                    // Collect all pending changes from decisions
                    let all_changes: Vec<_> = session_review.session.decisions
                        .iter()
                        .flat_map(|d| d.pending_changes.clone())
                        .collect();

                    let _ = config::log_message(&format!(
                        "Total pending changes to execute: {}",
                        all_changes.len()
                    ));

                    // Log each change before execution
                    for (i, change) in all_changes.iter().enumerate() {
                        let _ = config::log_message(&format!(
                            "  Change {}: {:?} {} -> {}",
                            i + 1,
                            change.change_type,
                            change.source_path,
                            change.target_path.as_deref().unwrap_or("(none)")
                        ));
                    }

                    // Open database for change execution
                    let db_path = match config::get_db_path() {
                        Ok(p) => p,
                        Err(e) => {
                            let _ = config::log_message(&format!("ERROR: Failed to get db path: {}", e));
                            self.status_message = Some(format!("Config error: {}", e));
                            self.session_review = None;
                            self.mode = UiMode::Insights;
                            return;
                        }
                    };

                    let db = match Database::open(&db_path) {
                        Ok(db) => db,
                        Err(e) => {
                            let _ = config::log_message(&format!("ERROR: Failed to open database: {}", e));
                            self.status_message = Some(format!("Database error: {}", e));
                            self.session_review = None;
                            self.mode = UiMode::Insights;
                            return;
                        }
                    };

                    // Execute changes
                    let _ = config::log_message("Executing changes...");
                    match crate::ops::changes::execute_changes(&db, &all_changes, false) {
                        Ok(report) => {
                            let _ = config::log_message(&format!(
                                "Execution complete: {} succeeded, {} failed, {} skipped",
                                report.succeeded, report.failed, report.skipped
                            ));
                            for err in &report.errors {
                                let _ = config::log_message(&format!("  ERROR: {}", err));
                            }
                            self.status_message = Some(format!(
                                "Committed: {} succeeded, {} failed, {} skipped",
                                report.succeeded, report.failed, report.skipped
                            ));
                            self.invalidate_tag_cloud();
                        }
                        Err(e) => {
                            let _ = config::log_message(&format!("ERROR: Change execution failed: {}", e));
                            self.status_message = Some(format!("Execution error: {}", e));
                        }
                    }
                }
                self.session_review = None;
                self.mode = UiMode::Insights;
            }
            dedup_flow::SessionReviewAction::Preview => {
                // Dry run - show what would happen
                if let Some(ref session_review) = self.session_review {
                    let _ = config::log_message("=== SESSION REVIEW: PREVIEW (DRY RUN) ===");

                    let all_changes: Vec<_> = session_review.session.decisions
                        .iter()
                        .flat_map(|d| d.pending_changes.clone())
                        .collect();

                    let _ = config::log_message(&format!(
                        "Would execute {} changes:",
                        all_changes.len()
                    ));

                    for (i, change) in all_changes.iter().enumerate() {
                        let _ = config::log_message(&format!(
                            "  [DRY RUN] {}: {:?} {} -> {}",
                            i + 1,
                            change.change_type,
                            change.source_path,
                            change.target_path.as_deref().unwrap_or("(none)")
                        ));
                    }

                    self.status_message = Some(format!(
                        "Preview: {} files would be stashed (see log)",
                        all_changes.len()
                    ));
                }
            }
            dedup_flow::SessionReviewAction::Export => {
                // Export change list to file
                if let Some(ref session_review) = self.session_review {
                    match config::get_data_dir() {
                        Ok(data_dir) => {
                            let reports_dir = data_dir.join("reports");
                            if let Err(e) = std::fs::create_dir_all(&reports_dir) {
                                self.status_message = Some(format!("Failed to create reports dir: {}", e));
                                return;
                            }

                            let filename = format!(
                                "dedup_export_{}.json",
                                chrono::Utc::now().format("%Y%m%d_%H%M%S")
                            );
                            let path = reports_dir.join(&filename);

                            let export_data = serde_json::json!({
                                "session_id": &session_review.session.session_id,
                                "decisions_count": session_review.session.decisions.len(),
                                "decisions": session_review.session.decisions.iter().map(|d| {
                                    serde_json::json!({
                                        "keeper_dir": d.keeper_dir,
                                        "cluster_directories": &d.cluster.directory_set,
                                        "cluster_file_count": d.cluster.file_count,
                                        "pending_changes_count": d.pending_changes.len(),
                                    })
                                }).collect::<Vec<_>>(),
                                "exported_at": chrono::Utc::now().to_rfc3339(),
                            });

                            match std::fs::write(&path, serde_json::to_string_pretty(&export_data).unwrap_or_default()) {
                                Ok(_) => {
                                    self.status_message = Some(format!(
                                        "Exported {} decisions to {}",
                                        session_review.session.decisions.len(),
                                        path.display()
                                    ));
                                }
                                Err(e) => {
                                    self.status_message = Some(format!("Export failed: {}", e));
                                }
                            }
                        }
                        Err(e) => {
                            self.status_message = Some(format!("Config error: {}", e));
                        }
                    }
                }
            }
            dedup_flow::SessionReviewAction::Cancel => {
                let _ = config::log_message("=== SESSION REVIEW: CANCELLED ===");
                self.session_review = None;
                self.mode = UiMode::Insights;
                self.status_message = Some("Session cancelled, no changes made".to_string());
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

                    // Generate mutations for all libraries
                    let all_changes = crate::ops::deploy::all_deployment_statuses_to_mutations(
                        &preview.statuses,
                        &self.config,
                        &preview.session_id,
                    );

                    let _ = config::log_message(&format!(
                        "Generated {} deployment mutations",
                        all_changes.len()
                    ));

                    if all_changes.is_empty() {
                        self.status_message = Some("No deployment changes needed".to_string());
                        self.deployment_preview = None;
                        self.mode = UiMode::Insights;
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

                    let change_count = all_changes.len();

                    // Add operation to manager for background execution
                    let (id, tx, cancel_flag) = self.operations.add(
                        OperationType::ExecutingChanges {
                            session_id: preview.session_id.clone(),
                            change_count,
                        }
                    );

                    self.status_message = Some(format!(
                        "Deploying {} changes to {}... [{}]",
                        change_count, library_display, id
                    ));

                    // Clone config for the spawned thread
                    let config_clone = self.config.clone();

                    // Spawn background deployment thread
                    std::thread::spawn(move || {
                        use std::time::Instant;

                        let start = Instant::now();
                        let reporter = ProgressReporter::new(tx.clone(), cancel_flag.clone());

                        // Open database
                        let db_path = match config::get_db_path() {
                            Ok(p) => p,
                            Err(e) => {
                                reporter.error(format!("Config error: {}", e));
                                return;
                            }
                        };
                        let db = match Database::open(&db_path) {
                            Ok(db) => db,
                            Err(e) => {
                                reporter.error(format!("Database error: {}", e));
                                return;
                            }
                        };

                        // Execute deployment changes
                        let _ = config::log_message("Executing deployment changes...");
                        match crate::ops::changes::execute_changes(&db, &all_changes, false) {
                            Ok(report) => {
                                let _ = config::log_message(&format!(
                                    "Deployment complete: {} succeeded, {} failed, {} skipped",
                                    report.succeeded, report.failed, report.skipped
                                ));
                                for err in &report.errors {
                                    let _ = config::log_message(&format!("  ERROR: {}", err));
                                }

                                let result = OperationResult {
                                    succeeded: report.succeeded,
                                    skipped: report.skipped,
                                    failed: report.failed,
                                    duration: start.elapsed(),
                                    bytes_processed: None,
                                    errors: report.errors,
                                    data: None,
                                };
                                reporter.complete(result);

                                // Trigger heartbeat refresh in background
                                // Note: This spawns another thread - the heartbeat receiver
                                // will be checked by the main UI loop
                                let _ = spawn_heartbeat(&config_clone);
                            }
                            Err(e) => {
                                let _ = config::log_message(&format!("ERROR: Deployment failed: {}", e));
                                reporter.error(format!("Deployment error: {}", e));
                            }
                        }
                    });
                }
                // Return to main menu immediately - deployment runs in background
                self.deployment_preview = None;
                self.mode = UiMode::Insights;
            }
            deploy_flow::DeploymentPreviewAction::Cancel => {
                let _ = config::log_message("Deployment preview cancelled");
                self.deployment_preview = None;
                self.mode = UiMode::Insights;
                self.status_message = Some("Deployment cancelled".to_string());
            }
            deploy_flow::DeploymentPreviewAction::CycleNext => {
                // Deploy → Corpus Browser
                self.deployment_preview = None;
                self.start_corpus_browser();
            }
            deploy_flow::DeploymentPreviewAction::CyclePrev => {
                // Deploy → Insights
                self.deployment_preview = None;
                self.start_insights_view();
            }
        }
    }

    // ========================================================================
    // Artist Canonicalization Flow Handlers
    // (Delegates to canon_flow::coordinator)
    // ========================================================================

    fn start_canon_flow(&mut self) {
        canon_flow::coordinator::start(self);
    }

    fn start_genre_canon_flow(&mut self) {
        canon_flow::coordinator::start_genre(self);
    }

    fn handle_canon_cluster_action(&mut self, action: canon_flow::ClusterViewAction) {
        canon_flow::coordinator::handle_cluster_action(self, action);
    }

    fn handle_canon_review_action(&mut self, action: canon_flow::ReviewAction) {
        canon_flow::coordinator::handle_review_action(self, action);
    }

    // ========================================================================
    // Album Artist Resolution Flow Handlers
    // STUBBED OUT - Flow needs complete redesign
    // ========================================================================

    #[allow(dead_code)]
    fn start_album_artist_flow(&mut self) {
        // STUB: Flow disabled pending redesign
        self.status_message = Some("Album Artist Resolution: stubbed for redesign".to_string());
    }

    #[allow(dead_code)]
    fn handle_album_artist_phase_action(&mut self, _action: album_artist_flow::PhaseSelectorAction) {
        // STUB: No-op
    }

    #[allow(dead_code)]
    fn handle_album_artist_cluster_action(&mut self, _action: album_artist_flow::AlbumArtistClusterAction) {
        // STUB: No-op
    }

    #[allow(dead_code)]
    fn handle_album_artist_review_action(&mut self, _action: album_artist_flow::AlbumArtistReviewAction) {
        // STUB: No-op
    }

    #[allow(dead_code)]
    fn handle_album_artist_collation_action(&mut self, _action: album_artist_flow::CollationAction) {
        // STUB: No-op
    }

    #[allow(dead_code)]
    fn handle_album_artist_collation_review_action(
        &mut self,
        _action: album_artist_flow::CollationReviewAction,
    ) {
        // STUB: No-op
    }

    #[allow(dead_code)]
    fn handle_album_artist_population_action(&mut self, _action: album_artist_flow::PopulationAction) {
        // STUB: No-op
    }

    #[allow(dead_code)]
    fn handle_album_artist_population_review_action(
        &mut self,
        _action: album_artist_flow::PopulationReviewAction,
    ) {
        // STUB: No-op
    }

    // ========================================================================
    // Album Canonicalization Flow Handlers
    // (Delegates to album_flow::coordinator)
    // ========================================================================

    fn handle_album_cluster_action(&mut self, action: album_flow::AlbumClusterAction) {
        album_flow::coordinator::handle_cluster_action(self, action);
    }

    fn handle_album_review_action(&mut self, action: album_flow::AlbumReviewAction) {
        album_flow::coordinator::handle_review_action(self, action);
    }

    fn start_album_flow(&mut self) {
        album_flow::coordinator::start(self);
    }

    fn update_operation_progress(&mut self) {
        // Poll all tracked operations from OperationManager
        let completed = self.operations.poll_all();
        for (id, result, op_type) in completed {
            // Log operation summary
            log_operation_summary(&result, &op_type);

            // Handle tag flush completion - clear tag mismatches and invalidate tag cloud
            if let OperationType::ExecutingChanges { ref session_id, .. } = op_type {
                if session_id == "canon_tag_flush" {
                    self.clear_tag_mismatches_for_flushed(&result);
                }
                // Invalidate tag cloud after any change execution (tags may have changed)
                self.invalidate_tag_cloud();
            }

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

        // Handle legacy scan receiver (for backwards compatibility with existing scan code)
        let mut legacy_complete = false;
        let mut legacy_result: Option<(OperationResult, OperationType)> = None;

        if let Some(ref rx) = self.legacy_scan_receiver {
            while let Ok(message) = rx.try_recv() {
                // Convert ScanMessage to OperationMessage using the From impl
                let op_message: OperationMessage = message.into();

                match op_message {
                    OperationMessage::Progress(progress) => {
                        // Record throughput sample if bytes are tracked
                        if let Some(bytes) = progress.bytes_processed {
                            let now = Instant::now();
                            self.throughput_samples.push_back((now, bytes));

                            // Keep only samples from last 15 seconds (allows 8s rolling window + buffer)
                            let cutoff = now - std::time::Duration::from_secs(15);
                            while let Some((t, _)) = self.throughput_samples.front() {
                                if *t < cutoff {
                                    self.throughput_samples.pop_front();
                                } else {
                                    break;
                                }
                            }
                        }

                        // Update loading splash progress if in that mode
                        if self.mode == UiMode::LoadingSplash {
                            if let Some(ref mut splash) = self.loading_splash_state {
                                splash.set_progress(progress.completed_items, progress.total_items);
                            }
                        }

                        // Store progress in a "virtual" tracked operation for rendering
                        self.operations.update_legacy_progress(progress);
                    }
                    OperationMessage::Complete(result) => {
                        // Format completion message
                        self.status_message = Some(if result.succeeded == 0 && result.skipped > 0 {
                            format!(
                                "Complete! {} skipped (unchanged) in {:.1}s",
                                result.skipped,
                                result.duration.as_secs_f64()
                            )
                        } else {
                            let bytes_str = result
                                .bytes_processed
                                .map(|b| format!(" ({:.2} GB)", b as f64 / 1_000_000_000.0))
                                .unwrap_or_default();
                            format!(
                                "Complete! {} succeeded{} in {:.1}s",
                                result.succeeded,
                                bytes_str,
                                result.duration.as_secs_f64()
                            )
                        });

                        // Store result for logging
                        if let Some(ref op_type) = self.legacy_scan_type {
                            legacy_result = Some((result, op_type.clone()));
                        }
                        legacy_complete = true;
                    }
                    OperationMessage::Error(err) => {
                        self.status_message = Some(format!("Operation error: {}", err));
                        legacy_complete = true;
                    }
                    OperationMessage::Cancelled => {
                        self.status_message = Some("Operation cancelled".to_string());
                        legacy_complete = true;
                    }
                }
            }
        }

        // Log legacy operation summary after clearing receiver
        if let Some((ref result, ref op_type)) = legacy_result {
            log_operation_summary(result, op_type);

            // Run canonicalization detection after corpus scan completes
            if let OperationType::Scanning { source_name } = op_type {
                if source_name == "corpus" && result.failed == 0 {
                    // Run in background to not block UI
                    std::thread::spawn(|| {
                        if let Ok(db_path) = config::get_db_path() {
                            if let Ok(db) = Database::open(&db_path) {
                                match crate::corpus::detect_and_store_canonicalizations(&db) {
                                    Ok(count) if count > 0 => {
                                        let _ = config::log_message(&format!(
                                            "Post-scan: detected {} new canonicalization issues",
                                            count
                                        ));
                                    }
                                    Err(e) => {
                                        let _ = config::log_message(&format!(
                                            "Post-scan canonicalization error: {}",
                                            e
                                        ));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    });
                }
            }
        }

        if legacy_complete {
            self.legacy_scan_receiver = None;
            self.legacy_scan_type = None;
            self.operations.clear_legacy_progress();
            self.throughput_samples.clear();
            // Refresh corpus summary after operation completes
            self.refresh_corpus_summary();

            // Transition from loading splash to target mode (e.g., Insights)
            if self.mode == UiMode::LoadingSplash {
                if let Some(ref splash) = self.loading_splash_state {
                    let target = splash.target_mode;
                    self.loading_splash_state = None;
                    self.mode = target;

                    // After scan completes, we need heartbeat data for Insights
                    // Start a heartbeat if we don't have one
                    if self.heartbeat_result.is_none() && self.heartbeat_receiver.is_none() {
                        let rx = spawn_heartbeat(&self.config);
                        self.heartbeat_receiver = Some(rx);
                        self.eye.set_heartbeat_pending(true);
                        // Show new splash for heartbeat
                        self.loading_splash_state = Some(LoadingSplashState::heartbeat());
                        self.mode = UiMode::LoadingSplash;
                    }
                }
            }
        }
    }

    /// Clear tag mismatches for successfully flushed paths
    fn clear_tag_mismatches_for_flushed(&self, result: &OperationResult) {
        use crate::ops::operation::ResultData;

        // Extract flushed paths from result data
        let flushed_paths = match &result.data {
            Some(ResultData::TagFlush { flushed_paths }) => flushed_paths.clone(),
            _ => Vec::new(),
        };

        if flushed_paths.is_empty() {
            return;
        }

        // Open database and clear mismatches
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return,
        };
        let db = match Database::open(&db_path) {
            Ok(d) => d,
            Err(_) => return,
        };

        let mut cleared = 0;
        for path in &flushed_paths {
            if db.clear_tag_mismatches_by_path(path).is_ok() {
                cleared += 1;
            }
        }

        let _ = config::log_message(&format!(
            "Cleared tag mismatches for {}/{} flushed paths",
            cleared,
            flushed_paths.len()
        ));
    }

    /// Refresh the cached corpus summary for the info panel
    // TODO: This updates main_menu state which is no longer displayed.
    // Consider removing or repurposing for Insights view. See main_menu.rs.
    fn refresh_corpus_summary(&mut self) {
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return,
        };
        let db = match Database::open(&db_path) {
            Ok(d) => d,
            Err(_) => return,
        };
        if let Ok(summary) = db.get_corpus_summary() {
            self.main_menu.set_corpus_summary(summary);
        }
    }

    /// Check for heartbeat completion
    fn update_heartbeat(&mut self) {
        if let Some(ref rx) = self.heartbeat_receiver {
            if let Ok(result) = rx.try_recv() {
                // TODO: main_menu.set_heartbeat_result updates state no longer displayed.
                // Consider removing. See main_menu.rs.
                self.main_menu.set_heartbeat_result(result.clone());
                self.heartbeat_result = Some(result.clone());
                self.heartbeat_receiver = None;
                self.eye.set_heartbeat_pending(false);

                // Transition from loading splash to target mode
                if self.mode == UiMode::LoadingSplash {
                    if let Some(ref splash) = self.loading_splash_state {
                        let target = splash.target_mode;
                        self.loading_splash_state = None;
                        self.mode = target;
                    }
                    // Initialize insights view with heartbeat data
                    self.initialize_insights_with_heartbeat(result);
                } else if self.mode == UiMode::Insights && self.insights_view.is_none() {
                    // If we're already in Insights mode and view not yet initialized
                    self.initialize_insights_with_heartbeat(result);
                }
            }
        }
    }

    /// Initialize insights view with a completed heartbeat result
    fn initialize_insights_with_heartbeat(&mut self, heartbeat: HeartbeatResult) {
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                self.status_message = Some(format!("Config error: {}", e));
                return;
            }
        };

        let db = match Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                self.status_message = Some(format!("Database error: {}", e));
                return;
            }
        };

        let mut view = insights_view::InsightsViewState::new();
        view.initialize(&db, db_path.to_str().unwrap_or(""), heartbeat);
        self.insights_view = Some(view);
    }

    // ========================================================================
    // Tag Cloud (canonicalization detection cache)
    // ========================================================================

    /// Start building tag cloud in background (non-blocking).
    #[allow(dead_code)]
    fn start_tag_cloud_build(&mut self) {
        if self.tag_cloud_receiver.is_none() {
            self.tag_cloud_receiver = Some(crate::corpus::health::spawn_tag_cloud_build());
        }
    }

    /// Check if tag cloud build completed (call in update loop).
    fn update_tag_cloud(&mut self) {
        if let Some(ref rx) = self.tag_cloud_receiver {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(cloud) => {
                        let _ = crate::config::log_message(&format!(
                            "TagCloud ready: {} tracks, {} artist collisions, {} genre collisions",
                            cloud.track_count(),
                            cloud.artist_collision_count(),
                            cloud.genre_collision_count()
                        ));
                        self.tag_cloud = Some(cloud);
                    }
                    Err(e) => {
                        let _ = crate::config::log_message(&format!(
                            "TagCloud build failed: {}",
                            e
                        ));
                    }
                }
                self.tag_cloud_receiver = None;
            }
        }
    }

    /// Invalidate the tag cloud (call after mutations).
    pub fn invalidate_tag_cloud(&mut self) {
        self.tag_cloud = None;
        self.tag_cloud_receiver = None;
    }

    /// Get the current tag cloud, if available.
    #[allow(dead_code)]
    pub fn tag_cloud(&self) -> Option<&crate::corpus::health::TagCloud> {
        self.tag_cloud.as_ref()
    }

    // ========================================================================
    // Directory Tag Editor
    // ========================================================================

    fn start_directory_tag_editor(&mut self, directory: &std::path::Path) {
        self.corpus_browser = None;
        self.directory_tag_editor = Some(tag_editor::DirectoryTagEditorState::start_gathering(
            directory.to_path_buf(),
        ));
        self.directory_tag_editor_modal = None;
        self.mode = UiMode::DirectoryTagEditor;
    }

    fn update_directory_tag_editor(&mut self) {
        if let Some(ref mut editor) = self.directory_tag_editor {
            if editor.is_gathering() {
                let complete = editor.poll_gathering();
                if complete {
                    self.status_message = Some(format!(
                        "Loaded {} files from {}",
                        editor.files.len(),
                        editor.current_directory.display()
                    ));
                }
            }
        }
    }

    fn handle_directory_tag_editor_action(
        &mut self,
        action: tag_editor::types::DirectoryTagEditorAction,
    ) {
        use tag_editor::types::DirectoryTagEditorAction;

        match action {
            DirectoryTagEditorAction::None => {}
            DirectoryTagEditorAction::ShowModal(modal) => {
                self.directory_tag_editor_modal = Some(modal);
            }
            DirectoryTagEditorAction::SaveAll => {
                self.save_directory_tag_changes(false);
            }
            DirectoryTagEditorAction::SaveAndNext => {
                self.save_directory_tag_changes(true);
            }
            DirectoryTagEditorAction::Exit => {
                self.directory_tag_editor = None;
                self.directory_tag_editor_modal = None;
                self.mode = UiMode::CorpusBrowser;
                // Restore corpus browser if it was set
                if self.corpus_browser.is_none() {
                    self.start_corpus_browser();
                }
            }
            DirectoryTagEditorAction::StatusMessage(msg) => {
                self.status_message = Some(msg);
            }
            DirectoryTagEditorAction::SwitchDirectory(next) => {
                if let Some(ref editor) = self.directory_tag_editor {
                    if let Some(new_dir) = editor.switch_to_sibling(next).clone() {
                        // Start gathering for new directory
                        self.directory_tag_editor = Some(
                            tag_editor::DirectoryTagEditorState::start_gathering(new_dir),
                        );
                    } else {
                        let msg = if next {
                            "No more directories after this one"
                        } else {
                            "No more directories before this one"
                        };
                        self.status_message = Some(msg.to_string());
                    }
                }
            }
        }
    }

    fn handle_directory_tag_editor_modal_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;
        use tag_editor::types::DirectoryTagEditorModal;

        let modal = match &self.directory_tag_editor_modal {
            Some(m) => m,
            None => return,
        };

        match modal {
            DirectoryTagEditorModal::ChangePreview { scroll_offset, save_and_next } => {
                let mut scroll = *scroll_offset;
                let save_next = *save_and_next;

                match key.code {
                    KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                        self.directory_tag_editor_modal = None;
                        if save_next {
                            self.save_directory_tag_changes(true);
                        } else {
                            self.save_directory_tag_changes(false);
                        }
                    }
                    KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                        self.directory_tag_editor_modal = None;
                    }
                    KeyCode::Up => {
                        scroll = scroll.saturating_sub(1);
                        self.directory_tag_editor_modal =
                            Some(DirectoryTagEditorModal::ChangePreview {
                                scroll_offset: scroll,
                                save_and_next: save_next,
                            });
                    }
                    KeyCode::Down => {
                        scroll += 1;
                        self.directory_tag_editor_modal =
                            Some(DirectoryTagEditorModal::ChangePreview {
                                scroll_offset: scroll,
                                save_and_next: save_next,
                            });
                    }
                    _ => {}
                }
            }
            DirectoryTagEditorModal::UnsavedChanges { going_next } => {
                let next = *going_next;
                match key.code {
                    KeyCode::Char('s') | KeyCode::Char('S') => {
                        // Save & Switch
                        self.directory_tag_editor_modal = None;
                        self.save_directory_tag_changes(false);
                        // After saving, switch directory
                        self.handle_directory_tag_editor_action(
                            tag_editor::types::DirectoryTagEditorAction::SwitchDirectory(next),
                        );
                    }
                    KeyCode::Char('d') | KeyCode::Char('D') => {
                        // Discard & Switch
                        self.directory_tag_editor_modal = None;
                        self.handle_directory_tag_editor_action(
                            tag_editor::types::DirectoryTagEditorAction::SwitchDirectory(next),
                        );
                    }
                    KeyCode::Char('c') | KeyCode::Char('C') | KeyCode::Esc => {
                        // Cancel
                        self.directory_tag_editor_modal = None;
                    }
                    _ => {}
                }
            }
        }
    }

    fn save_directory_tag_changes(&mut self, switch_to_next: bool) {
        use crate::corpus::metadata;

        let editor = match &self.directory_tag_editor {
            Some(e) => e,
            None => return,
        };

        let changes = editor.compute_changes();
        if changes.is_empty() {
            self.status_message = Some("No changes to save".to_string());
            return;
        }

        let mut success_count = 0;
        let mut error_count = 0;

        for file in &editor.files {
            // Build the new tags for this file
            let mut new_tags: Vec<(String, String)> = Vec::new();

            for change in &changes {
                new_tags.push((change.field_name.clone(), change.new_value.clone()));
            }

            // Write the tags (track_id=0 and empty session skips DB updates)
            if let Err(e) = metadata::write_tags(&file.path, &new_tags, 0, "") {
                let _ = crate::config::log_message(&format!(
                    "Error writing tags to {}: {}",
                    file.path.display(),
                    e
                ));
                error_count += 1;
            } else {
                success_count += 1;
            }
        }

        self.status_message = Some(format!(
            "Updated {} files ({} errors)",
            success_count, error_count
        ));

        if switch_to_next {
            if let Some(ref editor) = self.directory_tag_editor {
                if let Some(new_dir) = editor.switch_to_sibling(true).clone() {
                    self.directory_tag_editor = Some(
                        tag_editor::DirectoryTagEditorState::start_gathering(new_dir),
                    );
                } else {
                    self.status_message = Some("Saved. No more sibling directories.".to_string());
                }
            }
        }
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
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                self.status_message = Some(format!("Config error: {}", e));
                self.deploy_conflict_review = None;
                self.deploy_conflict_accumulated.clear();
                self.mode = UiMode::Insights;
                return;
            }
        };

        let db = match Database::open(&db_path) {
            Ok(d) => d,
            Err(e) => {
                self.status_message = Some(format!("Database error: {}", e));
                self.deploy_conflict_review = None;
                self.deploy_conflict_accumulated.clear();
                self.mode = UiMode::Insights;
                return;
            }
        };

        // Gather all pending changes from accumulated groups
        let all_changes: Vec<_> = self.deploy_conflict_accumulated
            .iter()
            .flat_map(|g| g.changes.clone())
            .collect();

        if all_changes.is_empty() {
            self.status_message = Some("No changes to commit".to_string());
            self.deploy_conflict_review = None;
            self.deploy_conflict_accumulated.clear();
            self.mode = UiMode::Insights;
            return;
        }

        // Execute all changes in bulk
        let report = match crate::ops::changes::execute_changes(&db, &all_changes, false) {
            Ok(r) => r,
            Err(e) => {
                self.status_message = Some(format!("Execution error: {}", e));
                self.deploy_conflict_review = None;
                self.deploy_conflict_accumulated.clear();
                self.mode = UiMode::Insights;
                return;
            }
        };

        // Cleanup resolved conflicts
        let resolved_count = crate::corpus::health::cleanup_resolved_deployment_conflicts(&self.config, &db)
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

        self.status_message = Some(format!("{}{}", base_msg, resolved_msg));

        // Clear state
        self.deploy_conflict_review = None;
        self.deploy_conflict_accumulated.clear();
        self.mode = UiMode::Insights;
    }

    fn discard_deploy_conflict_changes(&mut self) {
        let groups_count = self.deploy_conflict_accumulated.len();
        self.status_message = Some(format!(
            "Discarded changes from {} conflict group(s)",
            groups_count
        ));
        self.deploy_conflict_review = None;
        self.deploy_conflict_accumulated.clear();
        self.mode = UiMode::Insights;
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
        main_menu: &mut app.main_menu,
        tag_editor: app.tag_editor.as_mut(),
        tag_editor_modal: app.tag_editor_modal.as_ref(),
        dir_browser: app.dir_browser.as_mut(),
        corpus_browser: app.corpus_browser.as_mut(),
        dialogue: app.dialogue.as_mut(),
        dialogue_summary: app.dialogue_summary.as_mut(),
        cluster_dialogue: app.cluster_dialogue.as_mut(),
        bulk_prompt: app.bulk_prompt.as_mut(),
        session_review: app.session_review.as_mut(),
        drop_missing_state: app.drop_missing_state.as_ref(),
        deployment_preview: app.deployment_preview.as_mut(),
        canon_cluster_view: app.canon_cluster_view.as_mut(),
        canon_session_review: app.canon_session_review.as_mut(),
        canon_commit_modal_state: app.canon_commit_modal_state.as_ref(),
        album_artist_phase_selector: app.album_artist_phase_selector.as_ref(),
        album_artist_cluster_view: app.album_artist_cluster_view.as_mut(),
        album_artist_collation: app.album_artist_collation.as_mut(),
        album_artist_collation_review: app.album_artist_collation_review.as_mut(),
        album_artist_population: app.album_artist_population.as_mut(),
        album_artist_population_review: app.album_artist_population_review.as_mut(),
        album_artist_review: app.album_artist_review.as_mut(),
        album_cluster_view: app.album_cluster_view.as_mut(),
        album_review: app.album_review.as_mut(),
        directory_tag_editor: app.directory_tag_editor.as_mut(),
        directory_tag_editor_modal: app.directory_tag_editor_modal.as_ref(),
        exit_confirm_modal_state: app.exit_confirm_modal_state.as_ref(),
        loading_splash_state: app.loading_splash_state.as_ref(),
        deploy_conflict_review: app.deploy_conflict_review.as_ref(),
        insights_view: app.insights_view.as_mut(),
        heartbeat_result: app.heartbeat_result.as_ref(),
        heartbeat_pending: app.heartbeat_receiver.is_some(),
        eye: &app.eye,
        throughput_samples: &app.throughput_samples,
        active_operations: app.operations.all_for_display().into_iter().map(|(t, p)| (t.clone(), p.clone())).collect(),
    };
    render::render(f, &mut ctx);
}



// ============================================================================
// Startup Health Version Check
// ============================================================================

/// Check health data version and trigger rebuild if needed.
/// This runs at startup before the main event loop.
fn check_and_maybe_rebuild_health(app: &mut App) {
    // Open database to check version
    let db_path = match config::get_db_path() {
        Ok(p) => p,
        Err(_) => return, // No DB yet, nothing to rebuild
    };

    let db = match Database::open(&db_path) {
        Ok(db) => db,
        Err(_) => return, // Can't open DB, skip check
    };

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
fn check_and_run_migrations<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
) -> Result<()> {
    use ratatui::layout::{Alignment, Rect};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};

    // Open database
    let db_path = match config::get_db_path() {
        Ok(p) => p,
        Err(_) => return Ok(()), // No database path configured, skip migrations
    };

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

    // Render blocking migration dialog
    terminal.draw(|f| {
        let area = f.area();

        // Center the dialog
        let dialog_width = 60.min(area.width.saturating_sub(4));
        let dialog_height = (migration_count as u16 + 8).min(area.height.saturating_sub(4));

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
            ratatui::text::Line::from("Database migrations required:").style(
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            ratatui::text::Line::from(""),
        ];

        for desc in &pending {
            lines.push(ratatui::text::Line::from(format!("  • {}", desc)));
        }

        lines.push(ratatui::text::Line::from(""));
        lines.push(
            ratatui::text::Line::from("Running migrations... please wait.")
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
            .alignment(Alignment::Left);

        f.render_widget(paragraph, dialog_area);
    })?;

    // Execute migrations
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

    // Check for first-time startup (no corpus indexed) and auto-scan
    app.check_first_time_startup();

    // Spawn heartbeat check if enabled (skip if first-time scan is running)
    if app.config.opinions.startup.heartbeat_on_startup && !app.first_time_scan_active {
        let rx = spawn_heartbeat(&app.config);
        app.heartbeat_receiver = Some(rx);
        app.eye.set_heartbeat_pending(true);
        // Show loading splash while waiting for heartbeat
        app.loading_splash_state = Some(LoadingSplashState::heartbeat());
        app.mode = UiMode::LoadingSplash;
    } else if app.first_time_scan_active {
        // Show loading splash for first-time scan
        app.loading_splash_state = Some(LoadingSplashState::scan(true));
        app.mode = UiMode::LoadingSplash;
    }

    app.refresh_corpus_summary();
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
        app.eye.update();
        app.update_operation_progress();
        app.update_heartbeat();
        app.update_tag_cloud();
        app.update_directory_tag_editor();

        // Check if eye blink triggered a heartbeat (d20 >= 13)
        if let Some(roll_result) = app.eye.take_heartbeat_trigger() {
            if app.heartbeat_receiver.is_none() {
                // Both Normal (14-20) and Expensive (13) trigger heartbeat
                // Expensive roll result can also run cleanup operations after heartbeat
                let rx = spawn_heartbeat(&app.config);
                app.heartbeat_receiver = Some(rx);
                app.eye.set_heartbeat_pending(true);

                // Store roll result for potential expensive operations after heartbeat
                app.last_heartbeat_roll = Some(roll_result);
            }
        }

        terminal.draw(|f| render(f, app))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                // Global quit shortcut
                if key.code == KeyCode::Char('c')
                    && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                {
                    break;
                }
                app.handle_key(key);
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
