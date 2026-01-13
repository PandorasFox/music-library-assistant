//! Album Artist Collation Flow
//!
//! UI for unifying mixed-artist albums to a common album_artist (typically "Various Artists").
//! Presents albums where tracks have different artist values and suggests a unified album_artist.


use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Frame,
};

use crate::corpus::db::types::AlbumArtistCollation;
use crate::corpus::db::{ChangeStatus, ChangeType, PendingChange};

/// Actions returned from the collation view
#[derive(Debug, Clone)]
pub enum CollationAction {
    None,
    Continue,
    SessionComplete,
    ShowReview,
    StatusMessage(String),
    Cancel,
}

/// A decision made about an album's album_artist
#[derive(Debug, Clone)]
pub struct CollationDecision {
    /// The album this decision applies to
    pub album_name: String,
    /// The album_artist value chosen
    pub chosen_album_artist: String,
    /// Generated pending changes
    pub pending_changes: Vec<PendingChange>,
}

/// Session state for the collation workflow
#[derive(Debug, Clone)]
pub struct CollationSession {
    pub session_id: String,
    pub albums: Vec<AlbumArtistCollation>,
    pub current_index: usize,
    pub decisions: Vec<CollationDecision>,
}

impl CollationSession {
    pub fn new(session_id: String, albums: Vec<AlbumArtistCollation>) -> Self {
        Self {
            session_id,
            albums,
            current_index: 0,
            decisions: Vec::new(),
        }
    }

    pub fn current_album(&self) -> Option<&AlbumArtistCollation> {
        self.albums.get(self.current_index)
    }

    pub fn is_complete(&self) -> bool {
        self.current_index >= self.albums.len()
    }

    pub fn advance(&mut self) {
        self.current_index += 1;
    }

    pub fn go_back(&mut self) -> bool {
        if self.current_index > 0 {
            self.current_index -= 1;
            true
        } else {
            false
        }
    }
}

/// Which pane is focused
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollationPane {
    Albums,
    Artists,
    Action,
}

/// Focus within the Action pane
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollationActionFocus {
    NameField,
    ConfirmButton,
}

/// State for the collation view
#[derive(Debug, Clone)]
pub struct CollationState {
    session: CollationSession,
    focused_pane: CollationPane,
    albums_cursor: usize,
    albums_list_state: ListState,
    artists_cursor: usize,
    artists_list_state: ListState,
    action_focus: CollationActionFocus,
    album_artist_value: String,
    text_cursor: usize,
    status_message: Option<String>,
    confirmed: bool,
}

impl CollationState {
    pub fn new(session: CollationSession) -> Self {
        let album_artist_value = session
            .current_album()
            .and_then(|a| a.suggested_album_artist.clone())
            .unwrap_or_else(|| "Various Artists".to_string());
        let text_cursor = album_artist_value.len();

        let mut state = Self {
            session,
            focused_pane: CollationPane::Albums,
            albums_cursor: 0,
            albums_list_state: ListState::default(),
            artists_cursor: 0,
            artists_list_state: ListState::default(),
            action_focus: CollationActionFocus::NameField,
            album_artist_value,
            text_cursor,
            status_message: None,
            confirmed: false,
        };
        state.albums_list_state.select(Some(0));
        state.artists_list_state.select(Some(0));
        state
    }

    pub fn into_session(self) -> CollationSession {
        self.session
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> CollationAction {
        match key.code {
            KeyCode::Tab => return self.advance_to_next_album(),
            KeyCode::BackTab => return self.go_to_previous_album(),
            KeyCode::Esc => return CollationAction::ShowReview,
            KeyCode::Right => {
                match self.focused_pane {
                    CollationPane::Albums => {
                        self.focused_pane = CollationPane::Artists;
                        return CollationAction::Continue;
                    }
                    CollationPane::Artists => {
                        self.focused_pane = CollationPane::Action;
                        return CollationAction::Continue;
                    }
                    CollationPane::Action => {
                        if matches!(self.action_focus, CollationActionFocus::NameField) {
                            if self.text_cursor < self.album_artist_value.len() {
                                self.text_cursor += 1;
                                return CollationAction::Continue;
                            }
                        }
                    }
                }
            }
            KeyCode::Left => {
                match self.focused_pane {
                    CollationPane::Action => {
                        if matches!(self.action_focus, CollationActionFocus::NameField)
                            && self.text_cursor > 0
                        {
                            self.text_cursor -= 1;
                            return CollationAction::Continue;
                        } else {
                            self.focused_pane = CollationPane::Artists;
                            return CollationAction::Continue;
                        }
                    }
                    CollationPane::Artists => {
                        self.focused_pane = CollationPane::Albums;
                        return CollationAction::Continue;
                    }
                    CollationPane::Albums => {}
                }
            }
            _ => {}
        }

        match self.focused_pane {
            CollationPane::Albums => self.handle_albums_key(key),
            CollationPane::Artists => self.handle_artists_key(key),
            CollationPane::Action => self.handle_action_key(key),
        }
    }

    fn handle_albums_key(&mut self, key: KeyEvent) -> CollationAction {
        match key.code {
            KeyCode::Up => {
                if self.albums_cursor > 0 {
                    self.albums_cursor -= 1;
                    self.albums_list_state.select(Some(self.albums_cursor));
                    self.update_for_current_album();
                }
                CollationAction::Continue
            }
            KeyCode::Down => {
                if self.albums_cursor + 1 < self.session.albums.len() {
                    self.albums_cursor += 1;
                    self.albums_list_state.select(Some(self.albums_cursor));
                    self.update_for_current_album();
                }
                CollationAction::Continue
            }
            KeyCode::Enter => {
                // Move focus to action pane
                self.focused_pane = CollationPane::Action;
                CollationAction::Continue
            }
            _ => CollationAction::None,
        }
    }

    fn handle_artists_key(&mut self, key: KeyEvent) -> CollationAction {
        let current_album = match self.session.albums.get(self.albums_cursor) {
            Some(a) => a,
            None => return CollationAction::None,
        };

        match key.code {
            KeyCode::Up => {
                if self.artists_cursor > 0 {
                    self.artists_cursor -= 1;
                    self.artists_list_state.select(Some(self.artists_cursor));
                }
                CollationAction::Continue
            }
            KeyCode::Down => {
                if self.artists_cursor + 1 < current_album.artists.len() {
                    self.artists_cursor += 1;
                    self.artists_list_state.select(Some(self.artists_cursor));
                }
                CollationAction::Continue
            }
            KeyCode::Enter => {
                // Use this artist as the album_artist
                if let Some((artist_name, _)) = current_album.artists.get(self.artists_cursor) {
                    self.album_artist_value = artist_name.clone();
                    self.text_cursor = self.album_artist_value.len();
                }
                CollationAction::Continue
            }
            _ => CollationAction::None,
        }
    }

    fn handle_action_key(&mut self, key: KeyEvent) -> CollationAction {
        match key.code {
            KeyCode::Up | KeyCode::Down => {
                self.action_focus = match self.action_focus {
                    CollationActionFocus::NameField => CollationActionFocus::ConfirmButton,
                    CollationActionFocus::ConfirmButton => CollationActionFocus::NameField,
                };
                CollationAction::Continue
            }
            KeyCode::Enter => {
                match self.action_focus {
                    CollationActionFocus::NameField => {
                        self.action_focus = CollationActionFocus::ConfirmButton;
                    }
                    CollationActionFocus::ConfirmButton => {
                        return self.confirm_and_record_decision();
                    }
                }
                CollationAction::Continue
            }
            KeyCode::Char(c) => {
                if matches!(self.action_focus, CollationActionFocus::NameField) {
                    self.album_artist_value.insert(self.text_cursor, c);
                    self.text_cursor += 1;
                }
                CollationAction::Continue
            }
            KeyCode::Backspace => {
                if matches!(self.action_focus, CollationActionFocus::NameField)
                    && self.text_cursor > 0
                {
                    self.text_cursor -= 1;
                    self.album_artist_value.remove(self.text_cursor);
                }
                CollationAction::Continue
            }
            KeyCode::Delete => {
                if matches!(self.action_focus, CollationActionFocus::NameField)
                    && self.text_cursor < self.album_artist_value.len()
                {
                    self.album_artist_value.remove(self.text_cursor);
                }
                CollationAction::Continue
            }
            _ => CollationAction::None,
        }
    }

    fn update_for_current_album(&mut self) {
        self.artists_cursor = 0;
        self.artists_list_state.select(Some(0));
        self.confirmed = false;

        if let Some(album) = self.session.albums.get(self.albums_cursor) {
            self.album_artist_value = album
                .suggested_album_artist
                .clone()
                .unwrap_or_else(|| "Various Artists".to_string());
            self.text_cursor = self.album_artist_value.len();
        }
    }

    fn confirm_and_record_decision(&mut self) -> CollationAction {
        let current_album = match self.session.albums.get(self.albums_cursor) {
            Some(a) => a,
            None => return CollationAction::None,
        };

        // Generate pending changes for setting album_artist on all tracks
        let mut pending_changes = Vec::new();

        // We'll create a TagEdit change for each track that needs updating
        // For now, we track the album + chosen album_artist
        // The actual track_ids will be resolved at commit time
        pending_changes.push(PendingChange {
            id: None,
            session_id: self.session.session_id.clone(),
            change_type: ChangeType::TagEdit,
            source_path: format!("[album:{}]", current_album.album_name),
            target_path: None,
            metadata_changes: Some(
                serde_json::json!({
                    "operation": "collation",
                    "album": current_album.album_name,
                    "new_album_artist": self.album_artist_value,
                    "artist_count": current_album.artists.len(),
                    "total_tracks": current_album.total_tracks,
                })
                .to_string(),
            ),
            created_at: None,
            status: ChangeStatus::Pending,
        });

        let decision = CollationDecision {
            album_name: current_album.album_name.clone(),
            chosen_album_artist: self.album_artist_value.clone(),
            pending_changes,
        };

        self.session.decisions.push(decision);
        self.confirmed = true;

        let msg = format!(
            "Set album_artist='{}' for '{}'",
            self.album_artist_value, current_album.album_name
        );
        self.status_message = Some(msg.clone());

        CollationAction::StatusMessage(msg)
    }

    fn advance_to_next_album(&mut self) -> CollationAction {
        if self.albums_cursor + 1 < self.session.albums.len() {
            self.albums_cursor += 1;
            self.albums_list_state.select(Some(self.albums_cursor));
            self.update_for_current_album();
            CollationAction::Continue
        } else {
            CollationAction::SessionComplete
        }
    }

    fn go_to_previous_album(&mut self) -> CollationAction {
        if self.albums_cursor > 0 {
            self.albums_cursor -= 1;
            self.albums_list_state.select(Some(self.albums_cursor));
            self.update_for_current_album();
        }
        CollationAction::Continue
    }

    /// Render the collation view
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Three-pane layout: Albums | Artists | Action
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(35),
                Constraint::Percentage(35),
                Constraint::Percentage(30),
            ])
            .split(area);

        self.render_albums_pane(frame, chunks[0]);
        self.render_artists_pane(frame, chunks[1]);
        self.render_action_pane(frame, chunks[2]);
    }

    fn render_albums_pane(&mut self, frame: &mut Frame, area: Rect) {
        let focused = matches!(self.focused_pane, CollationPane::Albums);

        let title = format!(
            " Albums ({}/{}) ",
            self.albums_cursor + 1,
            self.session.albums.len()
        );

        let items: Vec<ListItem> = self
            .session
            .albums
            .iter()
            .enumerate()
            .map(|(idx, album)| {
                let is_selected = idx == self.albums_cursor;
                let has_decision = self
                    .session
                    .decisions
                    .iter()
                    .any(|d| d.album_name == album.album_name);

                let prefix = if has_decision { "[x] " } else { "[ ] " };
                let content = format!(
                    "{}{} ({} artists, {} tracks)",
                    prefix,
                    album.album_name,
                    album.artists.len(),
                    album.total_tracks
                );

                let style = if is_selected && focused {
                    Style::default()
                        .bg(Color::Blue)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else if is_selected {
                    Style::default().bg(Color::DarkGray)
                } else if has_decision {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                };

                ListItem::new(content).style(style)
            })
            .collect();

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            });

        let list = List::new(items).block(block);

        frame.render_stateful_widget(list, area, &mut self.albums_list_state);
    }

    fn render_artists_pane(&mut self, frame: &mut Frame, area: Rect) {
        let focused = matches!(self.focused_pane, CollationPane::Artists);

        let current_album = self.session.albums.get(self.albums_cursor);

        let items: Vec<ListItem> = if let Some(album) = current_album {
            album
                .artists
                .iter()
                .enumerate()
                .map(|(idx, (artist, count))| {
                    let is_selected = idx == self.artists_cursor;

                    let content = format!("{} ({} tracks)", artist, count);

                    let style = if is_selected && focused {
                        Style::default()
                            .bg(Color::Blue)
                            .fg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else if is_selected {
                        Style::default().bg(Color::DarkGray)
                    } else {
                        Style::default()
                    };

                    ListItem::new(content).style(style)
                })
                .collect()
        } else {
            vec![]
        };

        let title = format!(" Artists on Album ");

        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            });

        let list = List::new(items).block(block);

        frame.render_stateful_widget(list, area, &mut self.artists_list_state);
    }

    fn render_action_pane(&self, frame: &mut Frame, area: Rect) {
        let focused = matches!(self.focused_pane, CollationPane::Action);

        let block = Block::default()
            .title(" Set Album Artist ")
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            });

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Label
                Constraint::Length(3), // Input field
                Constraint::Length(1), // Spacer
                Constraint::Length(3), // Confirm button
                Constraint::Length(1), // Spacer
                Constraint::Min(0),    // Status/info
            ])
            .split(inner);

        // Label
        let label = Paragraph::new("Album Artist:");
        frame.render_widget(label, chunks[0]);

        // Text input
        let input_style = if focused
            && matches!(self.action_focus, CollationActionFocus::NameField)
        {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let display_text = if focused
            && matches!(self.action_focus, CollationActionFocus::NameField)
        {
            let before = &self.album_artist_value[..self.text_cursor];
            let cursor = "|";
            let after = &self.album_artist_value[self.text_cursor..];
            format!("{}{}{}", before, cursor, after)
        } else {
            self.album_artist_value.clone()
        };

        let input_block = Block::default().borders(Borders::ALL).style(input_style);
        let input = Paragraph::new(display_text).block(input_block);
        frame.render_widget(input, chunks[1]);

        // Confirm button
        let button_focused =
            focused && matches!(self.action_focus, CollationActionFocus::ConfirmButton);
        let button_style = if button_focused {
            Style::default()
                .bg(Color::Green)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else if self.confirmed {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let button_text = if self.confirmed {
            "[ Confirmed ]"
        } else {
            "[ Confirm ]"
        };

        let button = Paragraph::new(button_text)
            .style(button_style)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(button, chunks[3]);

        // Status/info
        if let Some(album) = self.session.albums.get(self.albums_cursor) {
            let info_lines = vec![
                Line::from(vec![
                    Span::styled("Existing: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(
                        album
                            .existing_album_artist
                            .as_deref()
                            .unwrap_or("[none]"),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Suggested: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(
                        album
                            .suggested_album_artist
                            .as_deref()
                            .unwrap_or("Various Artists"),
                    ),
                ]),
            ];

            let info = Paragraph::new(info_lines).wrap(Wrap { trim: true });
            frame.render_widget(info, chunks[5]);
        }
    }

    pub fn get_status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
    }
}

// ============================================================================
// Collation Review State
// ============================================================================

/// Which element is focused in the review screen
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollationReviewFocus {
    DecisionList,
    CommitButton,
    CancelButton,
}

/// Actions returned from the collation review screen
#[derive(Debug, Clone)]
pub enum CollationReviewAction {
    None,
    Continue,
    Commit,
    Cancel,
    BackToCollation,
}

/// State for the collation session review screen
#[derive(Debug, Clone)]
pub struct CollationReviewState {
    session: CollationSession,
    focus: CollationReviewFocus,
    list_state: ListState,
    selected_idx: usize,
}

impl CollationReviewState {
    /// Create a new review state from a session
    pub fn new(session: CollationSession) -> Self {
        let mut state = Self {
            session,
            focus: CollationReviewFocus::DecisionList,
            list_state: ListState::default(),
            selected_idx: 0,
        };
        if !state.session.decisions.is_empty() {
            state.list_state.select(Some(0));
        }
        state
    }

    /// Get a reference to the session
    pub fn session(&self) -> &CollationSession {
        &self.session
    }

    /// Take ownership of the session
    pub fn into_session(self) -> CollationSession {
        self.session
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> CollationReviewAction {
        match key.code {
            KeyCode::Up => {
                match self.focus {
                    CollationReviewFocus::DecisionList => {
                        self.move_selection(-1);
                    }
                    CollationReviewFocus::CommitButton => {
                        self.focus = CollationReviewFocus::DecisionList;
                    }
                    CollationReviewFocus::CancelButton => {
                        self.focus = CollationReviewFocus::CommitButton;
                    }
                }
                CollationReviewAction::Continue
            }
            KeyCode::Down => {
                match self.focus {
                    CollationReviewFocus::DecisionList => {
                        if self.selected_idx >= self.session.decisions.len().saturating_sub(1) {
                            self.focus = CollationReviewFocus::CommitButton;
                        } else {
                            self.move_selection(1);
                        }
                    }
                    CollationReviewFocus::CommitButton => {
                        self.focus = CollationReviewFocus::CancelButton;
                    }
                    CollationReviewFocus::CancelButton => {}
                }
                CollationReviewAction::Continue
            }
            KeyCode::Left => {
                if matches!(self.focus, CollationReviewFocus::CancelButton) {
                    self.focus = CollationReviewFocus::CommitButton;
                }
                CollationReviewAction::Continue
            }
            KeyCode::Right => {
                if matches!(self.focus, CollationReviewFocus::CommitButton) {
                    self.focus = CollationReviewFocus::CancelButton;
                }
                CollationReviewAction::Continue
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    CollationReviewFocus::DecisionList => CollationReviewFocus::CommitButton,
                    CollationReviewFocus::CommitButton => CollationReviewFocus::CancelButton,
                    CollationReviewFocus::CancelButton => CollationReviewFocus::DecisionList,
                };
                CollationReviewAction::Continue
            }
            KeyCode::Enter | KeyCode::Char(' ') => match self.focus {
                CollationReviewFocus::DecisionList => CollationReviewAction::None,
                CollationReviewFocus::CommitButton => CollationReviewAction::Commit,
                CollationReviewFocus::CancelButton => CollationReviewAction::Cancel,
            },
            KeyCode::Esc => CollationReviewAction::Cancel,
            KeyCode::BackTab => CollationReviewAction::BackToCollation,
            _ => CollationReviewAction::None,
        }
    }

    fn move_selection(&mut self, delta: i32) {
        let max = self.session.decisions.len();
        if max == 0 {
            return;
        }
        let new_idx = if delta < 0 {
            self.selected_idx.saturating_sub((-delta) as usize)
        } else {
            (self.selected_idx + delta as usize).min(max - 1)
        };
        self.selected_idx = new_idx;
        self.list_state.select(Some(new_idx));
    }

    /// Render the review screen
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(8),
                Constraint::Length(6),
            ])
            .split(area);

        self.render_header(f, chunks[0]);
        self.render_decisions(f, chunks[1]);
        self.render_footer(f, chunks[2]);
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let title = format!("Collation Review - {} Decisions", self.session.decisions.len());

        let header = Paragraph::new(Line::from(Span::styled(
            title,
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Album Artist Collation"),
        );

        f.render_widget(header, area);
    }

    fn render_decisions(&mut self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.focus, CollationReviewFocus::DecisionList);

        if self.session.decisions.is_empty() {
            let empty = Paragraph::new("No decisions made yet. Press Shift+Tab to go back.")
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::ALL).title("Decisions"));
            f.render_widget(empty, area);
            return;
        }

        let items: Vec<ListItem> = self
            .session
            .decisions
            .iter()
            .enumerate()
            .map(|(i, decision)| {
                let is_selected = i == self.selected_idx && is_focused;
                let prefix = if is_selected { ">" } else { " " };

                let line1 = Line::from(vec![
                    Span::styled(
                        format!("{} {}. ", prefix, i + 1),
                        if is_selected {
                            Style::default().fg(Color::Yellow)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        },
                    ),
                    Span::styled(
                        format!("\"{}\"", decision.album_name),
                        Style::default().fg(Color::White),
                    ),
                    Span::styled(" -> ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("\"{}\"", decision.chosen_album_artist),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]);

                let change_count = decision.pending_changes.len();
                let line2 = Line::from(vec![
                    Span::raw("   "),
                    Span::styled(
                        format!("{} change(s) pending", change_count),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);

                ListItem::new(vec![line1, line2])
            })
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
                .title("Decisions (Up/Down to scroll)"),
        );

        f.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_footer(&self, f: &mut Frame, area: Rect) {
        let footer_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(area);

        self.render_summary(f, footer_chunks[0]);
        self.render_buttons(f, footer_chunks[1]);
    }

    fn render_summary(&self, f: &mut Frame, area: Rect) {
        let total_decisions = self.session.decisions.len();
        let total_changes: usize = self
            .session
            .decisions
            .iter()
            .map(|d| d.pending_changes.len())
            .sum();

        let lines = vec![
            Line::from(vec![
                Span::styled("Albums processed: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    total_decisions.to_string(),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("Pending changes: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    total_changes.to_string(),
                    Style::default().fg(Color::Yellow),
                ),
            ]),
        ];

        let summary =
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Summary"));

        f.render_widget(summary, area);
    }

    fn render_buttons(&self, f: &mut Frame, area: Rect) {
        use ratatui::layout::Alignment;

        let commit_focused = matches!(self.focus, CollationReviewFocus::CommitButton);
        let cancel_focused = matches!(self.focus, CollationReviewFocus::CancelButton);

        let commit_style = if commit_focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };

        let cancel_style = if cancel_focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Red)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Red)
        };

        let buttons = Paragraph::new(vec![
            Line::from(""),
            Line::from(vec![
                Span::styled(" [Commit] ", commit_style),
                Span::raw("  "),
                Span::styled(" [Cancel] ", cancel_style),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Tab: Navigate | Enter: Select",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .block(Block::default().borders(Borders::ALL))
        .alignment(Alignment::Center);

        f.render_widget(buttons, area);
    }
}
