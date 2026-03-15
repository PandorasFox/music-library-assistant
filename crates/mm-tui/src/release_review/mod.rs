//! Release Review View — browse packed releases for approval.
//!
//! Multi-select StandardList with wizard integration:
//! - List rows: checkbox + release title + coverage
//! - Wizard popup (z): release overview
//! - Wizard pane (Z): tracklist detail

pub mod render;

use std::collections::BTreeSet;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use mm_meta::views::external_matches::ReviewableRelease;
use crate::input::InputAction;
use crate::widgets::rich_text::{RichBlock, RichSpan};
use crate::widgets::standard_list::{
    ListEntry, ListInputResult, StandardListConfig, StandardListState,
};
use crate::widgets::wizard::{WizardItem, WizardOffer};

// ============================================================================
// Actions
// ============================================================================

pub(crate) enum ReleaseReviewAction {
    None,
    Cancel,
    ApproveSelected {
        selected_indices: BTreeSet<usize>,
    },
}

// ============================================================================
// List item wrapper
// ============================================================================

pub(crate) struct ReleaseReviewEntry {
    pub release: ReviewableRelease,
    pub popup_lines: Vec<Line<'static>>,
    pub pane_title: String,
    pub pane_content: Vec<RichBlock>,
}

impl WizardItem for ReleaseReviewEntry {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let has_popup = !self.popup_lines.is_empty();
        let has_pane = !self.pane_content.is_empty();
        match (has_popup, has_pane) {
            (true, true) => Some(WizardOffer::Both {
                popup: self.popup_lines.clone(),
                pane_title: self.pane_title.clone(),
                pane_content: self.pane_content.clone(),
            }),
            (true, false) => Some(WizardOffer::Popup(self.popup_lines.clone())),
            (false, true) => Some(WizardOffer::Pane {
                title: self.pane_title.clone(),
                content: self.pane_content.clone(),
            }),
            (false, false) => None,
        }
    }
}

impl ListEntry for ReleaseReviewEntry {
    type Action = ReleaseReviewAction;

    fn on_confirm(&self, selected: &BTreeSet<usize>) -> Option<ReleaseReviewAction> {
        Some(ReleaseReviewAction::ApproveSelected {
            selected_indices: selected.clone(),
        })
    }

    fn is_selectable(&self) -> bool {
        true
    }
}

// ============================================================================
// State
// ============================================================================

pub(crate) struct ReleaseReviewState {
    pub entries: Vec<ReleaseReviewEntry>,
    pub list: StandardListState,
}

impl ReleaseReviewState {
    pub fn new(releases: Vec<ReviewableRelease>) -> Self {
        let entries = build_entries(&releases);
        let mut list = StandardListState::new(StandardListConfig {
            multi_select: true,
            ..Default::default()
        });
        // Pre-select all entries by default
        for i in 0..entries.len() {
            list.selected.insert(i);
        }
        Self { entries, list }
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.entries
            .get(self.list.cursor)
            .and_then(|e| e.release.tracks.first())
            .and_then(|t| t.matched_display_name.as_deref())
    }

    pub fn handle_input(&mut self, action: &InputAction) -> ReleaseReviewAction {
        match self.list.handle_input(action, &self.entries) {
            ListInputResult::Confirm(action) => action,
            ListInputResult::Consumed
            | ListInputResult::CursorMoved
            | ListInputResult::Toggled => ReleaseReviewAction::None,
            ListInputResult::Unhandled => match action {
                InputAction::Cancel => ReleaseReviewAction::Cancel,
                _ => ReleaseReviewAction::None,
            },
        }
    }

    pub fn handle_click(&mut self, x: u16, y: u16) {
        self.list.handle_click(x, y, &self.entries);
    }
}

// ============================================================================
// Entry construction
// ============================================================================

fn build_entries(releases: &[ReviewableRelease]) -> Vec<ReleaseReviewEntry> {
    releases
        .iter()
        .map(|release| {
            let popup_lines = build_popup_lines(release);
            let pane_title = format!("Tracks: {}", release.title);
            let pane_content = build_pane_content(release);
            ReleaseReviewEntry {
                release: release.clone(),
                popup_lines,
                pane_title,
                pane_content,
            }
        })
        .collect()
}

fn build_popup_lines(release: &ReviewableRelease) -> Vec<Line<'static>> {
    let label = Style::default().fg(Color::DarkGray);
    let value = Style::default().fg(Color::White);

    let mut lines = vec![
        Line::from(Span::styled(
            "Release Overview",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(vec![
            Span::styled("Release:  ", label),
            Span::styled(release.title.clone(), value),
        ]),
        Line::from(vec![
            Span::styled("Artist:   ", label),
            Span::styled(release.artist.clone(), value),
        ]),
        Line::from(vec![
            Span::styled("MBID:     ", label),
            Span::styled(
                release.release_id.clone(),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::raw(""),
    ];

    // Coverage
    let coverage = if release.track_count > 0 {
        release.matched_count as f64 / release.track_count as f64
    } else {
        0.0
    };
    let cov_color = if coverage >= 1.0 {
        Color::Green
    } else if coverage >= 0.7 {
        Color::Yellow
    } else {
        Color::Red
    };
    lines.push(Line::from(vec![
        Span::styled("Coverage: ", label),
        Span::styled(
            format!(
                "{}/{} tracks ({:.0}%)",
                release.matched_count,
                release.track_count,
                coverage * 100.0
            ),
            Style::default().fg(cov_color),
        ),
    ]));

    // Average confidence
    lines.push(Line::from(vec![
        Span::styled("Avg Conf: ", label),
        Span::styled(
            format!("{:.1}%", release.avg_confidence * 100.0),
            Style::default().fg(Color::Yellow),
        ),
    ]));

    // Category
    lines.push(Line::from(vec![
        Span::styled("Category: ", label),
        Span::styled(release.category.clone(), value),
    ]));

    lines
}

fn build_pane_content(release: &ReviewableRelease) -> Vec<RichBlock> {
    let label = Style::default().fg(Color::DarkGray);
    let value = Style::default().fg(Color::White);

    let mut blocks = Vec::new();

    blocks.push(RichBlock::Heading("Tracklist".to_string()));
    blocks.push(RichBlock::Blank);

    for track in &release.tracks {
        let pos_str = format!("{:>3}. ", track.position);

        if let Some(ref display_name) = track.matched_display_name {
            // Matched track
            let conf_str = track
                .confidence
                .map(|c| format!(" ({:.0}%)", c * 100.0))
                .unwrap_or_default();

            blocks.push(RichBlock::Paragraph(vec![
                RichSpan::new(pos_str, label),
                RichSpan::new(
                    track.mb_title.clone(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                RichSpan::new(conf_str, Style::default().fg(Color::Yellow)),
            ]));

            let filename = display_name.rsplit('/').next().unwrap_or(display_name);
            blocks.push(RichBlock::Paragraph(vec![
                RichSpan::new("      ", label),
                RichSpan::new(format!("\u{2192} {}", filename), label),
            ]));
        } else {
            // Unfilled slot
            blocks.push(RichBlock::Paragraph(vec![
                RichSpan::new(pos_str, label),
                RichSpan::new(
                    track.mb_title.clone(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
                RichSpan::new(" (unmatched)", Style::default().fg(Color::Red)),
            ]));
        }
    }

    blocks.push(RichBlock::Blank);

    // Artist info
    if !release.artist.is_empty() {
        blocks.push(RichBlock::Paragraph(vec![
            RichSpan::new("Artist: ", label),
            RichSpan::new(release.artist.clone(), value),
        ]));
    }

    blocks
}
