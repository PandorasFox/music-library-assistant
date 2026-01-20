//! Intake Confirmation Modal
//!
//! Shows a dialog when unindexed files are detected, asking the user
//! to confirm indexing. This runs after eyeballing completes and before
//! transitioning to the metadata analysis phase.
//!
//! After confirmation, shows progress while indexing mutations complete.

use std::path::PathBuf;

use crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::corpus::db::types::HealthIssueType;
use crate::corpus::db::Database;
use crate::corpus::mutations::Mutation;
use crate::config::log_message;
use crate::daemon::{DaemonStateSnapshot, TaskDaemon, WorkerStats};
use crate::db_thread::DbThreadStats;

/// State for the intake confirmation modal.
#[derive(Debug)]
pub struct IntakeConfirmationState {
    /// Number of unindexed files detected
    pub file_count: usize,
    /// Total bytes to read (sum of file sizes)
    pub total_bytes: u64,
    /// Paths to index (gathered from MissingFromIndex issues)
    pub paths: Vec<PathBuf>,
    /// Source identifier ("corpus" or "legacy")
    pub source: String,
    /// Number of directories containing unindexed files
    pub directory_count: usize,
    /// Whether we're in processing mode (indexing in progress)
    processing: bool,
    /// Whether we've seen daemon start working (to detect completion)
    seen_working: bool,
    /// Progress info while processing
    progress: Option<f32>,
    progress_detail: Option<String>,
    /// Worker stats for display
    worker_stats: Option<WorkerStats>,
    /// DB stats for display
    db_stats: Option<DbThreadStats>,
    /// Pending DB writes (always tracked, no timing guard)
    db_queue_depth: u64,
}

/// Action returned from handling input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeConfirmationAction {
    /// No action, continue showing modal
    None,
    /// User confirmed - queue mutations and start processing
    Confirmed,
    /// User skipped - proceed to metadata analysis without indexing
    Skipped,
    /// Processing complete - proceed to metadata analysis
    ProcessingComplete,
}

impl IntakeConfirmationState {
    /// Gather intake confirmation state from the database.
    ///
    /// Queries UnindexedFile signals (created during second-level signal derivation)
    /// to get the list of files that need indexing.
    ///
    /// Returns None if there are no unindexed files.
    pub fn gather(db: &Database, _corpus_root: &std::path::Path, source: &str) -> Option<Self> {
        // Get all UnindexedFile signals - these are pre-computed during Awakening
        let issues = match db.get_health_signals(Some(HealthIssueType::UnindexedFile)) {
            Ok(i) => i,
            Err(e) => {
                let _ = log_message(&format!(
                    "IntakeConfirmation::gather: query failed: {:?}",
                    e
                ));
                return None;
            }
        };

        let _ = log_message(&format!(
            "IntakeConfirmation::gather: found {} UnindexedFile signals",
            issues.len()
        ));

        if issues.is_empty() {
            return None;
        }

        // Each UnindexedFile signal has the file path as issue_key
        let mut all_paths: Vec<PathBuf> = Vec::new();
        let mut total_bytes: u64 = 0;
        let mut directories: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

        for issue in &issues {
            let path = PathBuf::from(&issue.issue_key);

            // Verify file still exists and get size
            if path.exists() && path.is_file() {
                if let Ok(meta) = std::fs::metadata(&path) {
                    total_bytes += meta.len();
                }

                // Track unique directories
                if let Some(parent) = path.parent() {
                    directories.insert(parent.to_path_buf());
                }

                all_paths.push(path);
            }
        }

        if all_paths.is_empty() {
            return None;
        }

        let _ = log_message(&format!(
            "IntakeConfirmation: gathered {} files ({} bytes) from {} directories",
            all_paths.len(),
            total_bytes,
            directories.len()
        ));

        Some(Self {
            file_count: all_paths.len(),
            total_bytes,
            paths: all_paths,
            source: source.to_string(),
            directory_count: directories.len(),
            processing: false,
            seen_working: false,
            progress: None,
            progress_detail: None,
            worker_stats: None,
            db_stats: None,
            db_queue_depth: 0,
        })
    }

    /// Check if we're in processing mode.
    pub fn is_processing(&self) -> bool {
        self.processing
    }

    /// Start processing mode after mutations are queued.
    pub fn start_processing(&mut self) {
        self.processing = true;
        self.progress = Some(0.0);
        self.progress_detail = Some("Starting...".to_string());
    }

    /// Update stats for display during processing.
    /// Only updates if stats are provided (timing instrumentation enabled).
    pub fn set_worker_stats(&mut self, stats: Option<WorkerStats>) {
        if stats.is_some() {
            self.worker_stats = stats;
        }
    }

    /// Update DB stats for display during processing.
    /// Only updates if stats are provided (timing instrumentation enabled).
    pub fn set_db_stats(&mut self, stats: Option<DbThreadStats>) {
        if stats.is_some() {
            self.db_stats = stats;
        }
    }

    /// Update pending DB write queue depth (always available).
    pub fn set_db_queue_depth(&mut self, depth: u64) {
        self.db_queue_depth = depth;
    }

    /// Tick while processing - returns true when complete.
    pub fn tick(&mut self, daemon: &TaskDaemon) -> bool {
        if !self.processing {
            return false;
        }

        let status = daemon.status();

        // Update progress
        let completed = status.total_processed;
        let total = status.session_queued;
        if total > 0 {
            self.progress = Some(completed as f32 / total as f32);
            self.progress_detail = Some(format!("{} / {}", completed, total));
        }

        // Track when daemon starts working
        if status.state == DaemonStateSnapshot::Working {
            self.seen_working = true;
        }

        // Check for completion
        if self.seen_working && status.pending == 0 {
            match status.state {
                DaemonStateSnapshot::Idle | DaemonStateSnapshot::Completed => {
                    return true;
                }
                DaemonStateSnapshot::Working => {}
            }
        }

        false
    }

    /// Handle keyboard input.
    pub fn handle_key(&self, key: crossterm::event::KeyEvent) -> IntakeConfirmationAction {
        // Ignore input while processing
        if self.processing {
            return IntakeConfirmationAction::None;
        }

        match key.code {
            KeyCode::Enter => IntakeConfirmationAction::Confirmed,
            KeyCode::Esc => IntakeConfirmationAction::Skipped,
            _ => IntakeConfirmationAction::None,
        }
    }

    /// Create IndexFileFromPath mutations for all unindexed files.
    ///
    /// These mutations just contain the path - metadata extraction happens
    /// on the worker thread, not the UI thread.
    pub fn create_index_mutations(&self) -> Vec<Mutation> {
        let mutations: Vec<Mutation> = self
            .paths
            .iter()
            .map(|path| Mutation::IndexFileFromPath {
                path: path.clone(),
                source: self.source.clone(),
            })
            .collect();

        let _ = log_message(&format!(
            "IntakeConfirmation: created {} IndexFileFromPath mutations",
            mutations.len()
        ));

        mutations
    }

    /// Format bytes for human-readable display.
    fn format_bytes(bytes: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if bytes >= GB {
            format!("{:.1} GB", bytes as f64 / GB as f64)
        } else if bytes >= MB {
            format!("{:.1} MB", bytes as f64 / MB as f64)
        } else if bytes >= KB {
            format!("{:.1} KB", bytes as f64 / KB as f64)
        } else {
            format!("{} bytes", bytes)
        }
    }
}

/// Check if a path is an audio file.
fn is_audio_file(path: &std::path::Path) -> bool {
    use crate::config::AUDIO_EXTENSIONS;

    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// Render the intake confirmation modal.
pub fn render(f: &mut Frame, area: Rect, state: &IntakeConfirmationState) {
    if state.processing {
        render_processing(f, area, state);
    } else {
        render_confirmation(f, area, state);
    }
}

/// Render the confirmation dialog (before user confirms).
fn render_confirmation(f: &mut Frame, area: Rect, state: &IntakeConfirmationState) {
    let dialog_width = 55.min(area.width.saturating_sub(4));
    let dialog_height = 14.min(area.height.saturating_sub(4));

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let size_str = IntakeConfirmationState::format_bytes(state.total_bytes);

    let lines = vec![
        Line::from(""),
        Line::from(format!("Found {} files not in index.", state.file_count)).style(
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
        Line::from(format!("Total size: {}", size_str)).style(
            Style::default().fg(Color::White),
        ),
        Line::from(format!("Across {} directories", state.directory_count)).style(
            Style::default().fg(Color::DarkGray),
        ),
        Line::from(""),
        Line::from("This will read metadata from all files").style(
            Style::default().fg(Color::White),
        ),
        Line::from("and add them to your library index.").style(
            Style::default().fg(Color::White),
        ),
        Line::from(""),
        Line::from("[Enter] Index Files    [Esc] Skip for Now")
            .style(Style::default().fg(Color::Cyan)),
    ];

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Unindexed Files Detected ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .alignment(Alignment::Center);

    f.render_widget(paragraph, dialog_area);
}

/// Render the processing view (indexing in progress).
fn render_processing(f: &mut Frame, area: Rect, state: &IntakeConfirmationState) {
    // Calculate heights for components
    let has_queue_depth = state.db_queue_depth > 0;
    let has_worker_stats = state.worker_stats.is_some();
    let has_db_stats = state.db_stats.is_some();
    let stats_height = if has_queue_depth { 1 } else { 0 }
        + if has_worker_stats { 2 } else { 0 }
        + if has_db_stats { 1 } else { 0 };

    let dialog_width = 70.min(area.width.saturating_sub(4));
    let dialog_height = (10 + stats_height).min(area.height.saturating_sub(4)) as u16;

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let mut lines = vec![
        Line::from(""),
        Line::from(format!("Indexing {} files...", state.file_count)).style(
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
    ];

    // Progress bar
    if let Some(progress) = state.progress {
        let bar_width = 40;
        let filled = (progress * bar_width as f32) as usize;
        let empty = bar_width - filled;
        let bar = format!("[{}{}]", "=".repeat(filled), " ".repeat(empty));
        lines.push(Line::from(bar).style(Style::default().fg(Color::Yellow)));

        if let Some(ref detail) = state.progress_detail {
            lines.push(Line::from(detail.clone()).style(Style::default().fg(Color::DarkGray)));
        }
    }

    lines.push(Line::from(""));

    // Pending DB writes (always shown when backlog exists)
    if has_queue_depth {
        let label_color = Color::Rgb(245, 28, 153);
        lines.push(Line::from(vec![
            Span::styled(
                format!("[{} pending DB writes queued]", state.db_queue_depth),
                Style::default().fg(label_color),
            ),
        ]));
    }

    // Worker stats
    if let Some(ref stats) = state.worker_stats {
        let label_color = Color::Rgb(245, 28, 153);
        let avg_task_color = if stats.avg_task_ms < 100 {
            Color::Green
        } else if stats.avg_task_ms < 500 {
            Color::Yellow
        } else {
            Color::Red
        };

        lines.push(Line::from(vec![
            Span::styled("Tasks: ", Style::default().fg(label_color)),
            Span::styled(format!("{}", stats.tasks_completed), Style::default().fg(Color::Cyan)),
            Span::styled(" | ", Style::default().fg(Color::DarkGray)),
            Span::styled("Avg: ", Style::default().fg(label_color)),
            Span::styled(format!("{}ms", stats.avg_task_ms), Style::default().fg(avg_task_color)),
            Span::styled(" | ", Style::default().fg(Color::DarkGray)),
            Span::styled("Threads: ", Style::default().fg(label_color)),
            Span::styled(format!("{}", stats.active_threads), Style::default().fg(Color::White)),
        ]));

        let queue_wait_color = if stats.queue_wait_avg_ms < 5 {
            Color::Green
        } else if stats.queue_wait_avg_ms < 20 {
            Color::Yellow
        } else {
            Color::Red
        };

        lines.push(Line::from(vec![
            Span::styled("Q Wait: ", Style::default().fg(label_color)),
            Span::styled(format!("{}avg", stats.queue_wait_avg_ms), Style::default().fg(queue_wait_color)),
            Span::styled("/", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}max ms", stats.queue_wait_max_ms), Style::default().fg(Color::White)),
            Span::styled(" | ", Style::default().fg(Color::DarkGray)),
            Span::styled("Reads: ", Style::default().fg(label_color)),
            Span::styled(format!("{}", stats.avg_db_read_us), Style::default().fg(Color::Green)),
            Span::styled("/", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", stats.median_db_read_us), Style::default().fg(Color::Cyan)),
            Span::styled("/", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}µs", stats.max_db_read_us), Style::default().fg(Color::White)),
        ]));
    }

    // DB stats
    if let Some(ref stats) = state.db_stats {
        let queue_color = if stats.queue_depth > 100 {
            Color::Red
        } else if stats.queue_depth > 0 {
            Color::Yellow
        } else {
            Color::Green
        };

        lines.push(Line::from(vec![
            Span::styled("Writes: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", stats.total_writes), Style::default().fg(Color::Cyan)),
            Span::styled(" | ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:.1}/s", stats.writes_per_sec), Style::default().fg(Color::Green)),
            Span::styled(" | Q: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", stats.queue_depth), Style::default().fg(queue_color)),
        ]));
    }

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Indexing Files ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Center);

    f.render_widget(paragraph, dialog_area);
}
