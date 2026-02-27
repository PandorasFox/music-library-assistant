//! Missing Album Singles Resolution Modal
//!
//! Shows tracks that lack an ALBUM tag but have ARTIST and TITLE,
//! grouped by artist. Offers resolution options via selectable buttons:
//!
//! ## Controls
//!
//! - Shift+Up/Down: Cycle focus between track list and resolution buttons
//! - Up/Down/j/k: Navigate track list (List focus)
//! - Left/Right/h/l: Cycle resolution option (Buttons focus)
//! - Enter: Confirm selected resolution (Buttons focus)
//! - Tab/Shift-Tab: Navigate between artist groups
//! - t: Edit selected track in tag editor
//! - T (Shift+T): Bulk-edit all tracks in group in tag editor
//! - Ctrl+R: Show transaction review
//! - Escape: Cancel

use crate::ui::input::InputAction;
use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use crate::meta::signals::data::MissingAlbumSingleSignal;
use crate::ui::helpers::render_pane;
use crate::ui::widgets::{
    control_colors as cc, render_file_path_list, ConfirmationButton, FocusPane, PathEntry,
    ResolutionLayout,
};

// ============================================================================
// Resolution Option
// ============================================================================

/// Resolution option for a group of tracks missing ALBUM tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumResolution {
    /// ALBUM = "{title}{suffix}" per track
    PerTrackTitle,
    /// ALBUM = "Singles" for all tracks
    AllSingles,
    /// Suppress: emit ExpectedMissingTag
    Suppress,
}

impl AlbumResolution {
    fn next(self) -> Self {
        match self {
            Self::PerTrackTitle => Self::AllSingles,
            Self::AllSingles => Self::Suppress,
            Self::Suppress => Self::PerTrackTitle,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::PerTrackTitle => Self::Suppress,
            Self::AllSingles => Self::PerTrackTitle,
            Self::Suppress => Self::AllSingles,
        }
    }
}

// ============================================================================
// Action Enum
// ============================================================================

/// Actions returned from the missing album modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingAlbumAction {
    /// No action needed.
    None,
    /// Cancel and return to Insights view.
    Cancel,
    /// Confirm the selected resolution for the current group.
    Confirm(AlbumResolution),
    /// Navigate to next/prev artist group.
    NavigateGroup(bool),
    /// Show transaction review.
    ShowReview,
    /// Open tag editor for all tracks in group (individual mode, Tab navigates).
    EditTracks,
    /// Open tag editor for all tracks in group (aggregated mode, unified view).
    EditTracksAggregated,
}

// ============================================================================
// Data Types
// ============================================================================

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
                tracks: s
                    .data
                    .tracks
                    .into_iter()
                    .map(|t| TrackEntry {
                        inode: t.inode,
                        title: t.title,
                        path: t.path,
                    })
                    .collect(),
            })
            .collect();
        Self { groups }
    }
}

// ============================================================================
// State
// ============================================================================

/// State for the missing album singles resolution modal.
#[derive(Debug)]
pub struct MissingAlbumState {
    /// Loaded data (immutable — groups are never removed).
    pub data: MissingAlbumData,
    /// Current artist group index.
    pub current_group: usize,
    /// Track cursor within current group.
    pub track_cursor: usize,
    /// Track scroll offset within current group.
    pub track_scroll: usize,
    /// Suffix from config opinion (e.g., " (Single)").
    pub suffix: String,
    /// Current focus pane (List or Buttons).
    pub focus_pane: FocusPane,
    /// Currently selected resolution option.
    pub selected_resolution: AlbumResolution,
}

impl MissingAlbumState {
    pub fn new(data: MissingAlbumData, suffix: String) -> Self {
        Self {
            data,
            current_group: 0,
            track_cursor: 0,
            track_scroll: 0,
            suffix,
            focus_pane: FocusPane::List,
            selected_resolution: AlbumResolution::PerTrackTitle,
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

    /// All inodes in the current group (for tag editor).
    pub fn current_group_inodes(&self) -> Vec<i64> {
        self.current_group_data()
            .map(|g| g.tracks.iter().map(|t| t.inode).collect())
            .unwrap_or_default()
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> MissingAlbumAction {
        // FocusUp/FocusDown: cycle focus pane
        match action {
            InputAction::FocusUp => {
                self.focus_pane = self.focus_pane.prev();
                return MissingAlbumAction::None;
            }
            InputAction::FocusDown => {
                self.focus_pane = self.focus_pane.next();
                return MissingAlbumAction::None;
            }
            _ => {}
        }

        // Ctrl+R: show transaction review
        if matches!(action, InputAction::Shortcut('r')) {
            return MissingAlbumAction::ShowReview;
        }

        match action {
            InputAction::Cancel => MissingAlbumAction::Cancel,

            // Group navigation
            InputAction::CycleNext => MissingAlbumAction::NavigateGroup(true),
            InputAction::CyclePrev => MissingAlbumAction::NavigateGroup(false),

            // Track list navigation (List focus)
            InputAction::NavUp if self.focus_pane == FocusPane::List => {
                if self.track_cursor > 0 {
                    self.track_cursor -= 1;
                }
                MissingAlbumAction::None
            }
            InputAction::NavDown if self.focus_pane == FocusPane::List => {
                if let Some(group) = self.current_group_data() {
                    if self.track_cursor + 1 < group.tracks.len() {
                        self.track_cursor += 1;
                    }
                }
                MissingAlbumAction::None
            }

            // Resolution button cycling (Buttons focus)
            InputAction::NavLeft if self.focus_pane == FocusPane::Buttons => {
                self.selected_resolution = self.selected_resolution.prev();
                MissingAlbumAction::None
            }
            InputAction::NavRight if self.focus_pane == FocusPane::Buttons => {
                self.selected_resolution = self.selected_resolution.next();
                MissingAlbumAction::None
            }

            // Confirm resolution (Buttons focus)
            InputAction::Confirm if self.focus_pane == FocusPane::Buttons => {
                if self.current_group_data().is_some() {
                    MissingAlbumAction::Confirm(self.selected_resolution)
                } else {
                    MissingAlbumAction::None
                }
            }

            // Tag editor shortcuts (available regardless of focus)
            InputAction::Char('t') => MissingAlbumAction::EditTracks,
            InputAction::Char('T') => MissingAlbumAction::EditTracksAggregated,

            _ => MissingAlbumAction::None,
        }
    }

    /// Render the modal.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        let padded = ResolutionLayout::padded(area);
        f.render_widget(Clear, padded);

        let layout = ResolutionLayout::new(padded, 3, 3, 50);

        let group = self.current_group_data();
        let total_groups = self.data.groups.len();

        // Info bar: artist name, group counter
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
        render_pane(f, layout.info_bar, header_block);

        // Content panes
        if let Some(g) = group {
            self.render_track_list(f, layout.list_pane, g);
            self.render_preview(f, layout.details_pane, g);
        } else {
            let empty = Paragraph::new("All groups resolved.")
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(empty, layout.list_pane);
        }

        // Buttons + hint bar
        self.render_buttons(f, layout.buttons);
    }

    fn render_track_list(&self, f: &mut Frame, area: Rect, group: &ArtistGroup) {
        let is_focused = self.focus_pane == FocusPane::List;
        let border_color = if is_focused {
            Color::Yellow
        } else {
            Color::DarkGray
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(" Tracks ")
            .border_style(Style::default().fg(border_color));
        let inner = render_pane(f, area, block);

        let entries: Vec<PathEntry> = group
            .tracks
            .iter()
            .map(|track| PathEntry {
                path: &track.path,
                prefix: vec![Span::styled(
                    format!("{} — ", track.title),
                    Style::default().fg(Color::White),
                )],
                suffix: vec![],
            })
            .collect();

        render_file_path_list(f, inner, &entries, self.track_cursor, self.track_scroll);
    }

    fn render_preview(&self, f: &mut Frame, area: Rect, group: &ArtistGroup) {
        let title = match self.selected_resolution {
            AlbumResolution::PerTrackTitle => " Preview: Per-Track Title ",
            AlbumResolution::AllSingles => " Preview: \"Singles\" ",
            AlbumResolution::Suppress => " Preview: Suppress ",
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = render_pane(f, area, block);

        let lines: Vec<ListItem> = match self.selected_resolution {
            AlbumResolution::PerTrackTitle => group
                .tracks
                .iter()
                .map(|track| {
                    let album_val = format!("{}{}", track.title, self.suffix);
                    ListItem::new(Line::from(Span::styled(
                        format!(" ALBUM = \"{}\"", album_val),
                        Style::default().fg(Color::Cyan),
                    )))
                })
                .collect(),
            AlbumResolution::AllSingles => group
                .tracks
                .iter()
                .map(|_| {
                    ListItem::new(Line::from(Span::styled(
                        " ALBUM = \"Singles\"".to_string(),
                        Style::default().fg(Color::Green),
                    )))
                })
                .collect(),
            AlbumResolution::Suppress => {
                vec![ListItem::new(Line::from(Span::styled(
                    " Signal will be suppressed (no tag changes)",
                    Style::default().fg(Color::DarkGray),
                )))]
            }
        };

        let list = List::new(lines);
        f.render_widget(list, inner);
    }

    fn render_buttons(&self, f: &mut Frame, area: Rect) {
        let is_focused = self.focus_pane == FocusPane::Buttons;
        let border_color = if is_focused {
            Color::Yellow
        } else {
            Color::DarkGray
        };

        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(border_color));
        let inner = render_pane(f, area, block);

        if inner.height == 0 {
            return;
        }

        // Resolution buttons (top line of inner area)
        let button_area = Rect {
            height: 1,
            ..inner
        };

        let per_track_label = format!("\"{{title}}{}\"", self.suffix);
        let buttons = [
            ConfirmationButton::new(&per_track_label, Color::Cyan)
                .selected(is_focused && self.selected_resolution == AlbumResolution::PerTrackTitle),
            ConfirmationButton::new("\"Singles\"", Color::Green)
                .selected(is_focused && self.selected_resolution == AlbumResolution::AllSingles),
            ConfirmationButton::new("Suppress", Color::Yellow)
                .selected(is_focused && self.selected_resolution == AlbumResolution::Suppress),
        ];

        crate::ui::widgets::render_button_row(f, button_area, &buttons);

        // Hint line (bottom line of inner area)
        if inner.height >= 2 {
            let hint_area = Rect {
                y: inner.y + inner.height - 1,
                height: 1,
                ..inner
            };

            let hints = Line::from(vec![
                cc::nav("Shift+↑↓"),
                cc::text(" focus  "),
                cc::nav("←→"),
                cc::text(" option  "),
                cc::confirm("[Enter]"),
                cc::text(" confirm  "),
                cc::nav("[Tab]"),
                cc::text(" group  "),
                cc::edit("[t]"),
                cc::text(" edit  "),
                cc::edit("[T]"),
                cc::text(" aggregate  "),
                cc::review("[^R]"),
                cc::text(" review  "),
                cc::cancel("[Esc]"),
                cc::text(" cancel"),
            ]);

            let hint_para = Paragraph::new(hints).alignment(Alignment::Center);
            f.render_widget(hint_para, hint_area);
        }
    }
}
