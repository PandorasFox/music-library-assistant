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
    /// Whether to show detailed file list
    show_details: bool,
    /// Corpus root path for computing relative paths
    corpus_root: String,
    /// Lost files root for delete targets
    lost_files_root: String,
}

impl ClusterDialogueState {
    /// Create a new cluster dialogue from conflict sets
    pub fn new(
        conflict_sets: Vec<ConflictSet>,
        session_id: String,
        corpus_root: String,
        lost_files_root: String,
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
            show_details: false,
            corpus_root,
            lost_files_root,
        };
        state.dir_list_state.select(Some(0));
        state
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> ClusterDialogueAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                ClusterDialogueAction::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                ClusterDialogueAction::Continue
            }
            KeyCode::Enter => self.select_keeper(),
            KeyCode::Char('s') | KeyCode::Char('S') => {
                // Skip this cluster (defer to end)
                self.skip_cluster()
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                self.show_details = !self.show_details;
                ClusterDialogueAction::Continue
            }
            KeyCode::Esc => {
                // Go to session review (or cancel if no decisions)
                if self.session.decisions.is_empty() {
                    ClusterDialogueAction::Cancel
                } else {
                    ClusterDialogueAction::ShowSessionReview
                }
            }
            KeyCode::Char('q') | KeyCode::Char('Q') => ClusterDialogueAction::Cancel,
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
        let cluster = match self.session.current_cluster() {
            Some(c) => c.clone(),
            None => return ClusterDialogueAction::ShowSessionReview,
        };

        if self.selected_dir_index >= cluster.directory_set.len() {
            return ClusterDialogueAction::StatusMessage("Invalid selection".to_string());
        }

        let keeper_dir = cluster.directory_set[self.selected_dir_index].clone();

        // Generate pending changes for this decision
        let changes = generate_cluster_changes(
            &cluster,
            &keeper_dir,
            &self.session.conflict_sets,
            Path::new(&self.corpus_root),
            Path::new(&self.lost_files_root),
            &self.session.session_id,
        );

        // Record the decision
        self.session.decisions.push(ClusterDecision {
            cluster: cluster.clone(),
            keeper_dir: Some(keeper_dir.clone()),
            pending_changes: changes,
        });

        // Advance to next cluster
        self.session.current_index += 1;
        self.selected_dir_index = 0;
        self.dir_list_state.select(Some(0));

        // Check if we're done or should offer bulk review
        if self.session.is_complete() {
            ClusterDialogueAction::ShowSessionReview
        } else if self.should_offer_bulk_review() {
            ClusterDialogueAction::ShowBulkPrompt
        } else {
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

        // Count files per directory for this cluster
        let mut dir_file_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for cs in &self.session.conflict_sets {
            let mut cs_dirs: Vec<_> = cs.conflict_dirs.clone();
            cs_dirs.sort();
            if cs_dirs == cluster.directory_set {
                for (dir, tracks) in &cs.tracks_by_dir {
                    *dir_file_counts.entry(dir.clone()).or_default() += tracks.len();
                }
            }
        }

        let items: Vec<ListItem> = cluster
            .directory_set
            .iter()
            .enumerate()
            .map(|(i, dir)| {
                let count = dir_file_counts.get(dir).copied().unwrap_or(0);
                let is_selected = i == self.selected_dir_index;

                let prefix = if is_selected { ">> " } else { "   " };
                let suffix = if is_selected { "  [KEEP]" } else { "" };

                let style = if is_selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                ListItem::new(Line::from(vec![
                    Span::styled(prefix, style),
                    Span::styled(format!("[{}] ", i + 1), Style::default().fg(Color::DarkGray)),
                    Span::styled(dir.clone(), style),
                    Span::styled(
                        format!("  ({} files)", count),
                        Style::default().fg(Color::DarkGray),
                    ),
                    Span::styled(suffix, Style::default().fg(Color::Green)),
                ]))
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
            // Get first fingerprint and show a sample track
            if let Some(fp) = cluster.fingerprints.first() {
                if let Some(cs) = self
                    .session
                    .conflict_sets
                    .iter()
                    .find(|c| &c.fingerprint == fp)
                {
                    if let Some((_, tracks)) = cs.tracks_by_dir.iter().next() {
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
                Span::styled("S", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Skip | "),
                Span::styled("D", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Details"),
            ]),
            Line::from(vec![
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Review pending changes | "),
                Span::styled("Q", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Cancel"),
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
