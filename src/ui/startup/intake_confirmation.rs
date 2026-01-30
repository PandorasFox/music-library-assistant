//! Intake Confirmation Modal
//!
//! Shows a dialog when unindexed files are detected, asking the user
//! to confirm indexing. This runs after eyeballing completes and before
//! transitioning to the metadata analysis phase.
//!
//! After confirmation, transitions immediately to the progress screen
//! which handles showing indexing + content analysis progress.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::ui::widgets::centered_rect_fixed;

use crate::corpus::db::types::SignalType;
use crate::corpus::db::ReadOnlyDb;
use crate::corpus::mutations::Mutation;
use crate::corpus::paths;
use crate::logging::log_general;

/// A directory group for display purposes
#[derive(Debug, Clone)]
pub struct DirectoryGroup {
    /// Display path (relative to corpus root)
    pub display_path: String,
    /// Filenames within this directory
    pub filenames: Vec<String>,
}

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
    /// Files grouped by directory for display
    pub grouped_files: Vec<DirectoryGroup>,
    /// Scroll offset for file list
    pub scroll_offset: usize,
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
    pub fn gather(read_db: &ReadOnlyDb<'_>, _corpus_root: &std::path::Path, source: &str) -> Option<Self> {
        // Get all UnindexedFile signals - these are pre-computed during Awakening
        let issues = match read_db.get_signals(Some(SignalType::UnindexedFile)) {
            Ok(i) => i,
            Err(e) => {
                crate::logging::log_error(format!(
                    "IntakeConfirmation::gather: query failed: {:?}",
                    e
                ));
                return None;
            }
        };

        log_general(format!(
            "IntakeConfirmation::gather: found {} UnindexedFile signals",
            issues.len()
        ));

        if issues.is_empty() {
            return None;
        }

        // Each UnindexedFile signal has a relative file path as issue_key
        // Resolve to absolute for filesystem operations
        let resolver = paths::get_resolver();
        let mut all_paths: Vec<PathBuf> = Vec::new();
        let mut total_bytes: u64 = 0;
        let mut directories: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        // Group files by their relative directory path for display
        let mut dir_to_files: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for issue in &issues {
            // Resolve relative signal key to absolute path
            let rel_path = std::path::Path::new(&issue.issue_key);
            let abs_path = resolver.resolve(rel_path);

            // Verify file still exists and get size
            if abs_path.exists() && abs_path.is_file() {
                if let Ok(meta) = std::fs::metadata(&abs_path) {
                    total_bytes += meta.len();
                }

                // Track unique directories
                if let Some(parent) = abs_path.parent() {
                    directories.insert(parent.to_path_buf());
                }

                // Group by relative directory for display
                if let (Some(parent), Some(filename)) = (rel_path.parent(), rel_path.file_name()) {
                    let dir_str = parent.to_string_lossy().to_string();
                    let file_str = filename.to_string_lossy().to_string();
                    dir_to_files.entry(dir_str).or_default().push(file_str);
                }

                all_paths.push(abs_path);
            }
        }

        if all_paths.is_empty() {
            return None;
        }

        // Build grouped files list, sorting filenames within each directory
        let grouped_files: Vec<DirectoryGroup> = dir_to_files
            .into_iter()
            .map(|(dir, mut files)| {
                files.sort();
                DirectoryGroup {
                    display_path: dir,
                    filenames: files,
                }
            })
            .collect();

        log_general(format!(
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
            grouped_files,
            scroll_offset: 0,
        })
    }

    /// Compute total number of lines in the file list display
    fn total_list_lines(&self) -> usize {
        self.grouped_files
            .iter()
            .map(|g| 1 + g.filenames.len()) // 1 for directory header + files
            .sum()
    }

    /// Handle keyboard input.
    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent, visible_height: usize) -> IntakeConfirmationAction {
        match key.code {
            KeyCode::Enter => IntakeConfirmationAction::Confirmed,
            KeyCode::Esc => IntakeConfirmationAction::Skipped,
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                IntakeConfirmationAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max_scroll = self.total_list_lines().saturating_sub(visible_height);
                if self.scroll_offset < max_scroll {
                    self.scroll_offset += 1;
                }
                IntakeConfirmationAction::None
            }
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

        log_general(format!(
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

/// Compute the visible height for the file list given an area.
/// Used by callers to pass to handle_key for scroll bounds.
pub fn compute_list_visible_height(area: Rect) -> usize {
    // Dialog sizing matches render()
    let dialog_height = 24.min(area.height.saturating_sub(2));
    // Subtract: border (2) + header (3) + footer (2)
    dialog_height.saturating_sub(7) as usize
}

/// Render the intake confirmation modal.
pub fn render(f: &mut Frame, area: Rect, state: &IntakeConfirmationState) {
    let dialog_width = 70.min(area.width.saturating_sub(4));
    let dialog_height = 24.min(area.height.saturating_sub(2));

    let dialog_area = centered_rect_fixed(dialog_width, dialog_height, area);
    f.render_widget(Clear, dialog_area);

    // Outer block with title
    let block = Block::default()
        .title(" Unindexed Files Detected ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let inner_area = block.inner(dialog_area);
    f.render_widget(block, dialog_area);

    // Split into header, file list, and footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(1),    // File list
            Constraint::Length(2), // Footer
        ])
        .split(inner_area);

    // Header
    let size_str = IntakeConfirmationState::format_bytes(state.total_bytes);
    let header_lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} files", state.file_count),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to index  "),
            Span::styled(format!("({})", size_str), Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(""),
    ];
    f.render_widget(Paragraph::new(header_lines), chunks[0]);

    // Build file list lines with directory grouping
    let mut list_lines: Vec<Line> = Vec::new();
    for group in &state.grouped_files {
        // Directory header
        list_lines.push(Line::from(Span::styled(
            format!("{}/", group.display_path),
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )));
        // Files within directory
        for filename in &group.filenames {
            list_lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(filename.clone(), Style::default().fg(Color::White)),
            ]));
        }
    }

    // Apply scrolling
    let visible_height = chunks[1].height as usize;
    let total_lines = list_lines.len();
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll_offset = state.scroll_offset.min(max_scroll);

    let visible_lines: Vec<Line> = list_lines
        .into_iter()
        .skip(scroll_offset)
        .take(visible_height)
        .collect();

    // Show scroll indicator if needed
    let list_block = if total_lines > visible_height {
        Block::default()
            .title(format!(" [{}/{}] ", scroll_offset + 1, max_scroll + 1))
            .title_alignment(Alignment::Right)
    } else {
        Block::default()
    };

    f.render_widget(
        Paragraph::new(visible_lines).block(list_block),
        chunks[1],
    );

    // Footer
    let footer_lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("[Enter]", Style::default().fg(Color::Cyan)),
            Span::raw(" Index  "),
            Span::styled("[Esc]", Style::default().fg(Color::Cyan)),
            Span::raw(" Skip  "),
            Span::styled("[↑↓]", Style::default().fg(Color::DarkGray)),
            Span::raw(" Scroll"),
        ]),
    ];
    f.render_widget(
        Paragraph::new(footer_lines).alignment(Alignment::Center),
        chunks[2],
    );
}
