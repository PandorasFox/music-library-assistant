//! Album Artist Session Review UI
//!
//! Displays all canonicalization decisions made during the session with a
//! scrollable list and summary statistics. Allows committing or canceling.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use super::session::AlbumArtistCanonSession;

/// Which element is focused
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewFocus {
    DecisionList,
    CommitButton,
    CancelButton,
}

/// Actions returned from the review screen
#[derive(Debug, Clone)]
pub enum AlbumArtistReviewAction {
    None,
    Continue,
    Commit,
    Cancel,
    BackToClusterView,
}

/// State for the session review screen
#[derive(Debug, Clone)]
pub struct AlbumArtistReviewState {
    session: AlbumArtistCanonSession,
    focus: ReviewFocus,
    list_state: ListState,
    selected_idx: usize,
}

impl AlbumArtistReviewState {
    /// Create a new review state from a session
    pub fn new(session: AlbumArtistCanonSession) -> Self {
        let mut state = Self {
            session,
            focus: ReviewFocus::DecisionList,
            list_state: ListState::default(),
            selected_idx: 0,
        };
        if !state.session.decisions.is_empty() {
            state.list_state.select(Some(0));
        }
        state
    }

    /// Get a reference to the session
    pub fn session(&self) -> &AlbumArtistCanonSession {
        &self.session
    }

    /// Take ownership of the session
    pub fn into_session(self) -> AlbumArtistCanonSession {
        self.session
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> AlbumArtistReviewAction {
        match key.code {
            KeyCode::Up => {
                match self.focus {
                    ReviewFocus::DecisionList => {
                        self.move_selection(-1);
                    }
                    ReviewFocus::CommitButton => {
                        self.focus = ReviewFocus::DecisionList;
                    }
                    ReviewFocus::CancelButton => {
                        self.focus = ReviewFocus::CommitButton;
                    }
                }
                AlbumArtistReviewAction::Continue
            }
            KeyCode::Down => {
                match self.focus {
                    ReviewFocus::DecisionList => {
                        if self.selected_idx >= self.session.decisions.len().saturating_sub(1) {
                            self.focus = ReviewFocus::CommitButton;
                        } else {
                            self.move_selection(1);
                        }
                    }
                    ReviewFocus::CommitButton => {
                        self.focus = ReviewFocus::CancelButton;
                    }
                    ReviewFocus::CancelButton => {}
                }
                AlbumArtistReviewAction::Continue
            }
            KeyCode::Left => {
                if matches!(self.focus, ReviewFocus::CancelButton) {
                    self.focus = ReviewFocus::CommitButton;
                }
                AlbumArtistReviewAction::Continue
            }
            KeyCode::Right => {
                if matches!(self.focus, ReviewFocus::CommitButton) {
                    self.focus = ReviewFocus::CancelButton;
                }
                AlbumArtistReviewAction::Continue
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    ReviewFocus::DecisionList => ReviewFocus::CommitButton,
                    ReviewFocus::CommitButton => ReviewFocus::CancelButton,
                    ReviewFocus::CancelButton => ReviewFocus::DecisionList,
                };
                AlbumArtistReviewAction::Continue
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                match self.focus {
                    ReviewFocus::DecisionList => AlbumArtistReviewAction::None,
                    ReviewFocus::CommitButton => AlbumArtistReviewAction::Commit,
                    ReviewFocus::CancelButton => AlbumArtistReviewAction::Cancel,
                }
            }
            KeyCode::Esc => AlbumArtistReviewAction::Cancel,
            KeyCode::BackTab => AlbumArtistReviewAction::BackToClusterView,
            _ => AlbumArtistReviewAction::None,
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
        let title = format!("Session Review - {} Decisions", self.session.decisions.len());

        let header = Paragraph::new(Line::from(Span::styled(
            title,
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        )))
        .block(Block::default().borders(Borders::ALL).title("Album Artist Canonicalization"));

        f.render_widget(header, area);
    }

    fn render_decisions(&mut self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.focus, ReviewFocus::DecisionList);

        if self.session.decisions.is_empty() {
            let empty = Paragraph::new("No decisions made yet. Press Shift+Tab to go back.")
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::ALL).title("Decisions"));
            f.render_widget(empty, area);
            return;
        }

        let items: Vec<ListItem> = self.session.decisions.iter().enumerate().map(|(i, decision)| {
            let is_selected = i == self.selected_idx && is_focused;

            let variants_str = decision.variants_to_rename.join(", ");
            let affected = decision.affected_track_count();

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
                    format!("\"{}\"", decision.canonical_name),
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" <- ", Style::default().fg(Color::DarkGray)),
                Span::styled(variants_str, Style::default()),
            ]);

            let line2 = Line::from(vec![
                Span::raw("   "),
                Span::styled(
                    format!("{} tracks affected", affected),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            ListItem::new(vec![line1, line2])
        }).collect();

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
            .constraints([
                Constraint::Percentage(60),
                Constraint::Percentage(40),
            ])
            .split(area);

        self.render_summary(f, footer_chunks[0]);
        self.render_buttons(f, footer_chunks[1]);
    }

    fn render_summary(&self, f: &mut Frame, area: Rect) {
        let total_decisions = self.session.decisions.len();
        let tracks_to_modify: usize = self.session.decisions.iter()
            .map(|d| d.affected_track_count())
            .sum();
        let tracks_unchanged: usize = self.session.decisions.iter()
            .map(|d| {
                d.bucket.variants.iter()
                    .filter(|v| v.name == d.canonical_name)
                    .map(|v| v.track_count)
                    .sum::<usize>()
            })
            .sum();

        let lines = vec![
            Line::from(vec![
                Span::styled("Decisions: ", Style::default().fg(Color::DarkGray)),
                Span::styled(total_decisions.to_string(), Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("Tracks to update: ", Style::default().fg(Color::DarkGray)),
                Span::styled(tracks_to_modify.to_string(), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::styled("Tracks unchanged: ", Style::default().fg(Color::DarkGray)),
                Span::styled(tracks_unchanged.to_string(), Style::default().fg(Color::Green)),
            ]),
        ];

        let summary = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title("Summary"));

        f.render_widget(summary, area);
    }

    fn render_buttons(&self, f: &mut Frame, area: Rect) {
        let commit_focused = matches!(self.focus, ReviewFocus::CommitButton);
        let cancel_focused = matches!(self.focus, ReviewFocus::CancelButton);

        let commit_style = if commit_focused {
            Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };

        let cancel_style = if cancel_focused {
            Style::default().fg(Color::Black).bg(Color::Red).add_modifier(Modifier::BOLD)
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
