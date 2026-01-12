//! Single-Screen Cluster View (Two-Pane Layout)
//!
//! Presents a single artist name cluster for canonicalization in a horizontal
//! two-pane layout. Left pane shows variants, right pane has canonical name
//! input and Squash! button.
//!
//! ## Panes
//! - **Left (Variants)**: Artist names with checkboxes and track counts
//! - **Right (Action)**: Canonical name input field + "Squash!" button
//!
//! ## Key Bindings
//! - Left/Right: Move focus between panes
//! - Tab: Advance to next group (preserves current decision)
//! - Shift+Tab: Go back to previous group
//! - Esc: Go to session review
//!
//! ## Action Pane Navigation
//! - Up/Down: Move between Name field and Squash button
//! - Space on Squash: Toggle squash state
//! - Enter on Squash: Toggle state and advance to next group

use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use super::session::{CanonDecision, CanonSession};
use crate::db::{ChangeStatus, ChangeType, PendingChange};
use crate::ui::app::flip_coin;

/// Which pane is currently focused
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonPane {
    Variants,
    Action,
}

/// Focus within the Action pane
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionFocus {
    NameField,
    SquashButton,
}

/// Actions returned from the cluster view
#[derive(Debug, Clone)]
pub enum ClusterViewAction {
    /// No action needed
    None,
    /// Continue processing (internal state change)
    Continue,
    /// Session complete, go to review
    SessionComplete,
    /// Show session review (via Esc)
    ShowSessionReview,
    /// Status message to display
    StatusMessage(String),
}

/// State for the single-screen cluster view
#[derive(Debug, Clone)]
pub struct ClusterViewState {
    /// The session containing all buckets
    session: CanonSession,

    // Pane focus
    focused_pane: CanonPane,

    // Variants pane
    selected_indices: HashSet<usize>,
    cursor_idx: usize,
    list_state: ListState,

    // Action pane
    action_focus: ActionFocus,
    canonical_name: String,
    text_cursor: usize,
    squash_confirmed: bool,

    // Status message
    status_message: Option<String>,
}

impl ClusterViewState {
    /// Create a new cluster view state from a session
    pub fn new(session: CanonSession) -> Self {
        let canonical_name = session
            .current_bucket()
            .and_then(|b| b.variants.first())
            .map(|v| v.name.clone())
            .unwrap_or_default();
        let text_cursor = canonical_name.len();

        let mut state = Self {
            session,
            focused_pane: CanonPane::Variants,
            selected_indices: HashSet::new(),
            cursor_idx: 0,
            list_state: ListState::default(),
            action_focus: ActionFocus::NameField,
            canonical_name,
            text_cursor,
            squash_confirmed: false,
            status_message: None,
        };
        state.list_state.select(Some(0));
        state
    }

    /// Get a reference to the session
    pub fn session(&self) -> &CanonSession {
        &self.session
    }

    /// Get mutable reference to the session
    pub fn session_mut(&mut self) -> &mut CanonSession {
        &mut self.session
    }

    /// Take ownership of the session (for transitioning)
    pub fn into_session(self) -> CanonSession {
        self.session
    }

    /// Check if the session is complete
    pub fn is_session_complete(&self) -> bool {
        self.session.is_complete()
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> ClusterViewAction {
        // Global navigation
        match key.code {
            KeyCode::Tab => {
                // Advance to next group (preserves decision if squash confirmed)
                return self.advance_to_next_group();
            }
            KeyCode::BackTab => {
                // Go back to previous group
                return self.go_to_previous_group();
            }
            KeyCode::Esc => {
                return ClusterViewAction::ShowSessionReview;
            }
            KeyCode::Right => {
                if matches!(self.focused_pane, CanonPane::Variants) {
                    self.focused_pane = CanonPane::Action;
                    return ClusterViewAction::Continue;
                } else if matches!(self.action_focus, ActionFocus::NameField) {
                    // Move cursor in text if not at end
                    if self.text_cursor < self.canonical_name.len() {
                        self.text_cursor += 1;
                        return ClusterViewAction::Continue;
                    }
                }
            }
            KeyCode::Left => {
                if matches!(self.focused_pane, CanonPane::Action) {
                    if matches!(self.action_focus, ActionFocus::NameField) && self.text_cursor > 0 {
                        // Move cursor in text
                        self.text_cursor -= 1;
                        return ClusterViewAction::Continue;
                    } else {
                        // Go back to variants pane
                        self.focused_pane = CanonPane::Variants;
                        return ClusterViewAction::Continue;
                    }
                }
            }
            _ => {}
        }

        // Pane-specific handling
        match self.focused_pane {
            CanonPane::Variants => self.handle_variants_key(key),
            CanonPane::Action => self.handle_action_key(key),
        }
    }

    fn handle_variants_key(&mut self, key: KeyEvent) -> ClusterViewAction {
        match key.code {
            KeyCode::Up => {
                self.move_cursor(-1);
                ClusterViewAction::Continue
            }
            KeyCode::Down => {
                self.move_cursor(1);
                ClusterViewAction::Continue
            }
            KeyCode::Char(' ') => {
                self.toggle_selection();
                self.update_canonical_from_selection();
                ClusterViewAction::Continue
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.toggle_all();
                self.update_canonical_from_selection();
                ClusterViewAction::Continue
            }
            KeyCode::Enter => {
                // Move to action pane, squash button
                self.focused_pane = CanonPane::Action;
                self.action_focus = ActionFocus::SquashButton;
                ClusterViewAction::Continue
            }
            _ => ClusterViewAction::None,
        }
    }

    fn handle_action_key(&mut self, key: KeyEvent) -> ClusterViewAction {
        match self.action_focus {
            ActionFocus::NameField => self.handle_name_field_key(key),
            ActionFocus::SquashButton => self.handle_squash_button_key(key),
        }
    }

    fn handle_name_field_key(&mut self, key: KeyEvent) -> ClusterViewAction {
        match key.code {
            KeyCode::Up => {
                // Stay on name field (already at top)
                ClusterViewAction::None
            }
            KeyCode::Down => {
                self.action_focus = ActionFocus::SquashButton;
                ClusterViewAction::Continue
            }
            KeyCode::Backspace => {
                if self.text_cursor > 0 {
                    self.text_cursor -= 1;
                    self.canonical_name.remove(self.text_cursor);
                }
                ClusterViewAction::Continue
            }
            KeyCode::Delete => {
                if self.text_cursor < self.canonical_name.len() {
                    self.canonical_name.remove(self.text_cursor);
                }
                ClusterViewAction::Continue
            }
            KeyCode::Home => {
                self.text_cursor = 0;
                ClusterViewAction::Continue
            }
            KeyCode::End => {
                self.text_cursor = self.canonical_name.len();
                ClusterViewAction::Continue
            }
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    return ClusterViewAction::None;
                }
                self.canonical_name.insert(self.text_cursor, c);
                self.text_cursor += 1;
                ClusterViewAction::Continue
            }
            KeyCode::Enter => {
                // Move to squash button
                self.action_focus = ActionFocus::SquashButton;
                ClusterViewAction::Continue
            }
            _ => ClusterViewAction::None,
        }
    }

    fn handle_squash_button_key(&mut self, key: KeyEvent) -> ClusterViewAction {
        match key.code {
            KeyCode::Up => {
                self.action_focus = ActionFocus::NameField;
                ClusterViewAction::Continue
            }
            KeyCode::Down => {
                // Stay on squash button (already at bottom)
                ClusterViewAction::None
            }
            KeyCode::Char(' ') => {
                // Space: toggle squash state only
                self.toggle_squash_state()
            }
            KeyCode::Enter => {
                // Enter: toggle state and advance to next group
                let toggle_result = self.toggle_squash_state();
                if self.squash_confirmed {
                    // If now confirmed, advance to next group
                    return self.advance_to_next_group();
                }
                toggle_result
            }
            _ => ClusterViewAction::None,
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

            // Tie case: use coin flip
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

    fn toggle_squash_state(&mut self) -> ClusterViewAction {
        // Need at least 2 selections to squash
        if self.selected_indices.len() < 2 {
            self.status_message = Some("Select at least 2 variants to squash".to_string());
            return ClusterViewAction::StatusMessage(
                "Select at least 2 variants to squash".to_string(),
            );
        }

        // Canonical name must not be empty
        if self.canonical_name.trim().is_empty() {
            self.status_message = Some("Canonical name cannot be empty".to_string());
            return ClusterViewAction::StatusMessage("Canonical name cannot be empty".to_string());
        }

        self.squash_confirmed = !self.squash_confirmed;

        if self.squash_confirmed {
            let affected = self.calculate_affected_tracks();
            self.status_message = Some(format!(
                "{} tracks will be squashed to \"{}\"",
                affected, self.canonical_name
            ));
        } else {
            self.status_message = None;
        }

        ClusterViewAction::Continue
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

    /// Advance to next group, preserving current decision if squash is confirmed
    fn advance_to_next_group(&mut self) -> ClusterViewAction {
        // If squash is confirmed, record the decision
        if self.squash_confirmed {
            self.record_current_decision();
        }

        // Advance session
        self.session.advance();

        // Check if session is complete
        if self.session.is_complete() {
            return ClusterViewAction::SessionComplete;
        }

        // Reset for next bucket
        self.reset_for_current_bucket();
        ClusterViewAction::Continue
    }

    /// Go back to previous group, persisting current decision first
    fn go_to_previous_group(&mut self) -> ClusterViewAction {
        // Persist current decision before navigating away
        if self.squash_confirmed {
            self.record_current_decision();
        }

        if self.session.go_back() {
            // Load the decision state if one was made for that bucket
            self.load_decision_for_current_bucket();
            ClusterViewAction::Continue
        } else {
            ClusterViewAction::None
        }
    }

    /// Record the current decision for this bucket
    fn record_current_decision(&mut self) {
        let canonical_name = self.canonical_name.clone();

        // Get variants to rename (those not already matching canonical)
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

        // Get the bucket
        let bucket = match self.session.current_bucket().cloned() {
            Some(b) => b,
            None => return,
        };

        // Generate pending changes
        let pending_changes = self.generate_pending_changes(&canonical_name, &variants_to_rename);

        // Check if we already have a decision for this bucket index
        let bucket_idx = self.session.current_index;

        // Remove any existing decision for this bucket
        self.session.decisions.retain(|d| {
            d.bucket.normalized_key != bucket.normalized_key
        });

        // Create new decision
        let variants_count = variants_to_rename.len();
        let decision = CanonDecision {
            bucket,
            canonical_name,
            variants_to_rename,
            pending_changes,
        };

        self.session.decisions.push(decision);

        let _ = crate::config::log_message(&format!(
            "Recorded decision for bucket {}: {} variants to rename",
            bucket_idx,
            variants_count
        ));
    }

    /// Generate pending changes for the current decision
    fn generate_pending_changes(
        &self,
        canonical_name: &str,
        variants_to_rename: &[String],
    ) -> Vec<PendingChange> {
        let db_path = match crate::config::get_db_path() {
            Ok(p) => p,
            Err(_) => return vec![],
        };
        let db = match crate::db::Database::open(&db_path) {
            Ok(db) => db,
            Err(_) => return vec![],
        };

        let session_id = &self.session.session_id;
        let mut changes = Vec::new();

        for variant_name in variants_to_rename {
            if let Ok(track_ids) = db.get_track_ids_by_artist(variant_name) {
                for track_id in track_ids {
                    if let Ok(Some(track)) = db.get_track_by_id(track_id) {
                        let metadata_json = serde_json::json!({
                            "old_artist": variant_name,
                            "new_artist": canonical_name,
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

    /// Load decision state when going back to a bucket
    fn load_decision_for_current_bucket(&mut self) {
        let bucket = match self.session.current_bucket() {
            Some(b) => b,
            None => {
                self.reset_for_current_bucket();
                return;
            }
        };

        // Find if there's an existing decision for this bucket
        let existing_decision = self.session.decisions.iter().find(|d| {
            d.bucket.normalized_key == bucket.normalized_key
        });

        if let Some(decision) = existing_decision {
            // Restore the decision state
            self.canonical_name = decision.canonical_name.clone();
            self.text_cursor = self.canonical_name.len();
            self.squash_confirmed = true;

            // Restore selected indices based on variants_to_rename + canonical
            self.selected_indices.clear();
            for (idx, variant) in bucket.variants.iter().enumerate() {
                if decision.variants_to_rename.contains(&variant.name)
                    || variant.name == decision.canonical_name
                {
                    self.selected_indices.insert(idx);
                }
            }

            self.status_message = Some(format!(
                "{} tracks will be squashed to \"{}\"",
                decision.affected_track_count(),
                decision.canonical_name
            ));
        } else {
            self.reset_for_current_bucket();
        }

        self.cursor_idx = 0;
        self.list_state.select(Some(0));
        self.focused_pane = CanonPane::Variants;
        self.action_focus = ActionFocus::NameField;
    }

    fn reset_for_current_bucket(&mut self) {
        self.selected_indices.clear();
        self.cursor_idx = 0;
        self.list_state.select(Some(0));
        self.focused_pane = CanonPane::Variants;
        self.action_focus = ActionFocus::NameField;
        self.squash_confirmed = false;
        self.status_message = None;

        // Update canonical name for new bucket
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
                Constraint::Length(3),  // Header
                Constraint::Min(10),    // Two panes
                Constraint::Length(3),  // Status/Controls
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
        .block(Block::default().borders(Borders::ALL).title("Artist Canonicalization"));

        f.render_widget(header, area);
    }

    fn render_panes(&mut self, f: &mut Frame, area: Rect) {
        // Two horizontal panes: 60% variants, 40% action
        let pane_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(60),
                Constraint::Percentage(40),
            ])
            .split(area);

        self.render_variants_pane(f, pane_chunks[0]);
        self.render_action_pane(f, pane_chunks[1]);
    }

    fn render_variants_pane(&mut self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.focused_pane, CanonPane::Variants);

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
        let is_focused = matches!(self.focused_pane, CanonPane::Action);

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

        // Split inner area for name field and squash button
        let action_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Name field
                Constraint::Length(3),  // Squash button
                Constraint::Min(0),     // Spacer
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

        // Build text with cursor
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
            Span::styled(format!("{} Squash!", checkbox), button_style),
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
