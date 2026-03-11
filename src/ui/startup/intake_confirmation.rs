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

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::ui::input::InputAction;
use crate::ui::widgets::centered_rect_fixed;

use crate::corpus::paths;
use crate::db::types::Zone;
use crate::db::ReadOnlyDb;
use crate::logging::log_general;
use crate::meta::mutations::indexing::IndexFileFromPathMutation;
use crate::meta::mutations::Mutation;
/// Where the intake confirmation was triggered from.
///
/// Replaces the old `zone: String` for post-action routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IntakeSource {
    /// Triggered at startup after eyeballing completes
    Startup,
    /// Triggered from Health Insights "Index unindexed" action
    Health,
    /// Triggered from Inbox view or inbox lateral navigation
    Inbox,
}

/// A directory group for display purposes
#[derive(Debug, Clone, serde::Serialize)]
pub struct DirectoryGroup {
    /// Display path (relative to corpus root)
    pub display_path: String,
    /// Filenames within this directory
    pub filenames: Vec<String>,
    /// Zone this directory belongs to
    pub zone: Zone,
}

/// File entry with path for indexing.
#[derive(Debug, Clone, serde::Serialize)]
pub struct UnindexedFileEntry {
    /// Absolute path for indexing
    pub abs_path: PathBuf,
    /// Zone this file belongs to (for mutation creation)
    pub zone: Zone,
}

/// State for the intake confirmation modal.
#[derive(Debug, serde::Serialize)]
pub struct IntakeConfirmationState {
    /// Number of unindexed files detected
    pub file_count: usize,
    /// Total bytes to read (sum of file sizes)
    pub total_bytes: u64,
    /// Files to index, keyed by inode
    pub files: Vec<UnindexedFileEntry>,
    /// Where this intake was triggered from (for post-action routing)
    pub source: IntakeSource,
    /// Whether this state contains files from multiple zones (corpus + inbox)
    pub multi_zone: bool,
    /// Number of directories containing unindexed files
    pub _directory_count: usize,
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
    /// Gather intake confirmation state for a specific zone.
    ///
    /// Queries unindexed signals for the given zone, verifies file existence
    /// on disk, and groups files by directory for display.
    ///
    /// Returns None if there are no unindexed files.
    pub fn gather_zone<Z: crate::zones::AudioZone>(
        read_db: &ReadOnlyDb<'_>,
        source: IntakeSource,
    ) -> Option<Self> {
        let unindexed = match read_db.get_unindexed_signals_for::<Z>() {
            Ok(u) => u,
            Err(e) => {
                crate::logging::log_error(format!(
                    "IntakeConfirmation::gather_zone<{}>: query failed: {:?}",
                    Z::ZONE_STR, e
                ));
                return None;
            }
        };

        log_general(format!(
            "IntakeConfirmation::gather_zone<{}>: found {} unindexed signals",
            Z::ZONE_STR,
            unindexed.len()
        ));

        if unindexed.is_empty() {
            return None;
        }

        let resolver = paths::get_resolver();
        let mut files: Vec<UnindexedFileEntry> = Vec::new();
        let mut total_bytes: u64 = 0;
        let mut directories: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        let mut dir_to_files: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for (_inode, rel_path_str) in &unindexed {
            let rel_path = std::path::Path::new(rel_path_str);
            let abs_path = resolver.resolve(rel_path);

            if abs_path.exists() && abs_path.is_file() {
                if let Ok(meta) = std::fs::metadata(&abs_path) {
                    total_bytes += meta.len();
                }

                if let Some(parent) = abs_path.parent() {
                    directories.insert(parent.to_path_buf());
                }

                if let (Some(parent), Some(filename)) = (rel_path.parent(), rel_path.file_name()) {
                    let dir_str = parent.to_string_lossy().to_string();
                    let file_str = filename.to_string_lossy().to_string();
                    dir_to_files.entry(dir_str).or_default().push(file_str);
                }

                files.push(UnindexedFileEntry {
                    abs_path,
                    zone: Z::ZONE,
                });
            }
        }

        if files.is_empty() {
            return None;
        }

        let grouped_files: Vec<DirectoryGroup> = dir_to_files
            .into_iter()
            .map(|(dir, mut filenames)| {
                filenames.sort();
                DirectoryGroup {
                    display_path: dir,
                    filenames,
                    zone: Z::ZONE,
                }
            })
            .collect();

        log_general(format!(
            "IntakeConfirmation ({}): gathered {} files ({} bytes) from {} directories",
            Z::ZONE_STR,
            files.len(),
            total_bytes,
            directories.len()
        ));

        Some(Self {
            file_count: files.len(),
            total_bytes,
            files,
            source,
            multi_zone: false,
            _directory_count: directories.len(),
            grouped_files,
            scroll_offset: 0,
        })
    }

    /// Gather intake confirmation state for startup: checks both corpus AND inbox.
    ///
    /// Queries unindexed signals for both zones, merging results with corpus
    /// groups first, then inbox groups.
    ///
    /// Returns None if there are no unindexed files in either zone.
    pub fn gather_startup(read_db: &ReadOnlyDb<'_>) -> Option<Self> {
        use crate::zones::{CorpusZone, InboxZone};
        let corpus_state = Self::gather_zone::<CorpusZone>(read_db, IntakeSource::Startup);
        let inbox_state = Self::gather_zone::<InboxZone>(read_db, IntakeSource::Startup);

        match (corpus_state, inbox_state) {
            (None, None) => None,
            (Some(mut state), None) => {
                state.source = IntakeSource::Startup;
                Some(state)
            }
            (None, Some(mut state)) => {
                state.source = IntakeSource::Startup;
                Some(state)
            }
            (Some(corpus), Some(inbox)) => {
                // Merge: corpus groups first, then inbox groups
                let mut files = corpus.files;
                files.extend(inbox.files);

                let mut grouped_files = corpus.grouped_files;
                grouped_files.extend(inbox.grouped_files);

                let file_count = files.len();
                let total_bytes = corpus.total_bytes + inbox.total_bytes;
                let directory_count = corpus._directory_count + inbox._directory_count;

                log_general(format!(
                    "IntakeConfirmation (startup): merged {} corpus + {} inbox = {} total files",
                    corpus.file_count, inbox.file_count, file_count
                ));

                Some(Self {
                    file_count,
                    total_bytes,
                    files,
                    source: IntakeSource::Startup,
                    multi_zone: true,
                    _directory_count: directory_count,
                    grouped_files,
                    scroll_offset: 0,
                })
            }
        }
    }

    /// Compute total number of lines in the file list display
    fn total_list_lines(&self) -> usize {
        let group_lines: usize = self
            .grouped_files
            .iter()
            .map(|g| 1 + g.filenames.len()) // 1 for directory header + files
            .sum();
        if self.multi_zone {
            // Add 2 lines per zone section header (label + blank separator)
            let zone_count = {
                let mut zones = Vec::new();
                for g in &self.grouped_files {
                    if zones.last() != Some(&g.zone) {
                        zones.push(g.zone);
                    }
                }
                zones.len()
            };
            group_lines + zone_count * 2
        } else {
            group_lines
        }
    }

    /// Handle semantic input action.
    pub fn handle_input(
        &mut self,
        action: &InputAction,
        visible_height: usize,
    ) -> IntakeConfirmationAction {
        match action {
            InputAction::Confirm => IntakeConfirmationAction::Confirmed,
            InputAction::Cancel => IntakeConfirmationAction::Skipped,
            InputAction::NavUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                IntakeConfirmationAction::None
            }
            InputAction::NavDown => {
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
    /// These mutations contain the absolute path - metadata extraction happens
    /// on the worker thread, not the UI thread.
    pub fn create_index_mutations(&self) -> Vec<Mutation> {
        let mutations: Vec<Mutation> = self
            .files
            .iter()
            .map(|entry| {
                Mutation::IndexFileFromPath(IndexFileFromPathMutation {
                    path: entry.abs_path.clone(),
                    zone: entry.zone.as_str().to_string(),
                })
            })
            .collect();

        log_general(format!(
            "IntakeConfirmation: created {} IndexFileFromPath mutations",
            mutations.len()
        ));

        mutations
    }

}

/// Compute the visible height for the file list given an area.
/// Used by callers to pass to handle_input for scroll bounds.
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
    let size_str = crate::ui::helpers::format_bytes(state.total_bytes);
    let header_lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} files", state.file_count),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to index  "),
            Span::styled(
                format!("({})", size_str),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(""),
    ];
    f.render_widget(Paragraph::new(header_lines), chunks[0]);

    // Build file list lines with directory grouping
    let mut list_lines: Vec<Line> = Vec::new();
    let mut last_zone: Option<Zone> = None;
    for group in &state.grouped_files {
        // Zone section header in multi-zone mode
        if state.multi_zone && last_zone != Some(group.zone) {
            if last_zone.is_some() {
                list_lines.push(Line::from("")); // separator between zones
            }
            let zone_label = match group.zone {
                Zone::Corpus => "--- Corpus ---",
                Zone::Inbox => "--- Inbox ---",
                _ => "---",
            };
            list_lines.push(Line::from(Span::styled(
                zone_label,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            last_zone = Some(group.zone);
        }
        // Directory header
        list_lines.push(Line::from(Span::styled(
            format!("{}/", group.display_path),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
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

    f.render_widget(Paragraph::new(visible_lines).block(list_block), chunks[1]);

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
