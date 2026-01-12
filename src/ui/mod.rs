//! TUI Module
//!
//! Modular UI components for the Music Library Assistant.

pub mod app;
pub mod dedup_flow;
pub mod dialogue;
pub mod dir_browser;
pub mod helpers;
pub mod main_menu;
pub mod picker;
pub mod tag_editor;

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame, Terminal,
};
use std::collections::VecDeque;
use std::io;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::Instant;

use crate::config::{self, Config};
use crate::db::Database;
use crate::health::{spawn_heartbeat, HeartbeatResult};
use crate::ops::{reports, scanner};
use crate::progress::ScanMessage;

use app::{EyeAnimation, EyeFrame, OperationState, OperationType, EYE_CLOSED, EYE_CLOSING, EYE_OPEN};
use helpers::{calculate_rolling_throughput, format_bytes_binary, format_eta, truncate_path_display};
use main_menu::{BackgroundTask, CommandAction, MainMenuState, MenuAction, ReportType, TransitionTarget};

// ============================================================================
// Application State
// ============================================================================

/// Current UI mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum UiMode {
    MainMenu,
    TagEditor,
    DirBrowser,
    Dialogue,
    DialogueSummary,
    ClusterDialogue,
    BulkReviewPrompt,
    SessionReview,
    DropMissingConfirmation,
}

/// State for drop missing confirmation dialog
#[derive(Debug, Clone)]
struct DropMissingState {
    missing_tracks: Vec<crate::db::Track>,
    list_offset: usize,
    selected_option: usize, // 0 = Cancel, 1 = Drop
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

    // Background operations
    operation: Option<OperationState>,
    operation_receiver: Option<mpsc::Receiver<ScanMessage>>,

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
            operation: None,
            operation_receiver: None,
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
                self.should_quit = true;
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
            BackgroundTask::DetectMissing => {
                self.detect_missing_files();
            }
            BackgroundTask::RebuildHealthIndex => {
                self.rebuild_health_index();
            }
            BackgroundTask::GenerateReport { report_type } => {
                self.generate_report(report_type);
            }
            BackgroundTask::Deploy { dry_run } => {
                self.status_message = Some(format!(
                    "Deploy {} (TODO)",
                    if dry_run { "preview" } else { "execute" }
                ));
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

    fn start_scan_source(&mut self, name: &str, path: &str) {
        let (tx, rx) = mpsc::channel();
        let source_path = path.to_string();
        let source_name = name.to_string();
        let cancel_flag = Arc::new(AtomicBool::new(false));

        self.operation = Some(OperationState::new(OperationType::Scanning {
            source_name: source_name.clone(),
        }));
        self.operation_receiver = Some(rx);

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
        // Get reports directory
        let reports_dir = match config::get_data_dir() {
            Ok(dir) => dir.join("reports"),
            Err(e) => {
                self.status_message = Some(format!("Failed to get data dir: {}", e));
                return;
            }
        };

        // Ensure reports directory exists
        if let Err(e) = std::fs::create_dir_all(&reports_dir) {
            self.status_message = Some(format!("Failed to create reports dir: {}", e));
            return;
        }

        let results: Vec<Result<String, String>> = match report_type {
            ReportType::GenerateAll => {
                vec![
                    reports::generate_duplicate_report(&reports_dir.join("duplicates.txt"))
                        .map_err(|e| e.to_string()),
                    reports::generate_quality_report(&reports_dir.join("quality.txt"))
                        .map_err(|e| e.to_string()),
                    reports::generate_deployment_report(&reports_dir.join("deployment.txt"))
                        .map_err(|e| e.to_string()),
                ]
            }
            ReportType::Legacy => {
                vec![
                    reports::generate_legacy_report(&reports_dir.join("legacy.txt"))
                        .map_err(|e| e.to_string()),
                ]
            }
            ReportType::Deployment => {
                vec![
                    reports::generate_deployment_report(&reports_dir.join("deployment.txt"))
                        .map_err(|e| e.to_string()),
                ]
            }
            ReportType::Quality => {
                vec![
                    reports::generate_quality_report(&reports_dir.join("quality.txt"))
                        .map_err(|e| e.to_string()),
                ]
            }
            ReportType::Duplicates => {
                vec![
                    reports::generate_duplicate_report(&reports_dir.join("duplicates.txt"))
                        .map_err(|e| e.to_string()),
                ]
            }
            ReportType::Health => {
                vec![
                    reports::generate_health_report(&reports_dir.join("health.txt"))
                        .map_err(|e| e.to_string()),
                ]
            }
        };

        let successes: Vec<_> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
        let failures: Vec<_> = results.iter().filter_map(|r| r.as_ref().err()).collect();

        if failures.is_empty() {
            self.status_message = Some(format!(
                "Generated {} report(s) in {}",
                successes.len(),
                reports_dir.display()
            ));
        } else if successes.is_empty() {
            self.status_message = Some(format!("Report generation failed: {}", failures[0]));
        } else {
            self.status_message = Some(format!(
                "{} report(s) generated, {} failed",
                successes.len(),
                failures.len()
            ));
        }
    }

    fn detect_missing_files(&mut self) {
        // Open database
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

        // Find missing files
        match scanner::find_missing_tracks(&db, "corpus") {
            Ok(missing) => {
                if missing.is_empty() {
                    self.status_message = Some("No missing files found in index".to_string());
                } else {
                    self.status_message = Some(format!(
                        "Found {} missing files. Use 'Drop Missing From Index' to remove entries.",
                        missing.len()
                    ));
                }
            }
            Err(e) => {
                self.status_message = Some(format!("Detection error: {}", e));
            }
        }
    }

    fn rebuild_health_index(&mut self) {
        use crate::health::{detect_fingerprint_issues, detect_metadata_issues};

        // Open database
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

        // Get all tracks with fingerprints
        let tracks = match db.get_all_tracks(Some("corpus")) {
            Ok(t) => t,
            Err(e) => {
                self.status_message = Some(format!("Error getting tracks: {}", e));
                return;
            }
        };

        let total = tracks.len();
        let with_fingerprints: Vec<_> = tracks.into_iter().filter(|t| t.fingerprint.is_some()).collect();
        let fp_count = with_fingerprints.len();

        // Run health detection on each track
        let mut issues_found = 0;
        for track in &with_fingerprints {
            if let Ok(issues) = detect_fingerprint_issues(&db, track) {
                issues_found += issues.len();
            }
            let _ = detect_metadata_issues(&db, track);
        }

        self.status_message = Some(format!(
            "Rebuilt health index: {} tracks ({} with fingerprints), {} issues detected",
            total, fp_count, issues_found
        ));

        // Refresh corpus summary
        self.refresh_corpus_summary();
    }

    fn start_drop_missing_confirmation(&mut self) {
        // Open database
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

        // Find missing files
        match scanner::find_missing_tracks(&db, "corpus") {
            Ok(missing) => {
                if missing.is_empty() {
                    self.status_message = Some("No missing files found in index".to_string());
                    // Stay in main menu
                } else {
                    // Enter confirmation mode
                    self.drop_missing_state = Some(DropMissingState {
                        missing_tracks: missing,
                        list_offset: 0,
                        selected_option: 0, // Default to Cancel
                    });
                    self.mode = UiMode::DropMissingConfirmation;
                }
            }
            Err(e) => {
                self.status_message = Some(format!("Detection error: {}", e));
            }
        }
    }

    fn handle_drop_missing_key(&mut self, key: crossterm::event::KeyEvent) {
        use crossterm::event::KeyCode;

        if let Some(ref mut state) = self.drop_missing_state {
            let list_len = state.missing_tracks.len();
            let max_visible = 20; // Number of items visible in the list

            match key.code {
                KeyCode::Up => {
                    if state.list_offset > 0 {
                        state.list_offset -= 1;
                    }
                }
                KeyCode::Down => {
                    if state.list_offset + max_visible < list_len {
                        state.list_offset += 1;
                    }
                }
                KeyCode::Left | KeyCode::Right => {
                    // Toggle between Cancel (0) and Drop (1)
                    state.selected_option = 1 - state.selected_option;
                }
                KeyCode::Enter => {
                    if state.selected_option == 1 {
                        // Execute drop
                        self.execute_drop_missing();
                    } else {
                        // Cancel - return to main menu
                        self.drop_missing_state = None;
                        self.mode = UiMode::MainMenu;
                        self.status_message = Some("Drop cancelled".to_string());
                    }
                }
                KeyCode::Esc => {
                    // Cancel
                    self.drop_missing_state = None;
                    self.mode = UiMode::MainMenu;
                }
                _ => {}
            }
        }
    }

    fn execute_drop_missing(&mut self) {
        use crate::db::{ChangeStatus, ChangeType, PendingChange};
        use crate::ops::changes;
        use std::fs::File;
        use std::io::Write;

        // Get the missing tracks from state
        let missing_tracks = match &self.drop_missing_state {
            Some(state) => state.missing_tracks.clone(),
            None => {
                self.status_message = Some("No missing tracks to drop".to_string());
                self.mode = UiMode::MainMenu;
                return;
            }
        };

        if missing_tracks.is_empty() {
            self.drop_missing_state = None;
            self.mode = UiMode::MainMenu;
            self.status_message = Some("No missing files to drop".to_string());
            return;
        }

        // Get reports directory for log file
        let reports_dir = match config::get_data_dir() {
            Ok(dir) => dir.join("reports"),
            Err(e) => {
                self.status_message = Some(format!("Failed to get data dir: {}", e));
                self.drop_missing_state = None;
                self.mode = UiMode::MainMenu;
                return;
            }
        };

        // Create reports directory if needed
        if let Err(e) = std::fs::create_dir_all(&reports_dir) {
            self.status_message = Some(format!("Failed to create reports dir: {}", e));
            self.drop_missing_state = None;
            self.mode = UiMode::MainMenu;
            return;
        }

        // Write log file with dropped track metadata
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let log_path = reports_dir.join(format!("dropped_tracks_{}.log", timestamp));
        let log_result = (|| -> Result<(), std::io::Error> {
            let mut log_file = File::create(&log_path)?;
            writeln!(log_file, "# Tracks dropped from corpus index")?;
            writeln!(log_file, "# Timestamp: {}", chrono::Local::now())?;
            writeln!(log_file, "# Count: {}", missing_tracks.len())?;
            writeln!(log_file, "#")?;
            for track in &missing_tracks {
                writeln!(log_file, "Path: {}", track.path)?;
                if let Some(ref artist) = track.artist {
                    writeln!(log_file, "  Artist: {}", artist)?;
                }
                if let Some(ref title) = track.title {
                    writeln!(log_file, "  Title: {}", title)?;
                }
                if let Some(ref album) = track.album {
                    writeln!(log_file, "  Album: {}", album)?;
                }
                writeln!(log_file)?;
            }
            Ok(())
        })();

        let log_created = log_result.is_ok();

        // Open database
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(e) => {
                self.status_message = Some(format!("Config error: {}", e));
                self.drop_missing_state = None;
                self.mode = UiMode::MainMenu;
                return;
            }
        };
        let db = match Database::open(&db_path) {
            Ok(db) => db,
            Err(e) => {
                self.status_message = Some(format!("Database error: {}", e));
                self.drop_missing_state = None;
                self.mode = UiMode::MainMenu;
                return;
            }
        };

        // Generate DropIndex changes for each missing track
        let session_id = uuid::Uuid::new_v4().to_string();
        let pending_changes: Vec<PendingChange> = missing_tracks
            .iter()
            .map(|track| PendingChange {
                id: None,
                session_id: session_id.clone(),
                change_type: ChangeType::DropIndex,
                source_path: track.path.clone(),
                target_path: None,
                metadata_changes: None,
                created_at: None,
                status: ChangeStatus::Pending,
            })
            .collect();

        // Execute the changes
        match changes::execute_changes(&db, &pending_changes, false) {
            Ok(report) => {
                let msg = if log_created {
                    format!(
                        "Dropped {} entries from index. Log: {}",
                        report.succeeded,
                        log_path.display()
                    )
                } else {
                    format!("Dropped {} entries from index", report.succeeded)
                };

                if report.failed > 0 {
                    self.status_message = Some(format!(
                        "{}. {} failed: {:?}",
                        msg, report.failed, report.errors
                    ));
                } else {
                    self.status_message = Some(msg);
                }
            }
            Err(e) => {
                self.status_message = Some(format!("Drop error: {}", e));
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

    fn update_operation_progress(&mut self) {
        let mut should_clear = false;

        if let Some(ref rx) = self.operation_receiver {
            while let Ok(message) = rx.try_recv() {
                match message {
                    ScanMessage::Progress(progress) => {
                        // Record throughput sample
                        let now = Instant::now();
                        self.throughput_samples.push_back((now, progress.bytes_processed));

                        // Keep only samples from last 15 seconds (allows 8s rolling window + buffer)
                        let cutoff = now - std::time::Duration::from_secs(15);
                        while let Some((t, _)) = self.throughput_samples.front() {
                            if *t < cutoff {
                                self.throughput_samples.pop_front();
                            } else {
                                break;
                            }
                        }

                        if let Some(ref mut op) = self.operation {
                            op.progress = progress;
                        }
                    }
                    ScanMessage::Complete(result) => {
                        self.status_message = Some(if result.files_scanned == 0 && result.files_skipped > 0 {
                            format!(
                                "Scan complete! {} files skipped (unchanged) in {:.1}s",
                                result.files_skipped,
                                result.duration.as_secs_f64()
                            )
                        } else {
                            format!(
                                "Scan complete! {} files ({:.2} GB) in {:.1}s",
                                result.files_scanned,
                                result.bytes_scanned as f64 / 1_000_000_000.0,
                                result.duration.as_secs_f64()
                            )
                        });
                        should_clear = true;
                    }
                    ScanMessage::Error(err) => {
                        self.status_message = Some(format!("Scan error: {}", err));
                        should_clear = true;
                    }
                }
            }
        }

        if should_clear {
            self.operation = None;
            self.operation_receiver = None;
            self.throughput_samples.clear();
            // Refresh corpus summary after operation completes
            self.refresh_corpus_summary();
        }
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Header
            Constraint::Min(10),    // Content
            Constraint::Length(18), // Footer with eye
        ])
        .split(f.area());

    render_header(f, chunks[0], app);
    render_content(f, chunks[1], app);
    render_footer(f, chunks[2], app);
}

fn render_header(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let title = match app.mode {
        UiMode::MainMenu => "Music Library Assistant",
        UiMode::TagEditor => "Music Library Assistant - Tag Editor",
        UiMode::DirBrowser => "Music Library Assistant - Directory Browser",
        UiMode::Dialogue => "Music Library Assistant - Decision Flow",
        UiMode::DialogueSummary => "Music Library Assistant - Session Summary",
        UiMode::ClusterDialogue => "Music Library Assistant - Fingerprint Deduplication",
        UiMode::BulkReviewPrompt => "Music Library Assistant - Bulk Decision Point",
        UiMode::SessionReview => "Music Library Assistant - Session Review",
        UiMode::DropMissingConfirmation => "Music Library Assistant - Drop Missing From Index",
    };

    let header = Paragraph::new(title)
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL));

    f.render_widget(header, area);
}

fn render_content(f: &mut Frame, area: ratatui::layout::Rect, app: &mut App) {
    match app.mode {
        UiMode::MainMenu => {
            app.main_menu.render(f, area, &Some(app.config.clone()));
        }
        UiMode::TagEditor => {
            if let Some(ref mut editor) = app.tag_editor {
                editor.render(f, area, app.status_message.as_deref());
            }
        }
        UiMode::DirBrowser => {
            if let Some(ref mut browser) = app.dir_browser {
                browser.render(f, area);
            }
        }
        UiMode::Dialogue => {
            if let Some(ref mut dialogue) = app.dialogue {
                dialogue.render(f, area);
            }
        }
        UiMode::DialogueSummary => {
            if let Some(ref mut summary) = app.dialogue_summary {
                summary.render(f, area);
            }
        }
        UiMode::ClusterDialogue => {
            if let Some(ref mut cluster_dlg) = app.cluster_dialogue {
                cluster_dlg.render(f, area);
            }
        }
        UiMode::BulkReviewPrompt => {
            if let Some(ref mut bulk_prompt) = app.bulk_prompt {
                bulk_prompt.render(f, area);
            }
        }
        UiMode::SessionReview => {
            if let Some(ref mut session_review) = app.session_review {
                session_review.render(f, area);
            }
        }
        UiMode::DropMissingConfirmation => {
            render_drop_missing_confirmation(f, area, app);
        }
    }
}

fn render_drop_missing_confirmation(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    use ratatui::widgets::{List, ListItem};

    if let Some(ref state) = app.drop_missing_state {
        let count = state.missing_tracks.len();

        // Layout: header info, file list, action buttons
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Header with count
                Constraint::Min(5),     // File list
                Constraint::Length(3),  // Action buttons
            ])
            .split(area);

        // Header
        let header_text = format!(
            "Found {} missing file{} in corpus index\nThese files no longer exist on disk but are still in the database.",
            count,
            if count == 1 { "" } else { "s" }
        );
        let header = Paragraph::new(header_text)
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(header, chunks[0]);

        // File list
        let max_visible = chunks[1].height.saturating_sub(2) as usize;
        let items: Vec<ListItem> = state
            .missing_tracks
            .iter()
            .skip(state.list_offset)
            .take(max_visible)
            .map(|track| {
                ListItem::new(track.path.clone())
                    .style(Style::default().fg(Color::White))
            })
            .collect();

        let list_title = format!(
            "Missing Files ({}-{} of {})",
            state.list_offset + 1,
            (state.list_offset + items.len()).min(count),
            count
        );
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(list_title));
        f.render_widget(list, chunks[1]);

        // Action buttons
        let cancel_style = if state.selected_option == 0 {
            Style::default().fg(Color::Black).bg(Color::White)
        } else {
            Style::default().fg(Color::White)
        };
        let drop_style = if state.selected_option == 1 {
            Style::default().fg(Color::Black).bg(Color::Red)
        } else {
            Style::default().fg(Color::Red)
        };

        let buttons = Line::from(vec![
            Span::raw("  "),
            Span::styled(" Cancel ", cancel_style),
            Span::raw("    "),
            Span::styled(format!(" Drop {} entries ", count), drop_style),
            Span::raw("  "),
        ]);
        let buttons_para = Paragraph::new(buttons)
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).title("Action"));
        f.render_widget(buttons_para, chunks[2]);
    }
}

fn render_footer(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    // Three status boxes + eye animation
    // Eye is 64 chars wide + 2 for borders = 66, but we want 70 total width
    let footer_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Min(30),      // Left status area (flexible)
            Constraint::Length(70),   // Eye animation (fixed 70 cols)
        ])
        .split(area);

    // Left side: three stacked status boxes
    let status_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),  // Corpus status
            Constraint::Length(6),  // Operation status
            Constraint::Min(4),     // Controls
        ])
        .split(footer_layout[0]);

    render_corpus_status(f, status_layout[0], app);
    render_operation_status(f, status_layout[1], app);
    render_controls(f, status_layout[2], app);

    // Eye animation (70 cols wide, 64-col eye centered)
    render_eye(f, footer_layout[1], app);
}

fn render_corpus_status(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let mut lines = Vec::new();

    // Show heartbeat status first
    if let Some(ref hb) = app.heartbeat_result {
        if hb.is_healthy() {
            lines.push(
                Line::from(format!("Validated ({:.0}ms)", hb.duration.as_millis()))
                    .style(Style::default().fg(Color::Green)),
            );
        } else {
            if hb.missing_from_disk > 0 {
                lines.push(
                    Line::from(format!("{} missing", hb.missing_from_disk))
                        .style(Style::default().fg(Color::Yellow)),
                );
            }
            if hb.new_on_disk > 0 {
                lines.push(
                    Line::from(format!("{} new files", hb.new_on_disk))
                        .style(Style::default().fg(Color::Cyan)),
                );
            }
        }
    } else if app.heartbeat_receiver.is_some() {
        lines.push(Line::from("Validating...").style(Style::default().fg(Color::DarkGray)));
    }

    // Show corpus summary from cached data
    if let Some(ref summary) = app.main_menu.corpus_summary {
        if summary.track_count == 0 {
            lines.push(
                Line::from("No scan data")
                    .style(Style::default().fg(Color::Yellow)),
            );
        } else {
            lines.push(Line::from(format!("Indexed: {} tracks", summary.track_count)));

            // Deployment status
            if let Some(ref ds) = summary.deployment_stats {
                lines.push(Line::from(format!(
                    "Deployed: {:.0}%",
                    ds.deployment_percentage
                )));
            }

            // Duplicate status
            if summary.duplicate_groups > 0 {
                lines.push(
                    Line::from(format!("Duplicates: {} groups", summary.duplicate_groups))
                        .style(Style::default().fg(Color::Yellow)),
                );
            }

            // Pending changes
            let pending_total: usize = summary.pending_changes.values().sum();
            if pending_total > 0 {
                lines.push(
                    Line::from(format!("Pending: {} changes", pending_total))
                        .style(Style::default().fg(Color::Cyan)),
                );
            }
        }
    }

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Corpus"));
    f.render_widget(para, area);
}

fn render_operation_status(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let mut lines = Vec::new();

    if let Some(ref op) = app.operation {
        let progress = &op.progress;

        // Operation description with ETA
        let desc = op.operation_type.description();
        let eta_span = if progress.bytes_processed > 0 && progress.total_bytes > 0 {
            // Calculate ETA from rolling throughput
            let throughput = calculate_rolling_throughput(&app.throughput_samples, 8);
            if let Some(mib_per_sec) = throughput {
                if mib_per_sec > 0.01 {
                    let remaining_bytes = progress.total_bytes.saturating_sub(progress.bytes_processed);
                    let remaining_mib = remaining_bytes as f64 / (1024.0 * 1024.0);
                    let eta_secs = (remaining_mib / mib_per_sec) as u64;
                    Span::styled(
                        format!(" (ETA: {})", format_eta(eta_secs)),
                        Style::default().fg(Color::DarkGray),
                    )
                } else {
                    Span::raw("")
                }
            } else {
                Span::raw("")
            }
        } else {
            Span::raw("")
        };

        lines.push(Line::from(vec![
            Span::styled(desc, Style::default().fg(Color::Cyan)),
            eta_span,
        ]));

        // Files progress
        lines.push(Line::from(format!(
            "Files: {}/{} | Skipped: {} unchanged",
            progress.files_processed,
            progress.total_files,
            progress.files_skipped
        )));

        // Mtime mismatch statistics (for debugging incremental scan)
        if let Some(ref stats) = progress.mtime_stats {
            if stats.mismatch_count > 0 || stats.not_in_db_count > 0 {
                let mut parts = Vec::new();
                if stats.not_in_db_count > 0 {
                    parts.push(format!("new:{}", stats.not_in_db_count));
                }
                if stats.mismatch_count > 0 {
                    parts.push(format!("mtime_delta:{}", stats.mismatch_count));
                    if let Some(mean) = stats.mean_diff_secs() {
                        let median = stats.median_diff_secs().unwrap_or(0);
                        let mode = stats.mode_diff_secs().unwrap_or(0);
                        let stddev = stats.stddev_diff_secs().unwrap_or(0.0);
                        parts.push(format!(
                            "μ={:.1}s med={}s mode={}s σ={:.1}",
                            mean, median, mode, stddev
                        ));
                    }
                }
                lines.push(Line::from(parts.join(" | "))
                    .style(Style::default().fg(Color::DarkGray)));
            }
        }

        // Bytes: XX GiB / YY TiB (ZZZ MiB/s)
        let processed_str = format_bytes_binary(progress.bytes_processed);
        let total_str = format_bytes_binary(progress.total_bytes);
        let throughput_str = match calculate_rolling_throughput(&app.throughput_samples, 8) {
            Some(mib_per_sec) => format!(" ({:.1} MiB/s)", mib_per_sec),
            None => String::new(),
        };
        lines.push(Line::from(format!(
            "Bytes: {} / {}{}",
            processed_str, total_str, throughput_str
        )));

        // Current file (UTF-8 safe truncation)
        if let Some(file) = &progress.current_file {
            let display = truncate_path_display(file, 50);
            lines.push(Line::from(display).style(Style::default().fg(Color::DarkGray)));
        }
    } else {
        lines.push(Line::from("No operation in progress")
            .style(Style::default().fg(Color::DarkGray)));
    }

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Operation"));
    f.render_widget(para, area);
}

fn render_controls(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let mut lines = Vec::new();

    // Show any status message first
    if let Some(ref msg) = app.status_message {
        lines.push(Line::from(msg.clone()).style(Style::default().fg(Color::Yellow)));
    }

    // Navigation hints based on mode
    let hints = match app.mode {
        UiMode::MainMenu => "↑↓ Navigate | ←→ Pane | Enter Select | Q Quit",
        UiMode::TagEditor => "Tab Tracks | ↑↓ Fields | Enter Edit | Esc Exit",
        UiMode::DirBrowser => "↑↓ Navigate | ←→ Expand | Space Toggle | Enter Proceed | Esc Cancel",
        UiMode::Dialogue | UiMode::DialogueSummary => "↑↓ Navigate | Enter Select | Esc Exit",
        UiMode::ClusterDialogue => "↑↓ Select | Enter Keep | Tab Skip | Esc Review",
        UiMode::BulkReviewPrompt => "↑↓ Select | Enter Choose | Esc Cancel",
        UiMode::SessionReview => "↑↓ Select | Enter Execute | Esc Cancel",
        UiMode::DropMissingConfirmation => "↑↓ Scroll | ←→ Select Option | Enter Confirm | Esc Cancel",
    };
    lines.push(Line::from(hints));

    let para = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("Controls"));
    f.render_widget(para, area);
}

fn render_eye(f: &mut Frame, area: ratatui::layout::Rect, app: &App) {
    let eye_text = match app.eye.current_frame() {
        EyeFrame::Open => EYE_OPEN,
        EyeFrame::Closing => EYE_CLOSING,
        EyeFrame::Closed => EYE_CLOSED,
    };

    // Center the 64-col eye in the 70-col frame (3 spaces padding each side)
    let centered_eye: String = eye_text
        .lines()
        .map(|line| format!("   {}   ", line))
        .collect::<Vec<_>>()
        .join("\n");

    let eye_para = Paragraph::new(centered_eye)
        .style(Style::default().fg(Color::Cyan))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(eye_para, area);
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
