//! TUI Module
//!
//! Modular UI components for the Music Library Assistant.

pub mod app;
pub mod dialogue;
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
use crate::db::{Database, DecisionStack};
use crate::progress::ScanMessage;
use crate::reports;
use crate::scanner;

use app::{EyeAnimation, EyeFrame, OperationState, OperationType, EYE_CLOSED, EYE_CLOSING, EYE_OPEN};
use helpers::{calculate_rolling_throughput, format_bytes_binary, format_eta, truncate_path_display};
use main_menu::{BackgroundTask, CommandAction, MainMenuState, MenuAction, ReportType, TransitionTarget};

// ============================================================================
// Application State
// ============================================================================

/// Current UI mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiMode {
    MainMenu,
    TagEditor,
    Dialogue,
    DialogueSummary,
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
    dialogue: Option<dialogue::DialogueState>,
    dialogue_summary: Option<dialogue::DialogueSummaryState>,

    // Background operations
    operation: Option<OperationState>,
    operation_receiver: Option<mpsc::Receiver<ScanMessage>>,

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
            dialogue: None,
            dialogue_summary: None,
            operation: None,
            operation_receiver: None,
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
            BackgroundTask::ScanCorpus { re_fingerprint } => {
                self.start_scan_corpus(re_fingerprint);
            }
            BackgroundTask::ScanLegacy => {
                self.start_scan_legacy();
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

    fn start_scan_corpus(&mut self, _re_fingerprint: bool) {
        let path = self.config.corpus_root.to_string_lossy().to_string();
        // TODO: Pass _re_fingerprint flag to scanner to clear scan_state cache
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

    fn transition_to(&mut self, target: TransitionTarget) {
        match target {
            TransitionTarget::TagEditor => {
                self.start_tag_editor();
            }
            TransitionTarget::DecisionFlow => {
                self.start_decision_flow();
            }
            TransitionTarget::PendingChangesView => {
                self.status_message = Some("Pending changes view (TODO)".to_string());
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
        // Decision flow uses fingerprint duplicates - load from deduplication module
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

        // Build decision stack from unresolved fingerprint duplicates
        match db.get_unresolved_duplicate_groups() {
            Ok(group_ids) if !group_ids.is_empty() => {
                use crate::db::{Decision, DecisionCategory, DecisionPriority};

                let mut decisions = Vec::new();
                for group_id in group_ids.iter().take(50) { // Limit to 50 for now
                    match db.get_duplicate_group_tracks(*group_id) {
                        Ok(tracks) if tracks.len() >= 2 => {
                            let affected_paths: Vec<String> = tracks.iter().map(|t| t.path.clone()).collect();
                            decisions.push(Decision {
                                id: group_id.to_string(),
                                priority: DecisionPriority::Medium,
                                category: DecisionCategory::FingerprintDuplicate,
                                summary: format!("{} duplicate files detected", tracks.len()),
                                details: format!("Group {} contains {} potential duplicates", group_id, tracks.len()),
                                affected_paths,
                                recommendation: Some("Keep highest quality version".to_string()),
                                pending_changes: Vec::new(), // Changes generated on accept
                                impact_summary: format!("{} files affected", tracks.len()),
                            });
                        }
                        _ => {}
                    }
                }

                if decisions.is_empty() {
                    self.status_message = Some("No pending decisions".to_string());
                    return;
                }

                let stack = DecisionStack {
                    decisions,
                    current_index: 0,
                    resolved: Vec::new(),
                    ignore_patterns: Vec::new(),
                };
                self.dialogue = Some(dialogue::DialogueState::new(stack));
                self.mode = UiMode::Dialogue;
            }
            Ok(_) => {
                self.status_message = Some("No pending decisions".to_string());
            }
            Err(e) => {
                self.status_message = Some(format!("Error loading decisions: {}", e));
            }
        }
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
        UiMode::Dialogue => "Music Library Assistant - Decision Flow",
        UiMode::DialogueSummary => "Music Library Assistant - Session Summary",
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

fn render_corpus_status(f: &mut Frame, area: ratatui::layout::Rect, _app: &App) {
    let mut lines = Vec::new();

    // Try to get corpus stats from database
    if let Ok(db_path) = config::get_db_path() {
        if let Ok(db) = Database::open(&db_path) {
            let track_count = db.get_track_count(None).unwrap_or(0);
            let dup_groups = db.get_unresolved_duplicate_groups().map(|g| g.len()).unwrap_or(0);

            if track_count == 0 {
                lines.push(Line::from("No scan data - run a scan first")
                    .style(Style::default().fg(Color::Yellow)));
            } else {
                lines.push(Line::from(format!("Indexed: {} tracks", track_count)));
                if dup_groups > 0 {
                    lines.push(Line::from(format!("Duplicates: {} groups need review", dup_groups))
                        .style(Style::default().fg(Color::Yellow)));
                } else {
                    lines.push(Line::from("Duplicates: None pending")
                        .style(Style::default().fg(Color::Green)));
                }
            }
        } else {
            lines.push(Line::from("Database unavailable").style(Style::default().fg(Color::Red)));
        }
    } else {
        lines.push(Line::from("Config error").style(Style::default().fg(Color::Red)));
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
        UiMode::Dialogue | UiMode::DialogueSummary => "↑↓ Navigate | Enter Select | Esc Exit",
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
