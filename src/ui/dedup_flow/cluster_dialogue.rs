//! Cluster Dialogue UI
//!
//! Presents directory set clusters for keeper selection. Each cluster represents
//! a set of directories that share EXACTLY the same duplicates. The librarian
//! selects which directory to keep; all others become pending deletes.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};
use std::path::Path;

use crate::deduplication::{
    compute_directory_set_clusters, find_divergence_root, generate_cluster_changes,
    ClusterDecision, ConflictSet, DeduplicationSession,
};

/// Actions returned from the cluster dialogue
#[derive(Debug, Clone)]
pub enum ClusterDialogueAction {
    /// No action needed
    None,
    /// Continue processing (internal state change)
    Continue,
    /// Ready to show bulk review prompt
    ShowBulkPrompt,
    /// Ready to show session review
    ShowSessionReview,
    /// User cancelled the workflow
    Cancel,
    /// Status message to display
    StatusMessage(String),
}

/// State for the cluster selection dialogue
#[derive(Debug, Clone)]
pub struct ClusterDialogueState {
    /// The deduplication session
    pub session: DeduplicationSession,
    /// Currently selected directory index within current cluster
    selected_dir_index: usize,
    /// List widget state for directory selection
    dir_list_state: ListState,
    /// Corpus root path for computing relative paths
    corpus_root: String,
    /// Stash root for delete targets
    stash_root: String,
}

impl ClusterDialogueState {
    /// Create a new cluster dialogue from conflict sets
    pub fn new(
        conflict_sets: Vec<ConflictSet>,
        session_id: String,
        corpus_root: String,
        stash_root: String,
    ) -> Self {
        let clusters = compute_directory_set_clusters(&conflict_sets);
        let divergence_root = find_divergence_root(&conflict_sets);

        let session = DeduplicationSession {
            session_id,
            clusters,
            current_index: 0,
            decisions: Vec::new(),
            bulk_phase_complete: false,
            divergence_root,
            conflict_sets,
            auto_ignore: Default::default(),
        };

        let mut state = Self {
            session,
            selected_dir_index: 0,
            dir_list_state: ListState::default(),
            corpus_root,
            stash_root,
        };
        state.dir_list_state.select(Some(0));
        state
    }

    /// Create a new cluster dialogue directly from clusters.
    /// Used by sleuthing (directory-set based deduplication) which builds
    /// clusters directly from selected directories.
    pub fn new_from_clusters(
        clusters: Vec<crate::deduplication::DirectorySetCluster>,
        session_id: String,
        corpus_root: String,
        stash_root: String,
    ) -> Self {
        let session = DeduplicationSession {
            session_id,
            clusters,
            current_index: 0,
            decisions: Vec::new(),
            bulk_phase_complete: false,
            divergence_root: String::new(), // Not needed for directory-set mode
            conflict_sets: Vec::new(),      // Not used - tracks are in cluster.tracks_by_dir
            auto_ignore: Default::default(),
        };

        let mut state = Self {
            session,
            selected_dir_index: 0,
            dir_list_state: ListState::default(),
            corpus_root,
            stash_root,
        };
        state.dir_list_state.select(Some(0));
        state
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> ClusterDialogueAction {
        match key.code {
            KeyCode::Up => {
                self.move_selection(-1);
                ClusterDialogueAction::Continue
            }
            KeyCode::Down => {
                self.move_selection(1);
                ClusterDialogueAction::Continue
            }
            KeyCode::Enter => self.select_keeper(),
            KeyCode::Tab => {
                // Skip this cluster (defer to end)
                self.skip_cluster()
            }
            KeyCode::BackTab => {
                // Go back to previous cluster
                self.prev_cluster()
            }
            KeyCode::Esc => {
                // Go to session review (or cancel if no decisions)
                if self.session.decisions.is_empty() {
                    ClusterDialogueAction::Cancel
                } else {
                    ClusterDialogueAction::ShowSessionReview
                }
            }
            _ => ClusterDialogueAction::None,
        }
    }

    fn move_selection(&mut self, delta: i32) {
        if let Some(cluster) = self.session.current_cluster() {
            let max = cluster.directory_set.len();
            if max == 0 {
                return;
            }
            let new_idx = if delta < 0 {
                self.selected_dir_index.saturating_sub((-delta) as usize)
            } else {
                (self.selected_dir_index + delta as usize).min(max - 1)
            };
            self.selected_dir_index = new_idx;
            self.dir_list_state.select(Some(new_idx));
        }
    }

    fn select_keeper(&mut self) -> ClusterDialogueAction {
        use crate::config;

        let _ = config::log_message(&format!(
            "=== select_keeper called: cluster_index={} selected_dir_index={} ===",
            self.session.current_index,
            self.selected_dir_index
        ));

        let cluster = match self.session.current_cluster() {
            Some(c) => c.clone(),
            None => {
                let _ = config::log_message("[select_keeper] No current cluster - going to session review");
                return ClusterDialogueAction::ShowSessionReview;
            }
        };

        let _ = config::log_message(&format!(
            "[select_keeper] cluster.directory_set={:?} cluster.tracks_by_dir.keys={:?}",
            cluster.directory_set,
            cluster.tracks_by_dir.keys().collect::<Vec<_>>()
        ));

        if self.selected_dir_index >= cluster.directory_set.len() {
            let _ = config::log_message("[select_keeper] Invalid selection index");
            return ClusterDialogueAction::StatusMessage("Invalid selection".to_string());
        }

        let keeper_dir = cluster.directory_set[self.selected_dir_index].clone();
        let _ = config::log_message(&format!(
            "[select_keeper] keeper_dir={} corpus_root={} stash_root={}",
            keeper_dir, self.corpus_root, self.stash_root
        ));

        // Generate pending changes for this decision
        let _ = config::log_message("[select_keeper] Calling generate_cluster_changes...");
        let changes = generate_cluster_changes(
            &cluster,
            &keeper_dir,
            &self.session.conflict_sets,
            Path::new(&self.corpus_root),
            Path::new(&self.stash_root),
            &self.session.session_id,
        );

        let _ = config::log_message(&format!(
            "[select_keeper] generate_cluster_changes returned {} changes",
            changes.len()
        ));
        for (i, change) in changes.iter().enumerate() {
            let _ = config::log_message(&format!(
                "[select_keeper]   change[{}]: {:?} {} -> {}",
                i,
                change.change_type,
                change.source_path,
                change.target_path.as_deref().unwrap_or("(none)")
            ));
        }

        // Record the decision
        self.session.decisions.push(ClusterDecision {
            cluster: cluster.clone(),
            keeper_dir: Some(keeper_dir.clone()),
            pending_changes: changes,
        });

        let _ = config::log_message(&format!(
            "[select_keeper] Decision recorded. Total decisions now: {}",
            self.session.decisions.len()
        ));

        // Advance to next cluster
        self.session.current_index += 1;
        self.selected_dir_index = 0;
        self.dir_list_state.select(Some(0));

        // Check if we're done or should offer bulk review
        if self.session.is_complete() {
            let _ = config::log_message("[select_keeper] Session complete - going to session review");
            ClusterDialogueAction::ShowSessionReview
        } else if self.should_offer_bulk_review() {
            let _ = config::log_message("[select_keeper] Offering bulk review");
            ClusterDialogueAction::ShowBulkPrompt
        } else {
            let _ = config::log_message("[select_keeper] Continuing to next cluster");
            ClusterDialogueAction::Continue
        }
    }

    fn skip_cluster(&mut self) -> ClusterDialogueAction {
        // Move current cluster to end by recording a decision without a keeper
        if let Some(cluster) = self.session.current_cluster().cloned() {
            self.session.decisions.push(ClusterDecision {
                cluster,
                keeper_dir: None,
                pending_changes: Vec::new(),
            });
            self.session.current_index += 1;
            self.selected_dir_index = 0;
            self.dir_list_state.select(Some(0));

            if self.session.is_complete() {
                ClusterDialogueAction::ShowSessionReview
            } else {
                ClusterDialogueAction::Continue
            }
        } else {
            ClusterDialogueAction::ShowSessionReview
        }
    }

    fn prev_cluster(&mut self) -> ClusterDialogueAction {
        // Go back to previous cluster by undoing the last decision
        if self.session.current_index > 0 && !self.session.decisions.is_empty() {
            // Remove the last decision
            self.session.decisions.pop();
            // Go back one cluster
            self.session.current_index -= 1;
            // Reset selection
            self.selected_dir_index = 0;
            self.dir_list_state.select(Some(0));
            ClusterDialogueAction::Continue
        } else {
            // Already at first cluster, can't go back
            ClusterDialogueAction::None
        }
    }

    fn should_offer_bulk_review(&self) -> bool {
        crate::deduplication::should_offer_bulk_review(
            &self.session.clusters,
            self.session.current_index,
        )
    }

    /// Render the cluster dialogue
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Cluster header
                Constraint::Min(10),    // Directory list
                Constraint::Length(4),  // Sample preview
                Constraint::Length(4),  // Controls
            ])
            .split(area);

        self.render_header(f, chunks[0]);
        self.render_directory_list(f, chunks[1]);
        self.render_sample(f, chunks[2]);
        self.render_controls(f, chunks[3]);
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let (title, subtitle) = if let Some(cluster) = self.session.current_cluster() {
            let title = format!(
                "Cluster {}/{} | {}-directory conflicts",
                self.session.current_index + 1,
                self.session.clusters.len(),
                cluster.magnitude
            );
            let subtitle = format!(
                "{} fingerprints | {} files affected",
                cluster.fingerprints.len(),
                cluster.file_count
            );
            (title, subtitle)
        } else {
            ("No clusters to process".to_string(), String::new())
        };

        let header = Paragraph::new(vec![
            Line::from(Span::styled(
                title,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(subtitle, Style::default().fg(Color::DarkGray))),
        ])
        .block(Block::default().borders(Borders::ALL));

        f.render_widget(header, area);
    }

    fn render_directory_list(&mut self, f: &mut Frame, area: Rect) {
        let cluster = match self.session.current_cluster() {
            Some(c) => c,
            None => {
                let empty = Paragraph::new("No cluster selected")
                    .block(Block::default().borders(Borders::ALL).title("Directories"));
                f.render_widget(empty, area);
                return;
            }
        };

        // Collect detailed file info per directory from the cluster's tracks_by_dir
        let mut dir_details: std::collections::HashMap<String, Vec<(String, Option<i64>, Option<i64>, i64)>> =
            std::collections::HashMap::new();
        for (dir, tracks) in &cluster.tracks_by_dir {
            for track in tracks {
                let filename = std::path::Path::new(&track.path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| track.path.clone());
                dir_details.entry(dir.clone()).or_default().push((
                    filename,
                    track.bitrate_kbps.map(|b| b as i64),
                    track.sample_rate.map(|s| s as i64),
                    track.file_size,
                ));
            }
        }

        // Find max bitrate across all directories for highlighting
        let max_bitrate: Option<i64> = dir_details.values()
            .flatten()
            .filter_map(|(_, br, _, _)| *br)
            .max();

        let items: Vec<ListItem> = cluster
            .directory_set
            .iter()
            .enumerate()
            .map(|(i, dir)| {
                let is_selected = i == self.selected_dir_index;
                let prefix = if is_selected { ">> " } else { "   " };
                let suffix = if is_selected { " [KEEP]" } else { "" };

                let style = if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                let details = dir_details.get(dir);
                let file_count = details.map(|d| d.len()).unwrap_or(0);

                // Build detail lines for each file
                let mut lines = vec![
                    Line::from(vec![
                        Span::styled(prefix, style),
                        Span::styled(dir.clone(), style),
                        Span::styled(
                            format!("  ({} files)", file_count),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(suffix, Style::default().fg(Color::Green)),
                    ]),
                ];

                // Add file details with bitrate highlighting
                if let Some(files) = details {
                    for (filename, bitrate, sample_rate, file_size) in files.iter().take(5) {
                        let size_str = if *file_size >= 1_000_000 {
                            format!("{:.1}MB", *file_size as f64 / 1_000_000.0)
                        } else {
                            format!("{}KB", *file_size / 1000)
                        };

                        // Build line with highlighted bitrate
                        let mut spans = vec![
                            Span::styled(format!("      {} ", filename), Style::default().fg(Color::DarkGray)),
                        ];

                        // Highlight bitrate based on comparison to max
                        if let Some(br) = bitrate {
                            let bitrate_style = match max_bitrate {
                                Some(max) if *br == max => {
                                    // Best bitrate - green and bold
                                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
                                }
                                Some(max) if *br >= max * 90 / 100 => {
                                    // Within 10% of max - cyan
                                    Style::default().fg(Color::Cyan)
                                }
                                Some(max) if *br >= max * 70 / 100 => {
                                    // 70-90% of max - yellow
                                    Style::default().fg(Color::Yellow)
                                }
                                _ => {
                                    // Below 70% of max - red
                                    Style::default().fg(Color::Red)
                                }
                            };
                            spans.push(Span::styled(format!("{}kbps ", br), bitrate_style));
                        }

                        // Sample rate and size in gray
                        let sample_str = sample_rate.map(|s| format!("{}Hz ", s)).unwrap_or_default();
                        spans.push(Span::styled(format!("{}{}", sample_str, size_str), Style::default().fg(Color::DarkGray)));

                        lines.push(Line::from(spans));
                    }
                    if files.len() > 5 {
                        lines.push(Line::from(Span::styled(
                            format!("      ... and {} more files", files.len() - 5),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }

                ListItem::new(lines)
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Directories with duplicates - Select directory to KEEP"),
        );

        f.render_stateful_widget(list, area, &mut self.dir_list_state);
    }

    fn render_sample(&self, f: &mut Frame, area: Rect) {
        let sample = if let Some(cluster) = self.session.current_cluster() {
            // Get a sample track from the cluster's tracks_by_dir
            if let Some((_, tracks)) = cluster.tracks_by_dir.iter().next() {
                if let Some(track) = tracks.first() {
                    let artist = track.artist.as_deref().unwrap_or("Unknown Artist");
                    let title = track.title.as_deref().unwrap_or("Unknown Title");
                    format!("\"{}\" by {}", title, artist)
                } else {
                    "No sample available".to_string()
                }
            } else {
                "No sample available".to_string()
            }
        } else {
            "No cluster selected".to_string()
        };

        let para = Paragraph::new(vec![
            Line::from(Span::styled("Sample:", Style::default().fg(Color::DarkGray))),
            Line::from(sample),
        ])
        .block(Block::default().borders(Borders::ALL).title("Preview"));

        f.render_widget(para, area);
    }

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let controls = vec![
            Line::from(vec![
                Span::styled("Up/Down", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Select | "),
                Span::styled("Enter", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Keep | "),
                Span::styled("Tab", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Skip | "),
                Span::styled("Shift+Tab", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Back | "),
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Review"),
            ]),
        ];

        let para = Paragraph::new(controls).block(Block::default().borders(Borders::ALL));
        f.render_widget(para, area);
    }

    /// Get the session (for passing to next stage)
    pub fn into_session(self) -> DeduplicationSession {
        self.session
    }
}
