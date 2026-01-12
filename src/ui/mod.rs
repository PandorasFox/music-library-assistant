//! TUI Module
//!
//! Modular UI components for the Music Library Assistant.

pub mod app;
pub mod canon_flow;
pub mod dedup_flow;
pub mod deploy_flow;
pub mod dialogue;
pub mod dir_browser;
pub mod drop_flow;
pub mod flows;
pub mod helpers;
pub mod main_menu;
pub mod picker;
pub mod render;
pub mod tag_editor;

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
use crate::db::Database;
use crate::ops::operation::{
    log_operation_summary, OperationManager, OperationMessage, OperationProgress,
    OperationResult, OperationType, ProgressReporter,
};
use crate::ops::{reports, scanner};
use crate::progress::ScanMessage;

use app::EyeAnimation;
use main_menu::{BackgroundTask, CommandAction, MainMenuState, MenuAction, ReportType, TransitionTarget};

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
}

/// Re-export from drop_flow module
pub(crate) use drop_flow::DropMissingState;

/// State for the exit confirmation modal.
/// Default selection is "No" (stay in application).
/// Pressing Esc/Enter/Space when selected_no=true returns to main menu.
#[derive(Debug, Clone)]
pub(crate) struct ExitConfirmModalState {
    /// True = "No" selected (default), False = "Yes" selected
    pub selected_no: bool,
}

impl Default for ExitConfirmModalState {
    fn default() -> Self {
        Self { selected_no: true }
    }
}

/// State for the canon commit modal with two options
#[derive(Debug, Clone)]
pub(crate) struct CanonCommitModalState {
    /// Currently selected option (0 = main menu, 1 = deployment review)
    selected_option: usize,
    /// Number of tracks updated in the database
    tracks_updated: usize,
    /// Number of deployments that are now stale
    stale_deployments: usize,
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

/// Context for what operation launched the directory browser.
/// The browser returns paths; this tells us what to do with them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserContext {
    /// Fingerprint-based deduplication
    Sleuthing,
}

/// Main application state
struct App {
    config: Config,
    should_quit: bool,
    status_message: Option<String>,

    // UI mode and state
    mode: UiMode,
    main_menu: MainMenuState,
    tag_editor: Option<tag_editor::TagEditorState>,
    dir_browser: Option<dir_browser::DirBrowserState>,
    browser_context: Option<BrowserContext>,
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
    // Exit confirmation modal
    exit_confirm_modal_state: Option<ExitConfirmModalState>,

    // Background operations (supports multiple concurrent)
    operations: OperationManager,
    // Legacy single-operation receiver for scan (uses ScanMessage)
    legacy_scan_receiver: Option<mpsc::Receiver<ScanMessage>>,
    legacy_scan_type: Option<OperationType>,

    // Startup heartbeat
    heartbeat_result: Option<HeartbeatResult>,
    heartbeat_receiver: Option<mpsc::Receiver<HeartbeatResult>>,

    // Throughput tracking for rolling average (timestamp, bytes_processed)
    throughput_samples: VecDeque<(Instant, u64)>,

    // Eye animation
    eye: EyeAnimation,

    // Tag editing session
    #[allow(dead_code)]
    tag_edit_session_id: String,
}

impl App {
    fn new(config: Config) -> Self {
        Self {
            main_menu: MainMenuState::new(&config),
            config,
            should_quit: false,
            status_message: None,
            mode: UiMode::MainMenu,
            tag_editor: None,
            dir_browser: None,
            browser_context: None,
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
            exit_confirm_modal_state: None,
            operations: OperationManager::new(),
            legacy_scan_receiver: None,
            legacy_scan_type: None,
            heartbeat_result: None,
            heartbeat_receiver: None,
            throughput_samples: VecDeque::with_capacity(100),
            eye: EyeAnimation::default(),
            tag_edit_session_id: uuid::Uuid::new_v4().to_string(),
        }
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        match self.mode {
            UiMode::MainMenu => {
                let action = self.main_menu.handle_key(key);
                self.handle_menu_action(action);
            }
            UiMode::TagEditor => {
                if let Some(ref mut editor) = self.tag_editor {
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
                                    self.mode = UiMode::MainMenu;
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
                            self.mode = UiMode::MainMenu;
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
                                // "No" selected - return to main menu with Exit highlighted
                                self.exit_confirm_modal_state = None;
                                self.mode = UiMode::MainMenu;
                            } else {
                                // "Yes" selected - actually quit
                                self.should_quit = true;
                            }
                        }
                        KeyCode::Esc => {
                            // Esc returns to main menu with Exit highlighted
                            self.exit_confirm_modal_state = None;
                            self.mode = UiMode::MainMenu;
                        }
                        KeyCode::Char('y') | KeyCode::Char('Y') => {
                            // 'y' confirms exit
                            self.should_quit = true;
                        }
                        KeyCode::Char('n') | KeyCode::Char('N') => {
                            // 'n' cancels
                            self.exit_confirm_modal_state = None;
                            self.mode = UiMode::MainMenu;
                        }
                        _ => {}
                    }
                }
            }
        }
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
                // If operations are running, show confirmation modal
                if !self.operations.is_empty() {
                    self.exit_confirm_modal_state = Some(ExitConfirmModalState::default());
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
            ReportType::Quality => "quality",
            ReportType::Duplicates => "duplicates",
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
                    ("duplicates", reports_dir.join("duplicates.txt")),
                    ("quality", reports_dir.join("quality.txt")),
                    ("deployment", reports_dir.join("deployment.txt")),
                    ("known_variants", reports_dir.join("known_variants.txt")),
                ],
                ReportType::Legacy => vec![
                    ("legacy", reports_dir.join("legacy.txt")),
                ],
                ReportType::Deployment => vec![
                    ("deployment", reports_dir.join("deployment.txt")),
                ],
                ReportType::Quality => vec![
                    ("quality", reports_dir.join("quality.txt")),
                ],
                ReportType::Duplicates => vec![
                    ("duplicates", reports_dir.join("duplicates.txt")),
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
                    "duplicates" => reports::generate_duplicate_report(&path),
                    "quality" => reports::generate_quality_report(&path),
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
            use crate::corpus::{detect_fingerprint_issues, detect_metadata_issues};
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
            let mut issues_found = 0;
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
                    issues_found += issues.len();
                }
                let _ = detect_metadata_issues(&db, track);

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
                    self.mode = UiMode::MainMenu;
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
                self.mode = UiMode::MainMenu;
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
        self.mode = UiMode::MainMenu;
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
        }
    }

    fn start_tag_editor(&mut self) {
        // Load duplicate groups from database
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

        // Get unresolved duplicate group IDs and load their tracks
        match db.get_unresolved_duplicate_groups() {
            Ok(group_ids) if !group_ids.is_empty() => {
                // Load tracks for each group
                let mut all_groups: Vec<tag_editor::DuplicateGroupInfo> = Vec::new();
                for group_id in &group_ids {
                    match db.get_duplicate_group_tracks(*group_id) {
                        Ok(tracks) if !tracks.is_empty() => {
                            all_groups.push(tag_editor::DuplicateGroupInfo {
                                group_id: *group_id,
                                tracks,
                                resolved: false,
                            });
                        }
                        Ok(_) => {} // Empty group, skip
                        Err(e) => {
                            self.status_message = Some(format!("Error loading group {}: {}", group_id, e));
                            return;
                        }
                    }
                }

                if all_groups.is_empty() {
                    self.status_message = Some("No duplicate groups with tracks found".to_string());
                    return;
                }

                // Start with first group's tracks
                let first_tracks = all_groups[0].tracks.clone();
                self.tag_editor = Some(tag_editor::TagEditorState::new(first_tracks, all_groups));
                self.mode = UiMode::TagEditor;
            }
            Ok(_) => {
                self.status_message = Some("No unresolved duplicate groups found".to_string());
            }
            Err(e) => {
                self.status_message = Some(format!("Error loading duplicates: {}", e));
            }
        }
    }

    fn start_decision_flow(&mut self) {
        // Use fingerprint-based deduplication with directory set clustering
        use crate::deduplication::find_fingerprint_duplicates;

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

    fn handle_dir_browser_action(&mut self, action: dir_browser::DirBrowserAction) {
        match action {
            dir_browser::DirBrowserAction::None => {}
            dir_browser::DirBrowserAction::Cancel => {
                self.dir_browser = None;
                self.browser_context = None;
                self.mode = UiMode::MainMenu;
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
                        self.mode = UiMode::MainMenu;
                    }
                }
            }
        }
    }

    fn start_sleuthing_with_paths(&mut self, paths: Vec<std::path::PathBuf>) {
        use crate::deduplication::find_duplicates_between_directories;

        let _ = config::log_message("=== start_sleuthing_with_paths called ===");
        for (i, path) in paths.iter().enumerate() {
            let _ = config::log_message(&format!("  path[{}]: {}", i, path.display()));
        }

        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                let _ = config::log_message(&format!("ERROR: Failed to get db path: {}", e));
                self.status_message = Some(format!("Config error: {}", e));
                self.mode = UiMode::MainMenu;
                return;
            }
        };
        let _ = config::log_message(&format!("Database path: {}", db_path.display()));

        let db = match Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                let _ = config::log_message(&format!("ERROR: Failed to open database: {}", e));
                self.status_message = Some(format!("Database error: {}", e));
                self.mode = UiMode::MainMenu;
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
                self.mode = UiMode::MainMenu;
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
                self.mode = UiMode::MainMenu;
                return;
            }
        };

        if clusters.is_empty() {
            let _ = config::log_message("No duplicates found - returning to main menu");
            self.status_message = Some("No duplicates found between selected directories".to_string());
            self.mode = UiMode::MainMenu;
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
                self.mode = UiMode::MainMenu;
            }
            tag_editor::TagEditorAction::SaveAll => {
                self.save_tag_editor_changes(false);
            }
            tag_editor::TagEditorAction::SaveAndNext => {
                self.save_tag_editor_changes(true);
            }
            tag_editor::TagEditorAction::ShowModal(_modal) => {
                // TODO: Handle modals
                self.status_message = Some("Modal display (TODO)".to_string());
            }
            tag_editor::TagEditorAction::StatusMessage(msg) => {
                self.status_message = Some(msg);
            }
        }
    }

    fn save_tag_editor_changes(&mut self, _advance_to_next: bool) {
        // TODO: Implement actual saving
        self.status_message = Some("Saving changes (TODO)".to_string());
        self.tag_editor = None;
        self.mode = UiMode::MainMenu;
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
                self.status_message = Some("Changes committed (TODO)".to_string());
                self.dialogue_summary = None;
                self.mode = UiMode::MainMenu;
            }
            dialogue::DialogueResult::Revert => {
                self.status_message = Some("Changes reverted".to_string());
                self.dialogue_summary = None;
                self.mode = UiMode::MainMenu;
            }
            dialogue::DialogueResult::Cancel => {
                self.dialogue = None;
                self.dialogue_summary = None;
                self.mode = UiMode::MainMenu;
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
                self.mode = UiMode::MainMenu;
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
                self.mode = UiMode::MainMenu;
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
                            self.mode = UiMode::MainMenu;
                            return;
                        }
                    };

                    let db = match Database::open(&db_path) {
                        Ok(db) => db,
                        Err(e) => {
                            let _ = config::log_message(&format!("ERROR: Failed to open database: {}", e));
                            self.status_message = Some(format!("Database error: {}", e));
                            self.session_review = None;
                            self.mode = UiMode::MainMenu;
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
                        }
                        Err(e) => {
                            let _ = config::log_message(&format!("ERROR: Change execution failed: {}", e));
                            self.status_message = Some(format!("Execution error: {}", e));
                        }
                    }
                }
                self.session_review = None;
                self.mode = UiMode::MainMenu;
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
                self.status_message = Some("Export change list (TODO)".to_string());
            }
            dedup_flow::SessionReviewAction::Cancel => {
                let _ = config::log_message("=== SESSION REVIEW: CANCELLED ===");
                self.session_review = None;
                self.mode = UiMode::MainMenu;
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
                        self.mode = UiMode::MainMenu;
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
                self.mode = UiMode::MainMenu;
            }
            deploy_flow::DeploymentPreviewAction::Cancel => {
                let _ = config::log_message("Deployment preview cancelled");
                self.deployment_preview = None;
                self.mode = UiMode::MainMenu;
                self.status_message = Some("Deployment cancelled".to_string());
            }
        }
    }

    // ========================================================================
    // Artist Canonicalization Flow Handlers
    // ========================================================================

    fn start_canon_flow(&mut self) {
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

        // Get artist buckets with variants
        match db.get_artist_canonicalization_buckets() {
            Ok(bucket_data) if !bucket_data.is_empty() => {
                // Convert to ArtistBucket structs
                let buckets: Vec<canon_flow::ArtistBucket> = bucket_data
                    .into_iter()
                    .map(|(normalized_key, variants)| canon_flow::ArtistBucket {
                        normalized_key,
                        variants: variants
                            .into_iter()
                            .map(|(name, track_count)| canon_flow::ArtistVariant {
                                name,
                                track_count,
                                selected_for_squash: false,
                            })
                            .collect(),
                    })
                    .collect();

                let session = canon_flow::CanonSession::new(
                    uuid::Uuid::new_v4().to_string(),
                    buckets,
                );

                self.canon_cluster_view = Some(canon_flow::ClusterViewState::new(session));
                self.mode = UiMode::CanonClusterView;
            }
            Ok(_) => {
                self.status_message = Some("No artist name variants found - corpus is clean!".to_string());
            }
            Err(e) => {
                self.status_message = Some(format!("Error loading artist buckets: {}", e));
            }
        }
    }

    fn handle_canon_cluster_action(&mut self, action: canon_flow::ClusterViewAction) {
        match action {
            canon_flow::ClusterViewAction::None => {}
            canon_flow::ClusterViewAction::Continue => {}
            canon_flow::ClusterViewAction::SessionComplete => {
                // All buckets processed, go to review
                self.transition_to_canon_review();
            }
            canon_flow::ClusterViewAction::ShowSessionReview => {
                self.transition_to_canon_review();
            }
            canon_flow::ClusterViewAction::StatusMessage(msg) => {
                self.status_message = Some(msg);
            }
        }
    }

    fn transition_to_canon_review(&mut self) {
        if let Some(cluster_view) = self.canon_cluster_view.take() {
            let session = cluster_view.into_session();

            if session.decisions.is_empty() {
                self.status_message = Some("No decisions made".to_string());
                self.mode = UiMode::MainMenu;
            } else {
                self.canon_session_review = Some(canon_flow::ReviewState::new(session));
                self.mode = UiMode::CanonSessionReview;
            }
        }
    }

    fn handle_canon_review_action(&mut self, action: canon_flow::ReviewAction) {
        match action {
            canon_flow::ReviewAction::None => {}
            canon_flow::ReviewAction::Continue => {}
            canon_flow::ReviewAction::Commit => {
                self.commit_canon_changes();
            }
            canon_flow::ReviewAction::Cancel => {
                self.canon_session_review = None;
                self.mode = UiMode::MainMenu;
                self.status_message = Some("Artist canonicalization cancelled".to_string());
            }
            canon_flow::ReviewAction::BackToClusterView => {
                // Return to cluster view to add more decisions
                if let Some(review) = self.canon_session_review.take() {
                    let session = review.into_session();
                    self.canon_cluster_view = Some(canon_flow::ClusterViewState::new(session));
                    self.mode = UiMode::CanonClusterView;
                }
            }
        }
    }

    fn commit_canon_changes(&mut self) {
        let _ = config::log_message("=== CANON REVIEW: COMMIT REQUESTED ===");

        // Get all pending changes from decisions
        let all_changes: Vec<_> = if let Some(ref review) = self.canon_session_review {
            review.session().decisions
                .iter()
                .flat_map(|d| d.pending_changes.clone())
                .collect()
        } else {
            vec![]
        };

        let _ = config::log_message(&format!(
            "Total pending tag changes to execute: {}",
            all_changes.len()
        ));

        if all_changes.is_empty() {
            self.canon_session_review = None;
            self.mode = UiMode::MainMenu;
            self.status_message = Some("No changes to commit".to_string());
            return;
        }

        // Open database
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                self.status_message = Some(format!("Config error: {}", e));
                self.canon_session_review = None;
                self.mode = UiMode::MainMenu;
                return;
            }
        };
        let db = match Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                self.status_message = Some(format!("Database error: {}", e));
                self.canon_session_review = None;
                self.mode = UiMode::MainMenu;
                return;
            }
        };

        // Update database artist fields directly
        let mut succeeded = 0;
        let mut failed = 0;

        for change in &all_changes {
            if let Some(ref metadata_json) = change.metadata_changes {
                if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(metadata_json) {
                    let new_artist = metadata.get("new_artist")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let old_artist = metadata.get("old_artist")
                        .and_then(|v| v.as_str());

                    // Get track ID by path
                    if let Ok(Some(track)) = db.get_track_by_path(&change.source_path) {
                        if let Some(track_id) = track.id {
                            match db.update_artist_for_tracks(&[track_id], new_artist) {
                                Ok(_) => {
                                    succeeded += 1;
                                    // Record tag mismatch: disk still has old value, DB now has new value
                                    if let Err(e) = db.record_tag_mismatch(
                                        track_id,
                                        "artist",
                                        Some(new_artist),  // db_value
                                        old_artist,        // disk_value
                                    ) {
                                        let _ = config::log_message(&format!(
                                            "Failed to record tag mismatch for {}: {}",
                                            change.source_path, e
                                        ));
                                    }
                                }
                                Err(e) => {
                                    let _ = config::log_message(&format!(
                                        "Failed to update artist for {}: {}",
                                        change.source_path, e
                                    ));
                                    failed += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        let _ = config::log_message(&format!(
            "Canon commit complete: {} succeeded, {} failed",
            succeeded, failed
        ));

        // Compute stale deployment count
        let stale_count = match crate::ops::deploy::compute_full_deployment_status(&self.config, &db) {
            Ok(statuses) => statuses.iter().map(|s| s.stale.len()).sum(),
            Err(_) => 0,
        };

        let _ = config::log_message(&format!(
            "Stale deployments after commit: {}",
            stale_count
        ));

        // Show commit modal with options
        self.canon_session_review = None;
        self.canon_commit_modal_state = Some(CanonCommitModalState {
            selected_option: 1, // Default to deployment review
            tracks_updated: succeeded,
            stale_deployments: stale_count,
        });
        self.mode = UiMode::CanonCommitModal;

        // Start background tag-flush operation
        self.start_canon_tag_flush(all_changes);
    }

    fn start_canon_tag_flush(&mut self, changes: Vec<crate::db::PendingChange>) {
        let change_count = changes.len();

        // Add operation to manager
        let (id, tx, cancel_flag) = self.operations.add(OperationType::ExecutingChanges {
            session_id: "canon_tag_flush".to_string(),
            change_count,
        });

        let _ = config::log_message(&format!(
            "Starting background tag flush for {} tracks [{}]",
            change_count, id
        ));

        // Spawn background thread to write tags to disk
        std::thread::spawn(move || {
            use std::time::Instant;

            let start = Instant::now();
            let mut reporter = ProgressReporter::new(tx.clone(), cancel_flag.clone());

            let mut progress = OperationProgress::new(changes.len());
            reporter.force_update(progress.clone());

            let mut succeeded = 0;
            let mut failed = 0;
            let mut errors = Vec::new();
            let mut flushed_paths = Vec::new();

            for change in &changes {
                if reporter.is_cancelled() {
                    reporter.cancelled();
                    return;
                }

                progress.current_item = Some(change.source_path.clone());
                reporter.update(progress.clone());

                // Parse metadata to get new artist
                if let Some(ref metadata_json) = change.metadata_changes {
                    if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(metadata_json) {
                        let new_artist = metadata.get("new_artist")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        // Write tag to file using lofty
                        match crate::metadata::write_artist_tag(&change.source_path, new_artist) {
                            Ok(_) => {
                                succeeded += 1;
                                flushed_paths.push(change.source_path.clone());
                            }
                            Err(e) => {
                                errors.push(format!("{}: {}", change.source_path, e));
                                failed += 1;
                            }
                        }
                    }
                }

                progress.completed_items += 1;
                reporter.update(progress.clone());
            }

            // Store flushed paths in result data for mismatch cleanup
            let result = OperationResult {
                succeeded,
                skipped: 0,
                failed,
                duration: start.elapsed(),
                bytes_processed: None,
                errors,
                data: Some(crate::ops::operation::ResultData::TagFlush { flushed_paths }),
            };
            reporter.complete(result);
        });
    }

    fn update_operation_progress(&mut self) {
        // Poll all tracked operations from OperationManager
        let completed = self.operations.poll_all();
        for (id, result, op_type) in completed {
            // Log operation summary
            log_operation_summary(&result, &op_type);

            // Handle tag flush completion - clear tag mismatches
            if let OperationType::ExecutingChanges { ref session_id, .. } = op_type {
                if session_id == "canon_tag_flush" {
                    self.clear_tag_mismatches_for_flushed(&result);
                }
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
                // Heartbeat completed - pass result to main menu for display
                self.main_menu.set_heartbeat_result(result.clone());
                self.heartbeat_result = Some(result);
                self.heartbeat_receiver = None;
                self.eye.set_heartbeat_pending(false);
            }
        }
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
        dir_browser: app.dir_browser.as_mut(),
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
        exit_confirm_modal_state: app.exit_confirm_modal_state.as_ref(),
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
// Entry Point
// ============================================================================

pub fn run_menu(config: Config) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config);

    // Check health data version and trigger rebuild if needed
    check_and_maybe_rebuild_health(&mut app);

    // Spawn heartbeat check if enabled
    if app.config.opinions.startup.heartbeat_on_startup {
        let rx = spawn_heartbeat(&app.config);
        app.heartbeat_receiver = Some(rx);
        app.eye.set_heartbeat_pending(true);
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

        // Check if eye blink triggered a heartbeat (d20 rolled 13)
        if app.eye.take_heartbeat_trigger() && app.heartbeat_receiver.is_none() {
            let rx = spawn_heartbeat(&app.config);
            app.heartbeat_receiver = Some(rx);
            app.eye.set_heartbeat_pending(true);
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
