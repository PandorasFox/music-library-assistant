//! Album Artist Population Flow
//!
//! UI for bulk-filling missing album_artist tags.
//! Groups tracks by album (or directory if no album tag) and suggests album_artist values.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
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
}

/// Which pane is focused
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopulationPane {
    Groups,
    Artists,
    Action,
}

/// Focus within the Action pane
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopulationActionFocus {
    NameField,
    ConfirmButton,
    SkipButton,
}

/// State for the population view
#[derive(Debug, Clone)]
pub struct PopulationState {
    session: PopulationSession,
    focused_pane: PopulationPane,
    groups_cursor: usize,
    groups_list_state: ListState,
    artists_cursor: usize,
    artists_list_state: ListState,
    action_focus: PopulationActionFocus,
    album_artist_value: String,
    text_cursor: usize,
    status_message: Option<String>,
    confirmed: bool,
}

impl PopulationState {
    pub fn new(session: PopulationSession) -> Self {
        let album_artist_value = session
            .current_group()
            .and_then(|g| g.suggested_album_artist.clone())
            .unwrap_or_default();
        let text_cursor = album_artist_value.len();

        let mut state = Self {
            session,
            focused_pane: PopulationPane::Groups,
            groups_cursor: 0,
            groups_list_state: ListState::default(),
            artists_cursor: 0,
            artists_list_state: ListState::default(),
            action_focus: PopulationActionFocus::NameField,
            album_artist_value,
            text_cursor,
            status_message: None,
            confirmed: false,
        };
        state.groups_list_state.select(Some(0));
        state.artists_list_state.select(Some(0));
        state
    }

    pub fn into_session(self) -> PopulationSession {
        self.session
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PopulationAction {
        match key.code {
            KeyCode::Tab => return self.advance_to_next_group(),
            KeyCode::BackTab => return self.go_to_previous_group(),
            KeyCode::Esc => return PopulationAction::ShowReview,
            KeyCode::Right => {
                match self.focused_pane {
                    PopulationPane::Groups => {
                        self.focused_pane = PopulationPane::Artists;
                        return PopulationAction::Continue;
                    }
                    PopulationPane::Artists => {
                        self.focused_pane = PopulationPane::Action;
                        return PopulationAction::Continue;
                    }
                    PopulationPane::Action => {
                        if matches!(self.action_focus, PopulationActionFocus::NameField) {
                            if self.text_cursor < self.album_artist_value.len() {
                                self.text_cursor += 1;
                                return PopulationAction::Continue;
                            }
                        }
                    }
                }
            }
            KeyCode::Left => {
                match self.focused_pane {
                    PopulationPane::Action => {
                        if matches!(self.action_focus, PopulationActionFocus::NameField)
                            && self.text_cursor > 0
                        {
                            self.text_cursor -= 1;
                            return PopulationAction::Continue;
                        } else {
                            self.focused_pane = PopulationPane::Artists;
                            return PopulationAction::Continue;
                        }
                    }
                    PopulationPane::Artists => {
                        self.focused_pane = PopulationPane::Groups;
                        return PopulationAction::Continue;
                    }
                    PopulationPane::Groups => {}
                }
            }
            _ => {}
        }

        match self.focused_pane {
            PopulationPane::Groups => self.handle_groups_key(key),
            PopulationPane::Artists => self.handle_artists_key(key),
            PopulationPane::Action => self.handle_action_key(key),
        }
    }

    fn handle_groups_key(&mut self, key: KeyEvent) -> PopulationAction {
        match key.code {
            KeyCode::Up => {
                if self.groups_cursor > 0 {
                    self.groups_cursor -= 1;
                    self.groups_list_state.select(Some(self.groups_cursor));
                    self.update_for_current_group();
                }
                PopulationAction::Continue
            }
            KeyCode::Down => {
                if self.groups_cursor + 1 < self.session.groups.len() {
                    self.groups_cursor += 1;
                    self.groups_list_state.select(Some(self.groups_cursor));
                    self.update_for_current_group();
                }
                PopulationAction::Continue
            }
            KeyCode::Enter => {
                self.focused_pane = PopulationPane::Action;
                PopulationAction::Continue
            }
            _ => PopulationAction::None,
        }
    }

    fn handle_artists_key(&mut self, key: KeyEvent) -> PopulationAction {
        let current_group = match self.session.groups.get(self.groups_cursor) {
            Some(g) => g,
            None => return PopulationAction::None,
        };

        match key.code {
            KeyCode::Up => {
                if self.artists_cursor > 0 {
                    self.artists_cursor -= 1;
                    self.artists_list_state.select(Some(self.artists_cursor));
                }
                PopulationAction::Continue
            }
            KeyCode::Down => {
                if self.artists_cursor + 1 < current_group.artists.len() {
                    self.artists_cursor += 1;
                    self.artists_list_state.select(Some(self.artists_cursor));
                }
                PopulationAction::Continue
            }
            KeyCode::Enter => {
                // Use this artist as the album_artist
                if let Some((artist_name, _)) = current_group.artists.get(self.artists_cursor) {
                    self.album_artist_value = artist_name.clone();
                    self.text_cursor = self.album_artist_value.len();
                }
                PopulationAction::Continue
            }
            _ => PopulationAction::None,
        }
    }

    fn handle_action_key(&mut self, key: KeyEvent) -> PopulationAction {
        match key.code {
            KeyCode::Up => {
                self.action_focus = match self.action_focus {
                    PopulationActionFocus::NameField => PopulationActionFocus::SkipButton,
                    PopulationActionFocus::ConfirmButton => PopulationActionFocus::NameField,
                    PopulationActionFocus::SkipButton => PopulationActionFocus::ConfirmButton,
                };
                PopulationAction::Continue
            }
            KeyCode::Down => {
                self.action_focus = match self.action_focus {
                    PopulationActionFocus::NameField => PopulationActionFocus::ConfirmButton,
                    PopulationActionFocus::ConfirmButton => PopulationActionFocus::SkipButton,
                    PopulationActionFocus::SkipButton => PopulationActionFocus::NameField,
                };
                PopulationAction::Continue
            }
            KeyCode::Enter => {
                match self.action_focus {
                    PopulationActionFocus::NameField => {
                        self.action_focus = PopulationActionFocus::ConfirmButton;
                    }
                    PopulationActionFocus::ConfirmButton => {
                        return self.confirm_and_record_decision();
                    }
                    PopulationActionFocus::SkipButton => {
                        return self.advance_to_next_group();
                    }
                }
                PopulationAction::Continue
            }
            KeyCode::Char(c) => {
                if matches!(self.action_focus, PopulationActionFocus::NameField) {
                    self.album_artist_value.insert(self.text_cursor, c);
                    self.text_cursor += 1;
                }
                PopulationAction::Continue
            }
            KeyCode::Backspace => {
                if matches!(self.action_focus, PopulationActionFocus::NameField)
                    && self.text_cursor > 0
                {
                    self.text_cursor -= 1;
                    self.album_artist_value.remove(self.text_cursor);
                }
                PopulationAction::Continue
            }
            KeyCode::Delete => {
                if matches!(self.action_focus, PopulationActionFocus::NameField)
                    && self.text_cursor < self.album_artist_value.len()
                {
                    self.album_artist_value.remove(self.text_cursor);
                }
                PopulationAction::Continue
            }
            _ => PopulationAction::None,
        }
    }

    fn update_for_current_group(&mut self) {
        self.artists_cursor = 0;
        self.artists_list_state.select(Some(0));
        self.confirmed = false;

        if let Some(group) = self.session.groups.get(self.groups_cursor) {
            self.album_artist_value = group
                .suggested_album_artist
                .clone()
                .unwrap_or_default();
            self.text_cursor = self.album_artist_value.len();
        }
    }

    fn confirm_and_record_decision(&mut self) -> PopulationAction {
        let current_group = match self.session.groups.get(self.groups_cursor) {
            Some(g) => g,
            None => return PopulationAction::None,
        };

        let group_key = current_group
            .album_name
            .clone()
            .unwrap_or_else(|| {
                current_group
                    .directory
                    .clone()
                    .unwrap_or_else(|| "[unknown]".to_string())
            });

        // Generate pending changes for setting album_artist
        let mut pending_changes = Vec::new();

        pending_changes.push(PendingChange {
            id: None,
            session_id: self.session.session_id.clone(),
            change_type: ChangeType::TagEdit,
            source_path: if current_group.album_name.is_some() {
                format!("[album:{}]", group_key)
            } else {
                format!("[directory:{}]", group_key)
            },
            target_path: None,
            metadata_changes: Some(
                serde_json::json!({
                    "operation": "population",
                    "album": current_group.album_name,
                    "directory": current_group.directory,
                    "new_album_artist": self.album_artist_value,
                    "total_tracks": current_group.total_tracks,
                })
                .to_string(),
            ),
            created_at: None,
            status: ChangeStatus::Pending,
        });

        let decision = PopulationDecision {
            group_key: group_key.clone(),
            chosen_album_artist: self.album_artist_value.clone(),
            track_count: current_group.total_tracks,
            pending_changes,
        };

        self.session.decisions.push(decision);
        self.confirmed = true;

        let msg = format!(
            "Set album_artist='{}' for {} ({} tracks)",
            self.album_artist_value, group_key, current_group.total_tracks
        );
        self.status_message = Some(msg.clone());

        PopulationAction::StatusMessage(msg)
    }

    fn advance_to_next_group(&mut self) -> PopulationAction {
        if self.groups_cursor + 1 < self.session.groups.len() {
            self.groups_cursor += 1;
            self.groups_list_state.select(Some(self.groups_cursor));
            self.update_for_current_group();
            PopulationAction::Continue
        } else {
            PopulationAction::SessionComplete
        }
    }

    fn go_to_previous_group(&mut self) -> PopulationAction {
        if self.groups_cursor > 0 {
            self.groups_cursor -= 1;
            self.groups_list_state.select(Some(self.groups_cursor));
            self.update_for_current_group();
        }
        PopulationAction::Continue
    }

    /// Render the population view
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        // Three-pane layout: Groups | Artists | Action
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(40),
                Constraint::Percentage(30),
                Constraint::Percentage(30),
            ])
            .split(area);

        self.render_groups_pane(frame, chunks[0]);
        self.render_artists_pane(frame, chunks[1]);
        self.render_action_pane(frame, chunks[2]);
    }

    fn render_groups_pane(&mut self, frame: &mut Frame, area: Rect) {
        let focused = matches!(self.focused_pane, PopulationPane::Groups);

        let title = format!(
            " Groups ({}/{}) ",
            self.groups_cursor + 1,
            self.session.groups.len()
        );

        let items: Vec<ListItem> = self
            .session
            .groups
            .iter()
            .enumerate()
            .map(|(idx, group)| {
                let is_selected = idx == self.groups_cursor;
                let group_key = group
                    .album_name
                    .as_ref()
                    .map(|a| format!("[Album] {}", a))
                    .or_else(|| {
                        group.directory.as_ref().map(|d| {
                            // Truncate directory path for display
                            let display = if d.len() > 30 {
                                format!("...{}", &d[d.len() - 27..])
                            } else {
                                d.clone()
                            };
                            format!("[Dir] {}", display)
                        })
                    })
                    .unwrap_or_else(|| "[Unknown]".to_string());

                let has_decision = self
                    .session
                    .decisions
                    .iter()
                    .any(|d| {
                        d.group_key == group.album_name.as_deref().unwrap_or("")
                            || d.group_key == group.directory.as_deref().unwrap_or("")
                    });

                let prefix = if has_decision { "[x] " } else { "[ ] " };
                let content = format!("{}{} ({} tracks)", prefix, group_key, group.total_tracks);

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

        frame.render_stateful_widget(list, area, &mut self.groups_list_state);
    }

    fn render_artists_pane(&mut self, frame: &mut Frame, area: Rect) {
        let focused = matches!(self.focused_pane, PopulationPane::Artists);

        let current_group = self.session.groups.get(self.groups_cursor);

        let items: Vec<ListItem> = if let Some(group) = current_group {
            group
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

        let title = " Artists in Group ";

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
        let focused = matches!(self.focused_pane, PopulationPane::Action);

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
                Constraint::Length(3), // Skip button
                Constraint::Min(0),    // Status/info
            ])
            .split(inner);

        // Label
        let label = Paragraph::new("Album Artist:");
        frame.render_widget(label, chunks[0]);

        // Text input
        let input_style = if focused
            && matches!(self.action_focus, PopulationActionFocus::NameField)
        {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };

        let display_text = if focused
            && matches!(self.action_focus, PopulationActionFocus::NameField)
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
        let confirm_focused =
            focused && matches!(self.action_focus, PopulationActionFocus::ConfirmButton);
        let confirm_style = if confirm_focused {
            Style::default()
                .bg(Color::Green)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else if self.confirmed {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let confirm_text = if self.confirmed {
            "[ Confirmed ]"
        } else {
            "[ Confirm ]"
        };

        let confirm_btn = Paragraph::new(confirm_text)
            .style(confirm_style)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(confirm_btn, chunks[3]);

        // Skip button
        let skip_focused =
            focused && matches!(self.action_focus, PopulationActionFocus::SkipButton);
        let skip_style = if skip_focused {
            Style::default()
                .bg(Color::Yellow)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let skip_btn = Paragraph::new("[ Skip ]")
            .style(skip_style)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(skip_btn, chunks[5]);

        // Status info
        if let Some(group) = self.session.groups.get(self.groups_cursor) {
            let info_lines = vec![
                Line::from(vec![
                    Span::styled("Suggested: ", Style::default().fg(Color::DarkGray)),
                    Span::raw(
                        group
                            .suggested_album_artist
                            .as_deref()
                            .unwrap_or("[none]"),
                    ),
                ]),
            ];

            let info = Paragraph::new(info_lines).wrap(Wrap { trim: true });
            frame.render_widget(info, chunks[6]);
        }
    }

    pub fn get_status_message(&self) -> Option<&str> {
        self.status_message.as_deref()
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
