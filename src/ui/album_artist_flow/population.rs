//! Album Artist Population Flow
//!
//! Sequential decision-based flow for bulk-filling missing album_artist tags.
//! Presents groups (albums or directories) one at a time and lets the user
//! decide what album_artist value to apply to each group.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::corpus::db::types::AlbumArtistPopulationGroup;
use crate::corpus::db::{ChangeStatus, ChangeType, PendingChange};

/// Actions returned from the population view
#[derive(Debug, Clone)]
pub enum PopulationAction {
    None,
    Continue,
    SessionComplete,
    ShowReview,
    StatusMessage(String),
    Cancel,
}

/// A decision made about populating album_artist for a group
#[derive(Debug, Clone)]
pub struct PopulationDecision {
    /// The group identifier (album name or directory)
    pub group_key: String,
    /// The album_artist value chosen
    pub chosen_album_artist: String,
    /// Number of tracks affected
    pub track_count: usize,
    /// Generated pending changes
    pub pending_changes: Vec<PendingChange>,
}

/// Session state for the population workflow
#[derive(Debug, Clone)]
pub struct PopulationSession {
    pub session_id: String,
    pub groups: Vec<AlbumArtistPopulationGroup>,
    pub current_index: usize,
    pub decisions: Vec<PopulationDecision>,
}

impl PopulationSession {
    pub fn new(session_id: String, groups: Vec<AlbumArtistPopulationGroup>) -> Self {
        Self {
            session_id,
            groups,
            current_index: 0,
            decisions: Vec::new(),
        }
    }

    pub fn current_group(&self) -> Option<&AlbumArtistPopulationGroup> {
        self.groups.get(self.current_index)
    }

    pub fn is_complete(&self) -> bool {
        self.current_index >= self.groups.len()
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
        self.groups.len().saturating_sub(self.current_index)
    }
}

/// Focus area within the decision view
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopulationFocus {
    /// Selecting from the artist list
    ArtistList,
    /// Editing the custom value field
    CustomField,
    /// Action buttons
    ActionButtons,
}

/// Which action button is selected
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopulationActionButton {
    AcceptSuggested,
    UseSelected,
    UseCustom,
    Skip,
}

impl PopulationActionButton {
    fn all() -> &'static [PopulationActionButton] {
        &[
            PopulationActionButton::AcceptSuggested,
            PopulationActionButton::UseSelected,
            PopulationActionButton::UseCustom,
            PopulationActionButton::Skip,
        ]
    }

    fn label(&self, suggested: Option<&str>, selected_artist: Option<&str>) -> String {
        match self {
            PopulationActionButton::AcceptSuggested => {
                match suggested {
                    Some(val) => format!("Accept: \"{}\"", val),
                    None => "Accept suggested".to_string(),
                }
            }
            PopulationActionButton::UseSelected => {
                match selected_artist {
                    Some(a) => format!("Use: \"{}\"", a),
                    None => "Use selected artist".to_string(),
                }
            }
            PopulationActionButton::UseCustom => "Use custom value".to_string(),
            PopulationActionButton::Skip => "Skip this group".to_string(),
        }
    }
}

/// State for the population view - presents one group at a time
#[derive(Debug, Clone)]
pub struct PopulationState {
    session: PopulationSession,
    focus: PopulationFocus,
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

impl PopulationState {
    pub fn new(session: PopulationSession) -> Self {
        let mut state = Self {
            session,
            focus: PopulationFocus::ActionButtons,
            artist_cursor: 0,
            artist_list_state: ListState::default(),
            custom_value: String::new(),
            text_cursor: 0,
            action_cursor: 0,
            action_list_state: ListState::default(),
        };
        state.artist_list_state.select(Some(0));
        state.action_list_state.select(Some(0));
        state.reset_for_current_group();
        state
    }

    pub fn into_session(self) -> PopulationSession {
        self.session
    }

    fn reset_for_current_group(&mut self) {
        self.artist_cursor = 0;
        self.artist_list_state.select(Some(0));
        self.custom_value.clear();
        self.text_cursor = 0;
        self.action_cursor = 0;
        self.action_list_state.select(Some(0));
        self.focus = PopulationFocus::ActionButtons;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PopulationAction {
        match key.code {
            KeyCode::Esc => return PopulationAction::ShowReview,
            KeyCode::Tab => {
                self.focus = match self.focus {
                    PopulationFocus::ActionButtons => PopulationFocus::ArtistList,
                    PopulationFocus::ArtistList => PopulationFocus::CustomField,
                    PopulationFocus::CustomField => PopulationFocus::ActionButtons,
                };
                return PopulationAction::Continue;
            }
            KeyCode::BackTab => {
                self.focus = match self.focus {
                    PopulationFocus::ActionButtons => PopulationFocus::CustomField,
                    PopulationFocus::ArtistList => PopulationFocus::ActionButtons,
                    PopulationFocus::CustomField => PopulationFocus::ArtistList,
                };
                return PopulationAction::Continue;
            }
            _ => {}
        }

        match self.focus {
            PopulationFocus::ActionButtons => self.handle_action_key(key),
            PopulationFocus::ArtistList => self.handle_artist_key(key),
            PopulationFocus::CustomField => self.handle_custom_key(key),
        }
    }

    fn handle_action_key(&mut self, key: KeyEvent) -> PopulationAction {
        match key.code {
            KeyCode::Up => {
                if self.action_cursor > 0 {
                    self.action_cursor -= 1;
                    self.action_list_state.select(Some(self.action_cursor));
                }
                PopulationAction::Continue
            }
            KeyCode::Down => {
                let max = PopulationActionButton::all().len().saturating_sub(1);
                if self.action_cursor < max {
                    self.action_cursor += 1;
                    self.action_list_state.select(Some(self.action_cursor));
                }
                PopulationAction::Continue
            }
            KeyCode::Enter => self.execute_action(),
            KeyCode::Left => {
                self.focus = PopulationFocus::ArtistList;
                PopulationAction::Continue
            }
            _ => PopulationAction::None,
        }
    }

    fn handle_artist_key(&mut self, key: KeyEvent) -> PopulationAction {
        let artist_count = self.session.current_group()
            .map(|g| g.artists.len())
            .unwrap_or(0);

        match key.code {
            KeyCode::Up => {
                if self.artist_cursor > 0 {
                    self.artist_cursor -= 1;
                    self.artist_list_state.select(Some(self.artist_cursor));
                }
                PopulationAction::Continue
            }
            KeyCode::Down => {
                if self.artist_cursor + 1 < artist_count {
                    self.artist_cursor += 1;
                    self.artist_list_state.select(Some(self.artist_cursor));
                }
                PopulationAction::Continue
            }
            KeyCode::Enter => {
                self.action_cursor = 1; // UseSelected
                self.action_list_state.select(Some(1));
                self.execute_action()
            }
            KeyCode::Right => {
                self.focus = PopulationFocus::ActionButtons;
                PopulationAction::Continue
            }
            _ => PopulationAction::None,
        }
    }

    fn handle_custom_key(&mut self, key: KeyEvent) -> PopulationAction {
        match key.code {
            KeyCode::Char(c) => {
                self.custom_value.insert(self.text_cursor, c);
                self.text_cursor += 1;
                PopulationAction::Continue
            }
            KeyCode::Backspace => {
                if self.text_cursor > 0 {
                    self.text_cursor -= 1;
                    self.custom_value.remove(self.text_cursor);
                }
                PopulationAction::Continue
            }
            KeyCode::Delete => {
                if self.text_cursor < self.custom_value.len() {
                    self.custom_value.remove(self.text_cursor);
                }
                PopulationAction::Continue
            }
            KeyCode::Left => {
                if self.text_cursor > 0 {
                    self.text_cursor -= 1;
                } else {
                    self.focus = PopulationFocus::ArtistList;
                }
                PopulationAction::Continue
            }
            KeyCode::Right => {
                if self.text_cursor < self.custom_value.len() {
                    self.text_cursor += 1;
                } else {
                    self.focus = PopulationFocus::ActionButtons;
                }
                PopulationAction::Continue
            }
            KeyCode::Enter => {
                if !self.custom_value.trim().is_empty() {
                    self.action_cursor = 2; // UseCustom
                    self.action_list_state.select(Some(2));
                    self.execute_action()
                } else {
                    PopulationAction::Continue
                }
            }
            KeyCode::Up | KeyCode::Down => {
                self.focus = PopulationFocus::ActionButtons;
                PopulationAction::Continue
            }
            _ => PopulationAction::None,
        }
    }

    fn execute_action(&mut self) -> PopulationAction {
        let action = PopulationActionButton::all()
            .get(self.action_cursor)
            .copied()
            .unwrap_or(PopulationActionButton::Skip);

        match action {
            PopulationActionButton::AcceptSuggested => {
                if let Some(group) = self.session.current_group() {
                    if let Some(suggested) = &group.suggested_album_artist {
                        self.record_decision(suggested.clone());
                    } else if group.artists.len() == 1 {
                        // If single artist, use that
                        self.record_decision(group.artists[0].0.clone());
                    }
                }
                self.advance_to_next()
            }
            PopulationActionButton::UseSelected => {
                if let Some(group) = self.session.current_group() {
                    if let Some((artist, _)) = group.artists.get(self.artist_cursor) {
                        self.record_decision(artist.clone());
                    }
                }
                self.advance_to_next()
            }
            PopulationActionButton::UseCustom => {
                if !self.custom_value.trim().is_empty() {
                    self.record_decision(self.custom_value.trim().to_string());
                    self.advance_to_next()
                } else {
                    self.focus = PopulationFocus::CustomField;
                    PopulationAction::Continue
                }
            }
            PopulationActionButton::Skip => {
                self.advance_to_next()
            }
        }
    }

    fn record_decision(&mut self, chosen_album_artist: String) {
        let group = match self.session.current_group() {
            Some(g) => g.clone(),
            None => return,
        };

        let group_key = group.album_name
            .clone()
            .or_else(|| group.directory.clone())
            .unwrap_or_else(|| "[unknown]".to_string());

        let source_path = if group.album_name.is_some() {
            format!("[album:{}]", group_key)
        } else {
            format!("[directory:{}]", group_key)
        };

        let pending_changes = vec![PendingChange {
            id: None,
            session_id: self.session.session_id.clone(),
            change_type: ChangeType::TagEdit,
            source_path,
            target_path: None,
            metadata_changes: Some(
                serde_json::json!({
                    "operation": "population",
                    "album": group.album_name,
                    "directory": group.directory,
                    "new_album_artist": chosen_album_artist,
                    "total_tracks": group.total_tracks,
                })
                .to_string(),
            ),
            created_at: None,
            status: ChangeStatus::Pending,
        }];

        self.session.decisions.push(PopulationDecision {
            group_key,
            chosen_album_artist,
            track_count: group.total_tracks,
            pending_changes,
        });
    }

    fn advance_to_next(&mut self) -> PopulationAction {
        self.session.advance();
        if self.session.is_complete() {
            PopulationAction::SessionComplete
        } else {
            self.reset_for_current_group();
            PopulationAction::Continue
        }
    }

    /// Render the population decision view
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        if self.session.is_complete() || self.session.current_group().is_none() {
            let message = Paragraph::new("All groups have been reviewed.\nPress ESC to see review.")
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

        self.render_group_info(frame, main_chunks[0]);
        self.render_actions(frame, main_chunks[1]);
    }

    fn render_group_info(&mut self, frame: &mut Frame, area: Rect) {
        let group = match self.session.current_group() {
            Some(g) => g,
            None => return,
        };

        // Vertical split: Header | Artists list
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8), // Header with group info
                Constraint::Min(6),    // Artists list
            ])
            .split(area);

        // Header: Group info
        let progress = format!(
            "Group {}/{} ({} remaining)",
            self.session.current_index + 1,
            self.session.groups.len(),
            self.session.remaining_count().saturating_sub(1)
        );

        let group_type = if group.album_name.is_some() { "Album" } else { "Directory" };
        let group_name = group.album_name
            .as_deref()
            .or(group.directory.as_deref())
            .unwrap_or("[unknown]");

        let suggested_str = group.suggested_album_artist
            .as_deref()
            .unwrap_or("[no suggestion]");

        let header_lines = vec![
            Line::from(Span::styled(progress, Style::default().fg(Color::Cyan))),
            Line::from(""),
            Line::from(vec![
                Span::styled(format!("{}: ", group_type), Style::default().fg(Color::DarkGray)),
                Span::styled(group_name, Style::default().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::styled("Tracks: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{}", group.total_tracks)),
                Span::styled("  Artists: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("{}", group.artists.len())),
            ]),
            Line::from(vec![
                Span::styled("Suggested: ", Style::default().fg(Color::DarkGray)),
                Span::styled(suggested_str, Style::default().fg(Color::Green)),
            ]),
        ];

        let header = Paragraph::new(header_lines)
            .block(Block::default().borders(Borders::ALL).title(" Album Artist Population "));
        frame.render_widget(header, chunks[0]);

        // Artists list
        let focused = matches!(self.focus, PopulationFocus::ArtistList);
        let items: Vec<ListItem> = group.artists
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
        let group = self.session.current_group();

        // Vertical split: Custom field | Action buttons
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(4), // Custom input
                Constraint::Min(8),    // Action buttons
            ])
            .split(area);

        // Custom value input
        let custom_focused = matches!(self.focus, PopulationFocus::CustomField);
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
        let actions_focused = matches!(self.focus, PopulationFocus::ActionButtons);
        let suggested = group.and_then(|g| g.suggested_album_artist.as_deref());
        let selected_artist = group.and_then(|g| {
            g.artists.get(self.artist_cursor).map(|(name, _)| name.as_str())
        });

        let items: Vec<ListItem> = PopulationActionButton::all()
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
// Population Review State
// ============================================================================

/// Which element is focused in the review screen
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopulationReviewFocus {
    DecisionList,
    CommitButton,
    CancelButton,
}

/// Actions returned from the population review screen
#[derive(Debug, Clone)]
pub enum PopulationReviewAction {
    None,
    Continue,
    Commit,
    Cancel,
    BackToPopulation,
}

/// State for the population session review screen
#[derive(Debug, Clone)]
pub struct PopulationReviewState {
    session: PopulationSession,
    focus: PopulationReviewFocus,
    list_state: ListState,
    selected_idx: usize,
}

impl PopulationReviewState {
    /// Create a new review state from a session
    pub fn new(session: PopulationSession) -> Self {
        let mut state = Self {
            session,
            focus: PopulationReviewFocus::DecisionList,
            list_state: ListState::default(),
            selected_idx: 0,
        };
        if !state.session.decisions.is_empty() {
            state.list_state.select(Some(0));
        }
        state
    }

    /// Get a reference to the session
    pub fn session(&self) -> &PopulationSession {
        &self.session
    }

    /// Take ownership of the session
    pub fn into_session(self) -> PopulationSession {
        self.session
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> PopulationReviewAction {
        match key.code {
            KeyCode::Up => {
                match self.focus {
                    PopulationReviewFocus::DecisionList => {
                        self.move_selection(-1);
                    }
                    PopulationReviewFocus::CommitButton => {
                        self.focus = PopulationReviewFocus::DecisionList;
                    }
                    PopulationReviewFocus::CancelButton => {
                        self.focus = PopulationReviewFocus::CommitButton;
                    }
                }
                PopulationReviewAction::Continue
            }
            KeyCode::Down => {
                match self.focus {
                    PopulationReviewFocus::DecisionList => {
                        if self.selected_idx >= self.session.decisions.len().saturating_sub(1) {
                            self.focus = PopulationReviewFocus::CommitButton;
                        } else {
                            self.move_selection(1);
                        }
                    }
                    PopulationReviewFocus::CommitButton => {
                        self.focus = PopulationReviewFocus::CancelButton;
                    }
                    PopulationReviewFocus::CancelButton => {}
                }
                PopulationReviewAction::Continue
            }
            KeyCode::Left => {
                if matches!(self.focus, PopulationReviewFocus::CancelButton) {
                    self.focus = PopulationReviewFocus::CommitButton;
                }
                PopulationReviewAction::Continue
            }
            KeyCode::Right => {
                if matches!(self.focus, PopulationReviewFocus::CommitButton) {
                    self.focus = PopulationReviewFocus::CancelButton;
                }
                PopulationReviewAction::Continue
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    PopulationReviewFocus::DecisionList => PopulationReviewFocus::CommitButton,
                    PopulationReviewFocus::CommitButton => PopulationReviewFocus::CancelButton,
                    PopulationReviewFocus::CancelButton => PopulationReviewFocus::DecisionList,
                };
                PopulationReviewAction::Continue
            }
            KeyCode::Enter | KeyCode::Char(' ') => match self.focus {
                PopulationReviewFocus::DecisionList => PopulationReviewAction::None,
                PopulationReviewFocus::CommitButton => PopulationReviewAction::Commit,
                PopulationReviewFocus::CancelButton => PopulationReviewAction::Cancel,
            },
            KeyCode::Esc => PopulationReviewAction::Cancel,
            KeyCode::BackTab => PopulationReviewAction::BackToPopulation,
            _ => PopulationReviewAction::None,
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
        let title = format!(
            "Population Review - {} Decisions",
            self.session.decisions.len()
        );

        let header = Paragraph::new(Line::from(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Album Artist Population"),
        );

        f.render_widget(header, area);
    }

    fn render_decisions(&mut self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.focus, PopulationReviewFocus::DecisionList);

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
                        format!("\"{}\"", decision.group_key),
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

                let line2 = Line::from(vec![
                    Span::raw("   "),
                    Span::styled(
                        format!("{} tracks", decision.track_count),
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
        let total_tracks: usize = self.session.decisions.iter().map(|d| d.track_count).sum();

        let lines = vec![
            Line::from(vec![
                Span::styled("Groups processed: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    total_decisions.to_string(),
                    Style::default().fg(Color::White),
                ),
            ]),
            Line::from(vec![
                Span::styled("Total tracks: ", Style::default().fg(Color::DarkGray)),
                Span::styled(total_tracks.to_string(), Style::default().fg(Color::Yellow)),
            ]),
        ];

        let summary =
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("Summary"));

        f.render_widget(summary, area);
    }

    fn render_buttons(&self, f: &mut Frame, area: Rect) {
        use ratatui::layout::Alignment;

        let commit_focused = matches!(self.focus, PopulationReviewFocus::CommitButton);
        let cancel_focused = matches!(self.focus, PopulationReviewFocus::CancelButton);

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
