//! Album Artist Cluster View
//!
//! Two-pane layout for album_artist canonicalization, similar to canon_flow/cluster_view.rs.
//! Includes quality resolution options for stashing lower-quality variants.

use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::ui::widgets::{PaneConfig, ThreePaneLayout, TwoPaneLayout};

use super::session::{AlbumArtistCanonSession, AlbumArtistDecision};
use crate::corpus::db::{ChangeStatus, ChangeType, PendingChange};
use crate::ui::app::flip_coin;
use crate::ui::shared::{
    analyze_quality, generate_stash_changes, QualityAnalysis,
    QualityResolutionState,
};

/// Which pane is currently focused
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Variants,
    Action,
    QualityOptions,
}

/// Focus within the Action pane
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionFocus {
    NameField,
    SquashButton,
}

/// Actions returned from the cluster view
#[derive(Debug, Clone)]
pub enum AlbumArtistClusterAction {
    None,
    Continue,
    SessionComplete,
    ShowSessionReview,
    StatusMessage(String),
}

/// State for the album artist cluster view
#[derive(Debug, Clone)]
pub struct AlbumArtistClusterState {
    session: AlbumArtistCanonSession,
    focused_pane: Pane,
    selected_indices: HashSet<usize>,
    cursor_idx: usize,
    list_state: ListState,
    action_focus: ActionFocus,
    canonical_name: String,
    text_cursor: usize,
    squash_confirmed: bool,
    status_message: Option<String>,
    /// Quality analysis for current bucket (populated after selection)
    quality_analysis: Option<QualityAnalysis>,
    /// Quality resolution options state
    quality_state: Option<QualityResolutionState>,
}

impl AlbumArtistClusterState {
    /// Create a new cluster view state from a session
    pub fn new(session: AlbumArtistCanonSession) -> Self {
        let canonical_name = session
            .current_bucket()
            .and_then(|b| b.variants.first())
            .map(|v| v.name.clone())
            .unwrap_or_default();
        let text_cursor = canonical_name.len();

        let mut state = Self {
            session,
            focused_pane: Pane::Variants,
            selected_indices: HashSet::new(),
            cursor_idx: 0,
            list_state: ListState::default(),
            action_focus: ActionFocus::NameField,
            canonical_name,
            text_cursor,
            squash_confirmed: false,
            status_message: None,
            quality_analysis: None,
            quality_state: None,
        };
        state.list_state.select(Some(0));
        state
    }

    /// Take ownership of the session
    pub fn into_session(self) -> AlbumArtistCanonSession {
        self.session
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> AlbumArtistClusterAction {
        match key.code {
            KeyCode::Tab => return self.advance_to_next_group(),
            KeyCode::BackTab => return self.go_to_previous_group(),
            KeyCode::Esc => return AlbumArtistClusterAction::ShowSessionReview,
            KeyCode::Right => {
                if matches!(self.focused_pane, Pane::Variants) {
                    self.focused_pane = Pane::Action;
                    return AlbumArtistClusterAction::Continue;
                } else if matches!(self.action_focus, ActionFocus::NameField) {
                    if self.text_cursor < self.canonical_name.len() {
                        self.text_cursor += 1;
                        return AlbumArtistClusterAction::Continue;
                    }
                }
            }
            KeyCode::Left => {
                if matches!(self.focused_pane, Pane::Action) {
                    if matches!(self.action_focus, ActionFocus::NameField) && self.text_cursor > 0 {
                        self.text_cursor -= 1;
                        return AlbumArtistClusterAction::Continue;
                    } else {
                        self.focused_pane = Pane::Variants;
                        return AlbumArtistClusterAction::Continue;
                    }
                }
            }
            _ => {}
        }

        match self.focused_pane {
            Pane::Variants => self.handle_variants_key(key),
            Pane::Action => self.handle_action_key(key),
            Pane::QualityOptions => self.handle_quality_key(key),
        }
    }

    fn handle_quality_key(&mut self, key: KeyEvent) -> AlbumArtistClusterAction {
        match key.code {
            KeyCode::Up => {
                if let Some(ref mut qs) = self.quality_state {
                    qs.move_up();
                }
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Down => {
                if let Some(ref mut qs) = self.quality_state {
                    qs.move_down();
                }
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Char(' ') | KeyCode::Enter => {
                if let Some(ref mut qs) = self.quality_state {
                    qs.toggle_current();
                    self.update_quality_status_message();
                }
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Left => {
                self.focused_pane = Pane::Action;
                AlbumArtistClusterAction::Continue
            }
            _ => AlbumArtistClusterAction::None,
        }
    }

    fn update_quality_status_message(&mut self) {
        if let Some(ref qs) = self.quality_state {
            let selected = qs.get_selected_options();
            if selected.is_empty() {
                self.status_message = Some("No quality resolution options selected".to_string());
            } else {
                let labels: Vec<_> = selected.iter().map(|o| o.short_label()).collect();
                self.status_message = Some(format!("Quality: {}", labels.join(", ")));
            }
        }
    }

    fn handle_variants_key(&mut self, key: KeyEvent) -> AlbumArtistClusterAction {
        match key.code {
            KeyCode::Up => {
                self.move_cursor(-1);
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Down => {
                self.move_cursor(1);
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Char(' ') => {
                self.toggle_selection();
                self.update_canonical_from_selection();
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.toggle_all();
                self.update_canonical_from_selection();
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Enter => {
                self.focused_pane = Pane::Action;
                self.action_focus = ActionFocus::SquashButton;
                AlbumArtistClusterAction::Continue
            }
            _ => AlbumArtistClusterAction::None,
        }
    }

    fn handle_action_key(&mut self, key: KeyEvent) -> AlbumArtistClusterAction {
        match self.action_focus {
            ActionFocus::NameField => self.handle_name_field_key(key),
            ActionFocus::SquashButton => self.handle_squash_button_key(key),
        }
    }

    fn handle_name_field_key(&mut self, key: KeyEvent) -> AlbumArtistClusterAction {
        match key.code {
            KeyCode::Up => AlbumArtistClusterAction::None,
            KeyCode::Down => {
                self.action_focus = ActionFocus::SquashButton;
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Backspace => {
                if self.text_cursor > 0 {
                    self.text_cursor -= 1;
                    self.canonical_name.remove(self.text_cursor);
                }
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Delete => {
                if self.text_cursor < self.canonical_name.len() {
                    self.canonical_name.remove(self.text_cursor);
                }
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Home => {
                self.text_cursor = 0;
                AlbumArtistClusterAction::Continue
            }
            KeyCode::End => {
                self.text_cursor = self.canonical_name.len();
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    return AlbumArtistClusterAction::None;
                }
                self.canonical_name.insert(self.text_cursor, c);
                self.text_cursor += 1;
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Enter => {
                self.action_focus = ActionFocus::SquashButton;
                AlbumArtistClusterAction::Continue
            }
            _ => AlbumArtistClusterAction::None,
        }
    }

    fn handle_squash_button_key(&mut self, key: KeyEvent) -> AlbumArtistClusterAction {
        match key.code {
            KeyCode::Up => {
                self.action_focus = ActionFocus::NameField;
                AlbumArtistClusterAction::Continue
            }
            KeyCode::Down => AlbumArtistClusterAction::None,
            KeyCode::Right => {
                // Navigate to quality options if available
                if self.quality_state.as_ref().map(|s| s.has_options()).unwrap_or(false) {
                    self.focused_pane = Pane::QualityOptions;
                    return AlbumArtistClusterAction::Continue;
                }
                AlbumArtistClusterAction::None
            }
            KeyCode::Char(' ') => self.toggle_squash_state(),
            KeyCode::Enter => {
                let toggle_result = self.toggle_squash_state();
                if self.squash_confirmed {
                    return self.advance_to_next_group();
                }
                toggle_result
            }
            _ => AlbumArtistClusterAction::None,
        }
    }

    fn move_cursor(&mut self, delta: i32) {
        if let Some(bucket) = self.session.current_bucket() {
            let max = bucket.variants.len();
            if max == 0 {
                return;
            }
            let new_idx = if delta < 0 {
                self.cursor_idx.saturating_sub((-delta) as usize)
            } else {
                (self.cursor_idx + delta as usize).min(max - 1)
            };
            self.cursor_idx = new_idx;
            self.list_state.select(Some(new_idx));
        }
    }

    fn toggle_selection(&mut self) {
        if self.selected_indices.contains(&self.cursor_idx) {
            self.selected_indices.remove(&self.cursor_idx);
        } else {
            self.selected_indices.insert(self.cursor_idx);
        }
    }

    fn toggle_all(&mut self) {
        if let Some(bucket) = self.session.current_bucket() {
            let total = bucket.variants.len();
            let selected_count = self.selected_indices.len();
            let majority_selected = selected_count > total / 2;

            let should_deselect = if selected_count * 2 == total {
                flip_coin()
            } else {
                majority_selected
            };

            if should_deselect {
                self.selected_indices.clear();
            } else {
                self.selected_indices = (0..total).collect();
            }
        }
    }

    fn update_canonical_from_selection(&mut self) {
        if let Some(bucket) = self.session.current_bucket() {
            let most_frequent = self
                .selected_indices
                .iter()
                .filter_map(|&idx| bucket.variants.get(idx))
                .max_by_key(|v| v.track_count);

            if let Some(variant) = most_frequent {
                self.canonical_name = variant.name.clone();
                self.text_cursor = self.canonical_name.len();
            }
        }
    }

    fn toggle_squash_state(&mut self) -> AlbumArtistClusterAction {
        if self.selected_indices.len() < 2 {
            return AlbumArtistClusterAction::StatusMessage(
                "Select at least 2 variants to squash".to_string(),
            );
        }

        if self.canonical_name.trim().is_empty() {
            return AlbumArtistClusterAction::StatusMessage(
                "Canonical name cannot be empty".to_string(),
            );
        }

        self.squash_confirmed = !self.squash_confirmed;

        if self.squash_confirmed {
            let affected = self.calculate_affected_tracks();
            self.status_message = Some(format!(
                "{} tracks will be unified to \"{}\"",
                affected, self.canonical_name
            ));

            // Analyze quality for selected variants
            self.analyze_quality_for_selection();
        } else {
            self.status_message = None;
            self.quality_analysis = None;
            self.quality_state = None;
        }

        AlbumArtistClusterAction::Continue
    }

    /// Analyze quality of tracks in selected variants.
    fn analyze_quality_for_selection(&mut self) {
        // Get variant names for selected indices
        let variant_names: Vec<String> = if let Some(bucket) = self.session.current_bucket() {
            self.selected_indices
                .iter()
                .filter_map(|&idx| bucket.variants.get(idx))
                .map(|v| v.name.clone())
                .collect()
        } else {
            return;
        };

        if variant_names.is_empty() {
            return;
        }

        // Fetch tracks from database
        let db_path = match crate::config::get_db_path() {
            Ok(p) => p,
            Err(_) => return,
        };
        let db = match crate::corpus::db::Database::open(&db_path) {
            Ok(db) => db,
            Err(_) => return,
        };

        match db.get_tracks_by_album_artists(&variant_names) {
            Ok(tracks) if !tracks.is_empty() => {
                let analysis = analyze_quality(&tracks);
                if analysis.has_quality_differences {
                    self.quality_state = Some(QualityResolutionState::new(&analysis));
                    if self.quality_state.as_ref().map(|s| s.has_options()).unwrap_or(false) {
                        self.status_message = Some(format!(
                            "{} tracks will be unified. Quality differences detected - Right arrow for options",
                            tracks.len()
                        ));
                    }
                }
                self.quality_analysis = Some(analysis);
            }
            _ => {}
        }
    }

    fn calculate_affected_tracks(&self) -> usize {
        if let Some(bucket) = self.session.current_bucket() {
            self.selected_indices
                .iter()
                .filter_map(|&idx| bucket.variants.get(idx))
                .filter(|v| v.name != self.canonical_name)
                .map(|v| v.track_count)
                .sum()
        } else {
            0
        }
    }

    fn advance_to_next_group(&mut self) -> AlbumArtistClusterAction {
        if self.squash_confirmed {
            self.record_current_decision();
        }

        self.session.advance();

        if self.session.is_complete() {
            return AlbumArtistClusterAction::SessionComplete;
        }

        self.reset_for_current_bucket();
        AlbumArtistClusterAction::Continue
    }

    fn go_to_previous_group(&mut self) -> AlbumArtistClusterAction {
        if self.squash_confirmed {
            self.record_current_decision();
        }

        if self.session.go_back() {
            self.load_decision_for_current_bucket();
            AlbumArtistClusterAction::Continue
        } else {
            AlbumArtistClusterAction::None
        }
    }

    fn record_current_decision(&mut self) {
        let canonical_name = self.canonical_name.clone();

        let variants_to_rename: Vec<String> = if let Some(bucket) = self.session.current_bucket() {
            self.selected_indices
                .iter()
                .filter_map(|&idx| bucket.variants.get(idx))
                .filter(|v| v.name != canonical_name)
                .map(|v| v.name.clone())
                .collect()
        } else {
            vec![]
        };

        let bucket = match self.session.current_bucket().cloned() {
            Some(b) => b,
            None => return,
        };

        let mut pending_changes = self.generate_pending_changes(&canonical_name, &variants_to_rename);

        // Add stash changes for selected quality resolution options
        if let (Some(ref analysis), Some(ref quality_state)) = (&self.quality_analysis, &self.quality_state) {
            let selected_options = quality_state.get_selected_options();
            if !selected_options.is_empty() {
                // Get stash root from config
                if let Ok(config) = crate::config::load_config() {
                    if let Some(ref stash_dir) = config.stash_dir {
                        for option in selected_options {
                            let stash_changes = generate_stash_changes(
                                option,
                                analysis,
                                &self.session.session_id,
                                stash_dir,
                                "album-artist-quality",
                            );
                            pending_changes.extend(stash_changes);
                        }
                    }
                }
            }
        }

        // Remove any existing decision for this bucket
        self.session.decisions.retain(|d| {
            d.bucket.normalized_key != bucket.normalized_key
        });

        let decision = AlbumArtistDecision {
            bucket,
            canonical_name,
            variants_to_rename,
            pending_changes,
        };

        self.session.decisions.push(decision);
    }

    fn generate_pending_changes(
        &self,
        canonical_name: &str,
        variants_to_rename: &[String],
    ) -> Vec<PendingChange> {
        let db_path = match crate::config::get_db_path() {
            Ok(p) => p,
            Err(_) => return vec![],
        };
        let db = match crate::corpus::db::Database::open(&db_path) {
            Ok(db) => db,
            Err(_) => return vec![],
        };

        let session_id = &self.session.session_id;
        let mut changes = Vec::new();

        for variant_name in variants_to_rename {
            if let Ok(track_ids) = db.get_track_ids_by_album_artist(variant_name) {
                for track_id in track_ids {
                    if let Ok(Some(track)) = db.get_track_by_id(track_id) {
                        let metadata_json = serde_json::json!({
                            "old_album_artist": variant_name,
                            "new_album_artist": canonical_name,
                        }).to_string();

                        changes.push(PendingChange {
                            id: None,
                            session_id: session_id.clone(),
                            change_type: ChangeType::TagEdit,
                            source_path: track.path,
                            target_path: None,
                            metadata_changes: Some(metadata_json),
                            created_at: None,
                            status: ChangeStatus::Pending,
                        });
                    }
                }
            }
        }

        changes
    }

    fn load_decision_for_current_bucket(&mut self) {
        let bucket = match self.session.current_bucket() {
            Some(b) => b,
            None => {
                self.reset_for_current_bucket();
                return;
            }
        };

        let existing_decision = self.session.decisions.iter().find(|d| {
            d.bucket.normalized_key == bucket.normalized_key
        });

        if let Some(decision) = existing_decision {
            self.canonical_name = decision.canonical_name.clone();
            self.text_cursor = self.canonical_name.len();
            self.squash_confirmed = true;

            self.selected_indices.clear();
            for (idx, variant) in bucket.variants.iter().enumerate() {
                if decision.variants_to_rename.contains(&variant.name)
                    || variant.name == decision.canonical_name
                {
                    self.selected_indices.insert(idx);
                }
            }

            self.status_message = Some(format!(
                "{} tracks will be unified to \"{}\"",
                decision.affected_track_count(),
                decision.canonical_name
            ));
        } else {
            self.reset_for_current_bucket();
        }

        self.cursor_idx = 0;
        self.list_state.select(Some(0));
        self.focused_pane = Pane::Variants;
        self.action_focus = ActionFocus::NameField;
    }

    fn reset_for_current_bucket(&mut self) {
        self.selected_indices.clear();
        self.cursor_idx = 0;
        self.list_state.select(Some(0));
        self.focused_pane = Pane::Variants;
        self.action_focus = ActionFocus::NameField;
        self.squash_confirmed = false;
        self.status_message = None;
        self.quality_analysis = None;
        self.quality_state = None;

        self.canonical_name = self
            .session
            .current_bucket()
            .and_then(|b| b.variants.first())
            .map(|v| v.name.clone())
            .unwrap_or_default();
        self.text_cursor = self.canonical_name.len();
    }

    /// Render the two-pane cluster view
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(3),
            ])
            .split(area);

        self.render_header(f, chunks[0]);
        self.render_panes(f, chunks[1]);
        self.render_controls(f, chunks[2]);
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let (title, subtitle) = if let Some(bucket) = self.session.current_bucket() {
            let title = format!(
                "Bucket {}/{}: \"{}\"",
                self.session.current_index + 1,
                self.session.buckets.len(),
                bucket.normalized_key
            );
            let subtitle = format!(
                "{} variants | {} total tracks | {} selected",
                bucket.variants.len(),
                bucket.total_tracks(),
                self.selected_indices.len()
            );
            (title, subtitle)
        } else {
            ("No buckets to process".to_string(), String::new())
        };

        let header = Paragraph::new(vec![
            Line::from(Span::styled(
                title,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(subtitle, Style::default().fg(Color::DarkGray))),
        ])
        .block(Block::default().borders(Borders::ALL).title("Album Artist Canonicalization"));

        f.render_widget(header, area);
    }

    fn render_panes(&mut self, f: &mut Frame, area: Rect) {
        let has_quality_options = self.quality_state.as_ref()
            .map(|s| s.has_options())
            .unwrap_or(false);

        if has_quality_options {
            // Three-pane layout when quality options available
            let layout = ThreePaneLayout::horizontal()
                .left(PaneConfig::new("", 45))
                .middle(PaneConfig::new("", 30))
                .right(PaneConfig::new("", 25))
                .build(area);

            self.render_variants_pane(f, layout.left.area);
            self.render_action_pane(f, layout.middle.area);
            self.render_quality_pane(f, layout.right.area);
        } else {
            // Two-pane layout
            let layout = TwoPaneLayout::horizontal()
                .left(PaneConfig::new("", 60))
                .right(PaneConfig::new("", 40))
                .build(area);

            self.render_variants_pane(f, layout.left.area);
            self.render_action_pane(f, layout.right.area);
        }
    }

    fn render_quality_pane(&self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.focused_pane, Pane::QualityOptions);

        let border_style = if is_focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let quality_state = match &self.quality_state {
            Some(qs) => qs,
            None => {
                let empty = Paragraph::new("No quality options")
                    .block(Block::default().borders(Borders::ALL).title("Quality"));
                f.render_widget(empty, area);
                return;
            }
        };

        let items: Vec<ListItem> = quality_state.options.iter().enumerate().map(|(i, option)| {
            let is_selected = quality_state.is_selected(i);
            let is_cursor = i == quality_state.cursor && is_focused;

            let checkbox = if is_selected { "[x]" } else { "[ ]" };
            let cursor_prefix = if is_cursor { ">" } else { " " };

            let style = if is_cursor {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default().fg(Color::Green)
            } else {
                Style::default()
            };

            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} {} ", cursor_prefix, checkbox),
                    if is_selected {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::styled(option.short_label(), style),
            ]))
        }).collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title("Quality Resolution"),
        );

        f.render_widget(list, area);
    }

    fn render_variants_pane(&mut self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.focused_pane, Pane::Variants);

        let bucket = match self.session.current_bucket() {
            Some(b) => b,
            None => {
                let empty = Paragraph::new("No bucket selected")
                    .block(Block::default().borders(Borders::ALL).title("Variants"));
                f.render_widget(empty, area);
                return;
            }
        };

        let max_count = bucket.variants.iter().map(|v| v.track_count).max().unwrap_or(0);
        let count_width = format!("{}", max_count).len();

        let items: Vec<ListItem> = bucket
            .variants
            .iter()
            .enumerate()
            .map(|(i, variant)| {
                let is_selected = self.selected_indices.contains(&i);
                let is_cursor = i == self.cursor_idx && is_focused;

                let checkbox = if is_selected { "[x]" } else { "[ ]" };
                let cursor_prefix = if is_cursor { ">" } else { " " };

                let name_style = if is_cursor {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else if is_selected {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };

                let count_str = format!("[{:>width$}]", variant.track_count, width = count_width);

                Line::from(vec![
                    Span::styled(
                        format!("{} {} ", cursor_prefix, checkbox),
                        if is_selected {
                            Style::default().fg(Color::Green)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        },
                    ),
                    Span::styled(&variant.name, name_style),
                    Span::raw("  "),
                    Span::styled(count_str, Style::default().fg(Color::DarkGray)),
                ])
            })
            .map(ListItem::new)
            .collect();

        let border_style = if is_focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title("Variants (Space: toggle, A: toggle all)"),
        );

        f.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_action_pane(&self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.focused_pane, Pane::Action);

        let border_style = if is_focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title("Action");

        let inner = block.inner(area);
        f.render_widget(block, area);

        let action_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(0),
            ])
            .split(inner);

        self.render_name_field(f, action_chunks[0], is_focused);
        self.render_squash_button(f, action_chunks[1], is_focused);
    }

    fn render_name_field(&self, f: &mut Frame, area: Rect, pane_focused: bool) {
        let is_focused = pane_focused && matches!(self.action_focus, ActionFocus::NameField);

        let border_style = if is_focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let text_style = if is_focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let input_line = if is_focused {
            let before = &self.canonical_name[..self.text_cursor];
            let cursor_char = self.canonical_name.chars().nth(self.text_cursor).unwrap_or(' ');
            let after = if self.text_cursor < self.canonical_name.len() {
                &self.canonical_name[self.text_cursor + 1..]
            } else {
                ""
            };

            Line::from(vec![
                Span::styled("Name: ", Style::default().fg(Color::DarkGray)),
                Span::styled(before, text_style),
                Span::styled(
                    cursor_char.to_string(),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(after, text_style),
            ])
        } else {
            Line::from(vec![
                Span::styled("Name: ", Style::default().fg(Color::DarkGray)),
                Span::styled(&self.canonical_name, text_style),
            ])
        };

        let para = Paragraph::new(input_line)
            .block(Block::default().borders(Borders::ALL).border_style(border_style));
        f.render_widget(para, area);
    }

    fn render_squash_button(&self, f: &mut Frame, area: Rect, pane_focused: bool) {
        let is_focused = pane_focused && matches!(self.action_focus, ActionFocus::SquashButton);

        let checkbox = if self.squash_confirmed { "[x]" } else { "[ ]" };

        let button_style = if is_focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if self.squash_confirmed {
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
        } else if self.selected_indices.len() >= 2 {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let border_style = if is_focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let button_text = Line::from(vec![
            Span::styled(format!("{} Unify!", checkbox), button_style),
        ]);

        let para = Paragraph::new(button_text)
            .block(Block::default().borders(Borders::ALL).border_style(border_style))
            .alignment(ratatui::layout::Alignment::Center);

        f.render_widget(para, area);
    }

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let status_or_controls = if let Some(msg) = &self.status_message {
            Line::from(Span::styled(msg, Style::default().fg(Color::Cyan)))
        } else {
            Line::from(vec![
                Span::styled("Space", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Toggle | "),
                Span::styled("A", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Toggle All | "),
                Span::styled("Tab", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Next | "),
                Span::styled("Shift+Tab", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Back | "),
                Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(": Review"),
            ])
        };

        let para = Paragraph::new(status_or_controls)
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(para, area);
    }
}
