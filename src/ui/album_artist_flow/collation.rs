//! Album Artist Collation Flow
//!
//! Sequential decision-based flow for unifying mixed-artist albums to a common album_artist.
//! Presents albums one at a time where tracks have different artist values and album_artist is unset.
//! The user decides what album_artist value to apply (typically "Various Artists" or the dominant artist).

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
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

    pub fn remaining_count(&self) -> usize {
        self.albums.len().saturating_sub(self.current_index)
    }
}

/// Focus area within the decision view
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollationFocus {
    /// Selecting from the artist list
    ArtistList,
    /// Editing the custom value field
    CustomField,
    /// Action buttons
    ActionButtons,
}

/// Which action button is selected
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionButton {
    AcceptSuggested,
    UseSelected,
    UseCustom,
    Skip,
}

impl ActionButton {
    fn all() -> &'static [ActionButton] {
        &[
            ActionButton::AcceptSuggested,
            ActionButton::UseSelected,
            ActionButton::UseCustom,
            ActionButton::Skip,
        ]
    }

    fn label(&self, suggested: Option<&str>, selected_artist: Option<&str>) -> String {
        match self {
            ActionButton::AcceptSuggested => {
                let val = suggested.unwrap_or("Various Artists");
                format!("Accept: \"{}\"", val)
            }
            ActionButton::UseSelected => {
                match selected_artist {
                    Some(a) => format!("Use: \"{}\"", a),
                    None => "Use selected artist".to_string(),
                }
            }
            ActionButton::UseCustom => "Use custom value".to_string(),
            ActionButton::Skip => "Skip this album".to_string(),
        }
    }
}

/// State for the collation view - presents one album at a time
#[derive(Debug, Clone)]
pub struct CollationState {
    session: CollationSession,
    focus: CollationFocus,
    /// Currently selected artist in the list
    artist_cursor: usize,
    artist_list_state: ListState,
    /// Custom value being typed
    custom_value: String,
    text_cursor: usize,
    /// Which action button is highlighted
    action_cursor: usize,
    action_list_state: ListState,
}

impl CollationState {
    pub fn new(session: CollationSession) -> Self {
        let mut state = Self {
            session,
            focus: CollationFocus::ActionButtons,
            artist_cursor: 0,
            artist_list_state: ListState::default(),
            custom_value: String::new(),
            text_cursor: 0,
            action_cursor: 0,
            action_list_state: ListState::default(),
        };
        state.artist_list_state.select(Some(0));
        state.action_list_state.select(Some(0));
        state.reset_for_current_album();
        state
    }

    pub fn into_session(self) -> CollationSession {
        self.session
    }

    fn reset_for_current_album(&mut self) {
        self.artist_cursor = 0;
        self.artist_list_state.select(Some(0));
        self.custom_value.clear();
        self.text_cursor = 0;
        self.action_cursor = 0;
        self.action_list_state.select(Some(0));
        self.focus = CollationFocus::ActionButtons;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> CollationAction {
        match key.code {
            KeyCode::Esc => return CollationAction::ShowReview,
            KeyCode::Tab => {
                // Cycle focus: ActionButtons -> ArtistList -> CustomField -> ActionButtons
                self.focus = match self.focus {
                    CollationFocus::ActionButtons => CollationFocus::ArtistList,
                    CollationFocus::ArtistList => CollationFocus::CustomField,
                    CollationFocus::CustomField => CollationFocus::ActionButtons,
                };
                return CollationAction::Continue;
            }
            KeyCode::BackTab => {
                // Reverse cycle
                self.focus = match self.focus {
                    CollationFocus::ActionButtons => CollationFocus::CustomField,
                    CollationFocus::ArtistList => CollationFocus::ActionButtons,
                    CollationFocus::CustomField => CollationFocus::ArtistList,
                };
                return CollationAction::Continue;
            }
            _ => {}
        }

        match self.focus {
            CollationFocus::ActionButtons => self.handle_action_key(key),
            CollationFocus::ArtistList => self.handle_artist_key(key),
            CollationFocus::CustomField => self.handle_custom_key(key),
        }
    }

    fn handle_action_key(&mut self, key: KeyEvent) -> CollationAction {
        match key.code {
            KeyCode::Up => {
                if self.action_cursor > 0 {
                    self.action_cursor -= 1;
                    self.action_list_state.select(Some(self.action_cursor));
                }
                CollationAction::Continue
            }
            KeyCode::Down => {
                let max = ActionButton::all().len().saturating_sub(1);
                if self.action_cursor < max {
                    self.action_cursor += 1;
                    self.action_list_state.select(Some(self.action_cursor));
                }
                CollationAction::Continue
            }
            KeyCode::Enter => self.execute_action(),
            KeyCode::Left => {
                self.focus = CollationFocus::ArtistList;
                CollationAction::Continue
            }
            _ => CollationAction::None,
        }
    }

    fn handle_artist_key(&mut self, key: KeyEvent) -> CollationAction {
        let artist_count = self.session.current_album()
            .map(|a| a.artists.len())
            .unwrap_or(0);

        match key.code {
            KeyCode::Up => {
                if self.artist_cursor > 0 {
                    self.artist_cursor -= 1;
                    self.artist_list_state.select(Some(self.artist_cursor));
                }
                CollationAction::Continue
            }
            KeyCode::Down => {
                if self.artist_cursor + 1 < artist_count {
                    self.artist_cursor += 1;
                    self.artist_list_state.select(Some(self.artist_cursor));
                }
                CollationAction::Continue
            }
            KeyCode::Enter => {
                // Use selected artist - set action to UseSelected and execute
                self.action_cursor = 1; // UseSelected
                self.action_list_state.select(Some(1));
                self.execute_action()
            }
            KeyCode::Right => {
                self.focus = CollationFocus::ActionButtons;
                CollationAction::Continue
            }
            _ => CollationAction::None,
        }
    }

    fn handle_custom_key(&mut self, key: KeyEvent) -> CollationAction {
        match key.code {
            KeyCode::Char(c) => {
                self.custom_value.insert(self.text_cursor, c);
                self.text_cursor += 1;
                CollationAction::Continue
            }
            KeyCode::Backspace => {
                if self.text_cursor > 0 {
                    self.text_cursor -= 1;
                    self.custom_value.remove(self.text_cursor);
                }
                CollationAction::Continue
            }
            KeyCode::Delete => {
                if self.text_cursor < self.custom_value.len() {
                    self.custom_value.remove(self.text_cursor);
                }
                CollationAction::Continue
            }
            KeyCode::Left => {
                if self.text_cursor > 0 {
                    self.text_cursor -= 1;
                } else {
                    self.focus = CollationFocus::ArtistList;
                }
                CollationAction::Continue
            }
            KeyCode::Right => {
                if self.text_cursor < self.custom_value.len() {
                    self.text_cursor += 1;
                } else {
                    self.focus = CollationFocus::ActionButtons;
                }
                CollationAction::Continue
            }
            KeyCode::Enter => {
                // Use custom value if not empty
                if !self.custom_value.trim().is_empty() {
                    self.action_cursor = 2; // UseCustom
                    self.action_list_state.select(Some(2));
                    self.execute_action()
                } else {
                    CollationAction::Continue
                }
            }
            KeyCode::Up | KeyCode::Down => {
                self.focus = CollationFocus::ActionButtons;
                CollationAction::Continue
            }
            _ => CollationAction::None,
        }
    }

    fn execute_action(&mut self) -> CollationAction {
        let action = ActionButton::all()
            .get(self.action_cursor)
            .copied()
            .unwrap_or(ActionButton::Skip);

        match action {
            ActionButton::AcceptSuggested => {
                if let Some(album) = self.session.current_album() {
                    let value = album.suggested_album_artist
                        .clone()
                        .unwrap_or_else(|| "Various Artists".to_string());
                    self.record_decision(value);
                }
                self.advance_to_next()
            }
            ActionButton::UseSelected => {
                if let Some(album) = self.session.current_album() {
                    if let Some((artist, _)) = album.artists.get(self.artist_cursor) {
                        self.record_decision(artist.clone());
                    }
                }
                self.advance_to_next()
            }
            ActionButton::UseCustom => {
                if !self.custom_value.trim().is_empty() {
                    self.record_decision(self.custom_value.trim().to_string());
                    self.advance_to_next()
                } else {
                    // Focus on custom field if empty
                    self.focus = CollationFocus::CustomField;
                    CollationAction::Continue
                }
            }
            ActionButton::Skip => {
                self.advance_to_next()
            }
        }
    }

    fn record_decision(&mut self, chosen_album_artist: String) {
        let album = match self.session.current_album() {
            Some(a) => a.clone(),
            None => return,
        };

        let pending_changes = vec![PendingChange {
            id: None,
            session_id: self.session.session_id.clone(),
            change_type: ChangeType::TagEdit,
            source_path: format!("[album:{}]", album.album_name),
            target_path: None,
            metadata_changes: Some(
                serde_json::json!({
                    "operation": "collation",
                    "album": album.album_name,
                    "new_album_artist": chosen_album_artist,
                    "artist_count": album.artists.len(),
                    "total_tracks": album.total_tracks,
                })
                .to_string(),
            ),
            created_at: None,
            status: ChangeStatus::Pending,
        }];

        self.session.decisions.push(CollationDecision {
            album_name: album.album_name,
            chosen_album_artist,
            pending_changes,
        });
    }

    fn advance_to_next(&mut self) -> CollationAction {
        self.session.advance();
        if self.session.is_complete() {
            CollationAction::SessionComplete
        } else {
            self.reset_for_current_album();
            CollationAction::Continue
        }
    }

    /// Render the collation decision view
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if self.session.is_complete() || self.session.current_album().is_none() {
            let message = Paragraph::new("All albums have been reviewed.\nPress ESC to see review.")
                .block(Block::default().borders(Borders::ALL).title("Complete"));
            frame.render_widget(message, area);
            return;
        }

        // Two-column layout: Info (left) | Actions (right)
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(60),
                Constraint::Percentage(40),
            ])
            .split(area);

        self.render_album_info(frame, main_chunks[0]);
        self.render_actions(frame, main_chunks[1]);
    }

    fn render_album_info(&mut self, frame: &mut Frame, area: Rect) {
        let album = match self.session.current_album() {
            Some(a) => a,
            None => return,
        };

        // Vertical split: Header | Artists list
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8), // Header with album info
                Constraint::Min(6),    // Artists list
            ])
            .split(area);

        // Header: Album info
        let progress = format!(
            "Album {}/{} ({} remaining)",
            self.session.current_index + 1,
            self.session.albums.len(),
            self.session.remaining_count().saturating_sub(1)
        );

        let existing_str = album.existing_album_artist
            .as_deref()
            .unwrap_or("[not set]");

        let suggested_str = album.suggested_album_artist
            .as_deref()
            .unwrap_or("Various Artists");

        let header_lines = vec![
            Line::from(Span::styled(progress, Style::default().fg(Color::Cyan))),
            Line::from(""),
            Line::from(vec![
                Span::styled("Album: ", Style::default().fg(Color::DarkGray)),
                Span::styled(&album.album_name, Style::default().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::styled("Tracks: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{}", album.total_tracks)),
                Span::styled("  Artists: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{}", album.artists.len())),
            ]),
            Line::from(vec![
                Span::styled("Current: ", Style::default().fg(Color::DarkGray)),
                Span::raw(existing_str),
            ]),
            Line::from(vec![
                Span::styled("Suggested: ", Style::default().fg(Color::DarkGray)),
                Span::styled(suggested_str, Style::default().fg(Color::Green)),
            ]),
        ];

        let header = Paragraph::new(header_lines)
            .block(Block::default().borders(Borders::ALL).title(" Album Artist Collation "));
        frame.render_widget(header, chunks[0]);

        // Artists list
        let focused = matches!(self.focus, CollationFocus::ArtistList);
        let items: Vec<ListItem> = album.artists
            .iter()
            .enumerate()
            .map(|(idx, (artist, count))| {
                let is_selected = idx == self.artist_cursor;
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
                ListItem::new(format!("{} ({} tracks)", artist, count)).style(style)
            })
            .collect();

        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let artist_list = List::new(items)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(" Artists (Tab to focus, Enter to use) "));

        frame.render_stateful_widget(artist_list, chunks[1], &mut self.artist_list_state);
    }

    fn render_actions(&mut self, frame: &mut Frame, area: Rect) {
        let album = self.session.current_album();

        // Vertical split: Custom field | Action buttons
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4), // Custom input
                Constraint::Min(8),    // Action buttons
            ])
            .split(area);

        // Custom value input
        let custom_focused = matches!(self.focus, CollationFocus::CustomField);
        let display_text = if custom_focused {
            let before = &self.custom_value[..self.text_cursor];
            let after = &self.custom_value[self.text_cursor..];
            format!("{}|{}", before, after)
        } else {
            self.custom_value.clone()
        };

        let input_style = if custom_focused {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let custom_input = Paragraph::new(display_text)
            .style(input_style)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(if custom_focused {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                })
                .title(" Custom Value "));
        frame.render_widget(custom_input, chunks[0]);

        // Action buttons
        let actions_focused = matches!(self.focus, CollationFocus::ActionButtons);
        let suggested = album.and_then(|a| a.suggested_album_artist.as_deref());
        let selected_artist = album.and_then(|a| {
            a.artists.get(self.artist_cursor).map(|(name, _)| name.as_str())
        });

        let items: Vec<ListItem> = ActionButton::all()
            .iter()
            .enumerate()
            .map(|(idx, action)| {
                let is_selected = idx == self.action_cursor;
                let style = if is_selected && actions_focused {
                    Style::default()
                        .bg(Color::Blue)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else if is_selected {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };
                ListItem::new(action.label(suggested, selected_artist)).style(style)
            })
            .collect();

        let border_style = if actions_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let action_list = List::new(items)
            .block(Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(" Actions (Enter to select) "));

        frame.render_stateful_widget(action_list, chunks[1], &mut self.action_list_state);
    }

    pub fn get_status_message(&self) -> Option<&str> {
        None
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
            let empty = Paragraph::new("No decisions made. Press Shift+Tab to go back.")
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
