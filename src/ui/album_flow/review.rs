//! Album Flow Review
//!
//! Session review before committing album canonicalization changes.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use super::session::AlbumCanonSession;
use super::types::AlbumReviewAction;

/// Focus within the review screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewFocus {
    DecisionList,
    CommitButton,
    CancelButton,
}

/// State for the album review view.
#[derive(Debug, Clone)]
pub struct AlbumReviewState {
    session: AlbumCanonSession,
    focus: ReviewFocus,
    list_state: ListState,
    cursor_idx: usize,
}

impl AlbumReviewState {
    /// Create a new review state from a session.
    pub fn new(session: AlbumCanonSession) -> Self {
        let mut state = Self {
            session,
            focus: ReviewFocus::DecisionList,
            list_state: ListState::default(),
            cursor_idx: 0,
        };
        state.list_state.select(Some(0));
        state
    }

    /// Take ownership of the session.
    pub fn into_session(self) -> AlbumCanonSession {
        self.session
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> AlbumReviewAction {
        match key.code {
            KeyCode::Esc => AlbumReviewAction::BackToClusterView,
            KeyCode::Up => {
                match self.focus {
                    ReviewFocus::DecisionList => {
                        if self.cursor_idx > 0 {
                            self.cursor_idx -= 1;
                            self.list_state.select(Some(self.cursor_idx));
                        }
                    }
                    ReviewFocus::CommitButton => {
                        self.focus = ReviewFocus::DecisionList;
                    }
                    ReviewFocus::CancelButton => {
                        self.focus = ReviewFocus::CommitButton;
                    }
                }
                AlbumReviewAction::Continue
            }
            KeyCode::Down => {
                match self.focus {
                    ReviewFocus::DecisionList => {
                        if self.cursor_idx + 1 < self.session.decisions.len() {
                            self.cursor_idx += 1;
                            self.list_state.select(Some(self.cursor_idx));
                        } else {
                            self.focus = ReviewFocus::CommitButton;
                        }
                    }
                    ReviewFocus::CommitButton => {
                        self.focus = ReviewFocus::CancelButton;
                    }
                    ReviewFocus::CancelButton => {
                        self.focus = ReviewFocus::DecisionList;
                        self.cursor_idx = 0;
                        self.list_state.select(Some(0));
                    }
                }
                AlbumReviewAction::Continue
            }
            KeyCode::Enter => {
                match self.focus {
                    ReviewFocus::DecisionList => {
                        self.focus = ReviewFocus::CommitButton;
                    }
                    ReviewFocus::CommitButton => {
                        return AlbumReviewAction::Commit;
                    }
                    ReviewFocus::CancelButton => {
                        return AlbumReviewAction::Cancel;
                    }
                }
                AlbumReviewAction::Continue
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    ReviewFocus::DecisionList => ReviewFocus::CommitButton,
                    ReviewFocus::CommitButton => ReviewFocus::CancelButton,
                    ReviewFocus::CancelButton => ReviewFocus::DecisionList,
                };
                AlbumReviewAction::Continue
            }
            _ => AlbumReviewAction::None,
        }
    }

    /// Render the review view.
    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Min(10),   // Decision list
                Constraint::Length(5), // Buttons
            ])
            .split(area);

        self.render_header(frame, chunks[0]);
        self.render_decisions(frame, chunks[1]);
        self.render_buttons(frame, chunks[2]);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let flagged_count = self.session.decisions.iter().filter(|d| d.flag_for_review).count();

        let header_text = vec![
            Line::from(vec![
                Span::styled("Album Canonicalization Review", Style::default().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::raw(format!(
                    "{} decisions, {} tracks affected",
                    self.session.decision_count(),
                    self.session.total_affected_tracks()
                )),
                if flagged_count > 0 {
                    Span::styled(
                        format!(" ({} flagged for review)", flagged_count),
                        Style::default().fg(Color::Yellow),
                    )
                } else {
                    Span::raw("")
                },
            ]),
        ];

        let header = Paragraph::new(header_text)
            .block(Block::default().borders(Borders::BOTTOM));
        frame.render_widget(header, area);
    }

    fn render_decisions(&mut self, frame: &mut Frame, area: Rect) {
        let focused = matches!(self.focus, ReviewFocus::DecisionList);

        let items: Vec<ListItem> = self
            .session
            .decisions
            .iter()
            .enumerate()
            .map(|(idx, decision)| {
                let is_cursor = idx == self.cursor_idx;

                let flag_indicator = if decision.flag_for_review {
                    Span::styled(" [!]", Style::default().fg(Color::Yellow))
                } else {
                    Span::raw("")
                };

                let content = Line::from(vec![
                    Span::raw(format!(
                        "{} variants → '{}'",
                        decision.variants_to_rename.len(),
                        decision.canonical_name
                    )),
                    flag_indicator,
                ]);

                let style = if is_cursor && focused {
                    Style::default()
                        .bg(Color::Blue)
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else if is_cursor {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };

                ListItem::new(content).style(style)
            })
            .collect();

        let block = Block::default()
            .title(" Decisions ")
            .borders(Borders::ALL)
            .border_style(if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            });

        let list = List::new(items).block(block);
        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_buttons(&self, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Percentage(50),
            ])
            .split(area);

        // Commit button
        let commit_focused = matches!(self.focus, ReviewFocus::CommitButton);
        let commit_style = if commit_focused {
            Style::default()
                .bg(Color::Green)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };

        let commit_btn = Paragraph::new("[ Commit Changes ]")
            .style(commit_style)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(commit_btn, chunks[0]);

        // Cancel button
        let cancel_focused = matches!(self.focus, ReviewFocus::CancelButton);
        let cancel_style = if cancel_focused {
            Style::default()
                .bg(Color::Red)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Red)
        };

        let cancel_btn = Paragraph::new("[ Cancel ]")
            .style(cancel_style)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(cancel_btn, chunks[1]);
    }
}
