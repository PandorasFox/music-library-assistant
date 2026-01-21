//! Intake Confirmation Modal
//!
//! Shows a dialog when unindexed files are detected, asking the user
//! to confirm indexing. This runs after eyeballing completes and before
//! transitioning to the metadata analysis phase.
//!
//! After confirmation, transitions immediately to the progress screen
//! which handles showing indexing + content analysis progress.

use std::path::PathBuf;

use crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::corpus::db::types::HealthIssueType;
use crate::corpus::db::Database;
use crate::corpus::mutations::Mutation;
use crate::config::log_message;

/// State for the intake confirmation modal.
#[derive(Debug)]
pub struct IntakeConfirmationState {
    /// Number of unindexed files detected
    pub file_count: usize,
    /// Total bytes to read (sum of file sizes)
    pub total_bytes: u64,
    /// Paths to index (gathered from UnindexedFile signals)
    pub paths: Vec<PathBuf>,
    /// Source identifier ("corpus" or "legacy")
    pub source: String,
    /// Number of directories containing unindexed files
    pub directory_count: usize,
}

/// Action returned from handling input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeConfirmationAction {
    /// No action, continue showing modal
    None,
    /// User confirmed - queue mutations and transition to progress screen
    Confirmed,
    /// User skipped - proceed to Insights without indexing
    Skipped,
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
        })
    }

    /// Handle keyboard input.
    pub fn handle_key(&self, key: crossterm::event::KeyEvent) -> IntakeConfirmationAction {
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

/// Render the intake confirmation modal.
pub fn render(f: &mut Frame, area: Rect, state: &IntakeConfirmationState) {
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
