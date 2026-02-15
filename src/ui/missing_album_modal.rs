//! Missing Album Singles Resolution Modal
//!
//! Shows tracks that lack an ALBUM tag but have ARTIST and TITLE,
//! grouped by artist. Offers quick actions to assign album values
//! or suppress the signal.
//!
//! - Tab/Shift-Tab: Navigate between artist groups
//! - j/k/Up/Down: Navigate track list within group
//! - Enter: Tag each track as "{title}{suffix}" (per-track singles)
//! - S: Tag all tracks as "Singles"
//! - Ctrl+F: Suppress (mark as expected-missing-tag)
//! - Ctrl+R: Show transaction review
//! - Escape: Cancel

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use crate::meta::signals::data::MissingAlbumSingleSignal;
use crate::ui::helpers::render_pane;

/// Actions returned from the missing album modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingAlbumAction {
    /// No action needed.
    None,
    /// Cancel and return to Insights view.
    Cancel,
    /// Tag each track as "{title}{suffix}" for the current group.
    TagAsSingles,
    /// Tag all tracks in current group as "Singles".
    TagAllSingles,
    /// Suppress: emit ExpectedMissingTag for all inodes in current group.
    Suppress,
    /// Navigate to next/prev artist group.
    NavigateGroup(bool),
    /// Show transaction review.
    ShowReview,
}

/// A single artist group with its tracks.
#[derive(Debug, Clone)]
pub struct ArtistGroup {
    pub artist: String,
    pub tracks: Vec<TrackEntry>,
}

/// A single track entry within an artist group.
#[derive(Debug, Clone)]
pub struct TrackEntry {
    pub inode: i64,
    pub title: String,
    pub path: String,
}

/// Loaded data for the modal.
#[derive(Debug, Clone)]
pub struct MissingAlbumData {
    pub groups: Vec<ArtistGroup>,
}

impl MissingAlbumData {
    /// Build from loaded signals.
    pub fn from_signals(signals: Vec<MissingAlbumSingleSignal>) -> Self {
        let groups = signals
            .into_iter()
            .map(|s| ArtistGroup {
                artist: s.data.artist,
                tracks: s.data.tracks.into_iter().map(|t| TrackEntry {
                    inode: t.inode,
                    title: t.title,
                    path: t.path,
                }).collect(),
            })
            .collect();
        Self { groups }
    }
}

/// State for the missing album singles resolution modal.
#[derive(Debug)]
pub struct MissingAlbumState {
    /// Loaded data.
    pub data: MissingAlbumData,
    /// Current artist group index.
    pub current_group: usize,
    /// Track cursor within current group.
    pub track_cursor: usize,
    /// Suffix from config opinion.
    pub suffix: String,
}

impl MissingAlbumState {
    pub fn new(data: MissingAlbumData, suffix: String) -> Self {
        Self {
            data,
            current_group: 0,
            track_cursor: 0,
            suffix,
        }
    }

    /// Get the current artist group, if any.
    pub fn current_group_data(&self) -> Option<&ArtistGroup> {
        self.data.groups.get(self.current_group)
    }

    /// Path of the currently selected track (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.current_group_data()
            .and_then(|g| g.tracks.get(self.track_cursor))
            .map(|t| t.path.as_str())
    }

    /// Advance to next group after a decision. Returns true if there are more groups.
    pub fn advance_group(&mut self) -> bool {
        // Remove the current group (it was resolved)
        if self.current_group < self.data.groups.len() {
            self.data.groups.remove(self.current_group);
        }
        // Adjust cursor if we went past the end
        if self.current_group >= self.data.groups.len() && !self.data.groups.is_empty() {
            self.current_group = self.data.groups.len() - 1;
        }
        self.track_cursor = 0;
        !self.data.groups.is_empty()
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> MissingAlbumAction {
        match key.code {
            KeyCode::Esc => MissingAlbumAction::Cancel,

            KeyCode::Enter => MissingAlbumAction::TagAsSingles,

            KeyCode::Char('s') | KeyCode::Char('S')
                if !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                MissingAlbumAction::TagAllSingles
            }

            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                MissingAlbumAction::Suppress
            }

            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                MissingAlbumAction::ShowReview
            }

            KeyCode::Tab => MissingAlbumAction::NavigateGroup(true),

            KeyCode::BackTab => MissingAlbumAction::NavigateGroup(false),

            KeyCode::Up | KeyCode::Char('k') => {
                if self.track_cursor > 0 {
                    self.track_cursor -= 1;
                }
                MissingAlbumAction::None
            }

            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(group) = self.current_group_data() {
                    if self.track_cursor + 1 < group.tracks.len() {
                        self.track_cursor += 1;
                    }
                }
                MissingAlbumAction::None
            }

            _ => MissingAlbumAction::None,
        }
    }

    /// Render the modal.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        f.render_widget(Clear, area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // header
                Constraint::Min(5),    // content
                Constraint::Length(3), // controls
            ])
            .split(area);

        let group = self.current_group_data();
        let total_groups = self.data.groups.len();

        // Header
        let header_text = if let Some(g) = group {
            format!(
                " Artist: {} ({} tracks) [group {}/{}] ",
                g.artist,
                g.tracks.len(),
                self.current_group + 1,
                total_groups,
            )
        } else {
            " No missing album singles ".to_string()
        };

        let header_block = Block::default()
            .borders(Borders::ALL)
            .title(header_text)
            .border_style(Style::default().fg(Color::Yellow));
        render_pane(f, chunks[0], header_block);

        // Content: two-pane layout (tracks + preview)
        if let Some(g) = group {
            let content_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(60),
                    Constraint::Percentage(40),
                ])
                .split(chunks[1]);

            // Left pane: track list
            let track_items: Vec<ListItem> = g
                .tracks
                .iter()
                .enumerate()
                .map(|(i, track)| {
                    let is_selected = i == self.track_cursor;
                    let style = if is_selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    let text = format!("  {} — {}", track.title, track.path);
                    ListItem::new(Line::from(Span::styled(text, style)))
                })
                .collect();

            let track_list = List::new(track_items)
                .block(Block::default().borders(Borders::ALL).title(" Tracks "));
            f.render_widget(track_list, content_chunks[0]);

            // Right pane: preview of album tag assignments
            let preview_items: Vec<ListItem> = g
                .tracks
                .iter()
                .map(|track| {
                    let album_val = format!("{}{}", track.title, self.suffix);
                    let text = format!("  ALBUM = \"{}\"", album_val);
                    ListItem::new(Line::from(Span::styled(
                        text,
                        Style::default().fg(Color::Cyan),
                    )))
                })
                .collect();

            let preview_list = List::new(preview_items)
                .block(Block::default().borders(Borders::ALL).title(" Preview (Enter) "));
            f.render_widget(preview_list, content_chunks[1]);
        } else {
            let empty = Paragraph::new("All groups resolved.")
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(empty, chunks[1]);
        }

        // Controls
        let controls = Paragraph::new(Line::from(vec![
            Span::styled(
                " [Enter] ",
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" \"{{title}}{}\" ", self.suffix),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                " [S] ",
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" \"Singles\"  ", Style::default().fg(Color::White)),
            Span::styled(
                " [^F] ",
                Style::default().fg(Color::Black).bg(Color::Magenta).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Suppress  ", Style::default().fg(Color::White)),
            Span::styled(
                " [Tab] ",
                Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Next  ", Style::default().fg(Color::White)),
            Span::styled(
                " [Esc] ",
                Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" Cancel ", Style::default().fg(Color::White)),
        ]))
        .block(Block::default().borders(Borders::ALL));

        f.render_widget(controls, chunks[2]);
    }
}
