//! Manual Review V3 render — StandardList + Buttons pattern.
//!
//! Layout: Title (3) + StandardList with wizard pane (min) + Buttons (3)
//! The wizard pane shows file metadata (format, duration, bitrate, tags) on Z.

use std::collections::BTreeSet;

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear},
    Frame,
};

use mm_meta::views::review_match::{ManualReviewData, ReviewFileEntry, ReviewKind};
use mm_ui::resolution_state::ResolutionData;
use mm_ui::resolutions::manual_review::ManualReviewState;
use mm_ui::rich_text::{RichBlock, RichSpan};
use mm_ui::standard_list::ListEntry;
use mm_ui::wizard::{WizardItem, WizardOffer};

use crate::helpers::{
    format_bytes, format_duration_ms, format_kbps, format_sample_rate, truncate_left,
    truncate_right,
};

// ============================================================================
// WizardItem wrapper
// ============================================================================

/// Display wrapper for a file entry in a manual review group (V3 StandardList).
pub struct ReviewFileItem {
    pub corpus_path: String,
    pub inode: i64,
    pub context: String,
    pub stashed: bool,
    pub file_type: String,
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub file_size: i64,
    pub has_pictures: bool,
    pub tags: Vec<(String, String)>,
}

impl ReviewFileItem {
    pub fn from_entry(entry: &ReviewFileEntry) -> Self {
        let (file_type, duration_ms, bitrate_kbps, sample_rate, file_size, has_pictures, tags) =
            if let Some(ref meta) = entry.meta {
                (
                    meta.file_type.clone(),
                    meta.duration_ms,
                    meta.bitrate_kbps,
                    meta.sample_rate,
                    meta.file_size,
                    meta.has_pictures,
                    meta.tags.clone(),
                )
            } else {
                (String::new(), None, None, None, 0, false, Vec::new())
            };
        Self {
            corpus_path: entry.corpus_path.clone(),
            inode: entry.inode,
            context: entry.context.clone(),
            stashed: entry.stashed,
            file_type,
            duration_ms,
            bitrate_kbps,
            sample_rate,
            file_size,
            has_pictures,
            tags,
        }
    }
}

impl WizardItem for ReviewFileItem {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let mut content = Vec::new();

        // Path
        content.push(RichBlock::Paragraph(vec![RichSpan::new(
            &self.corpus_path,
            Style::default().fg(Color::White),
        )]));

        // Context line
        if !self.context.is_empty() {
            content.push(RichBlock::Paragraph(vec![
                RichSpan::new("Context: ", Style::default().fg(Color::DarkGray)),
                RichSpan::new(&self.context, Style::default().fg(Color::Yellow)),
            ]));
        }

        // Audio metadata
        if !self.file_type.is_empty() {
            content.push(RichBlock::Paragraph(vec![
                RichSpan::new("Format: ", Style::default().fg(Color::DarkGray)),
                RichSpan::new(
                    &self.file_type.to_uppercase(),
                    Style::default().fg(Color::White),
                ),
            ]));
        }

        if let Some(dur) = self.duration_ms {
            content.push(RichBlock::Paragraph(vec![
                RichSpan::new("Duration: ", Style::default().fg(Color::DarkGray)),
                RichSpan::new(
                    &format_duration_ms(dur),
                    Style::default().fg(Color::White),
                ),
            ]));
        }

        if let Some(br) = self.bitrate_kbps {
            content.push(RichBlock::Paragraph(vec![
                RichSpan::new("Bitrate: ", Style::default().fg(Color::DarkGray)),
                RichSpan::new(&format_kbps(br), Style::default().fg(Color::White)),
            ]));
        }

        if let Some(sr) = self.sample_rate {
            content.push(RichBlock::Paragraph(vec![
                RichSpan::new("Sample rate: ", Style::default().fg(Color::DarkGray)),
                RichSpan::new(&format_sample_rate(sr), Style::default().fg(Color::White)),
            ]));
        }

        if self.file_size > 0 {
            content.push(RichBlock::Paragraph(vec![
                RichSpan::new("Size: ", Style::default().fg(Color::DarkGray)),
                RichSpan::new(
                    &format_bytes(self.file_size as u64),
                    Style::default().fg(Color::White),
                ),
            ]));
        }

        content.push(RichBlock::Paragraph(vec![RichSpan::new(
            &format!(
                "Album art: {}",
                if self.has_pictures { "Yes" } else { "No" }
            ),
            Style::default().fg(Color::DarkGray),
        )]));

        // Tags
        if !self.tags.is_empty() {
            content.push(RichBlock::Paragraph(vec![RichSpan::new(
                "Tags:",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )]));
            for (name, value) in &self.tags {
                content.push(RichBlock::Paragraph(vec![
                    RichSpan::new(&format!("  {}: ", name), Style::default().fg(Color::DarkGray)),
                    RichSpan::new(value, Style::default().fg(Color::White)),
                ]));
            }
        }

        if self.stashed {
            content.push(RichBlock::Paragraph(vec![RichSpan::new(
                "STASHED \u{2014} staged for removal",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            )]));
        }

        Some(WizardOffer::Pane {
            title: truncate_left(&self.corpus_path, 60),
            content,
        })
    }
}

impl ListEntry for ReviewFileItem {
    type Action = ();
    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<()> {
        None
    }
}

/// Build items vec from a review group's files.
pub fn build_review_items(group: &mm_meta::views::review_match::ReviewGroup) -> Vec<ReviewFileItem> {
    group
        .files
        .iter()
        .map(ReviewFileItem::from_entry)
        .collect()
}

// ============================================================================
// Render
// ============================================================================

/// Render the Manual Review V3 resolution view.
///
/// Layout: Title (3) + StandardList with wizard (min) + Buttons (3)
pub fn render_v3(
    f: &mut Frame,
    area: Rect,
    state: &mut ManualReviewState,
) {
    let data = &state.data.inner;
    let review_kind = state.data.review_kind;
    let current_group = state.data.current_group;

    let padded = mm_ui::geometry::padded_rect(area);
    f.render_widget(Clear, padded);

    let vertical = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(3), // Title bar
            ratatui::layout::Constraint::Min(5),    // StandardList
            ratatui::layout::Constraint::Length(3),  // Buttons
        ])
        .split(padded);

    // --- Title bar ---
    render_title(f, vertical[0], data, review_kind, current_group);

    // --- StandardList ---
    let group = data.groups.get(current_group);
    let items: Vec<ReviewFileItem> = group.map(build_review_items).unwrap_or_default();

    let list_focused = state.frame.focus_pane == mm_ui::geometry::FocusPane::List;

    let list_title = {
        let current = current_group + 1;
        let total = data.groups.len();
        let label = group.map(|g| g.label.as_str()).unwrap_or("?");
        let file_count = items.len();
        format!(
            "{} ({} files) [group {}/{}] \u{2014} [Z] details",
            truncate_right(label, 40),
            file_count,
            current,
            total
        )
    };

    crate::widgets::standard_list::render_standard_list(
        &mut state.list,
        f,
        vertical[1],
        &items,
        |idx, is_cursor, _is_selected, width| render_file_item(&items, idx, is_cursor, width),
        &list_title,
        list_focused,
    );

    // --- Buttons ---
    let ctx = state.data.button_ctx();
    let button_focused = state.frame.focus_pane == mm_ui::geometry::FocusPane::Buttons;
    crate::widgets::modal_buttons::render_buttons(&mut state.frame.buttons, f, vertical[2], &ctx, button_focused);
}

fn render_title(
    f: &mut Frame,
    area: Rect,
    data: &ManualReviewData,
    review_kind: ReviewKind,
    current_group: usize,
) {
    let group = data.groups.get(current_group);
    let current = current_group + 1;
    let total = data.groups.len();
    let file_count = group.map_or(0, |g| g.files.len());
    let label = group
        .map(|g| g.label.as_str())
        .unwrap_or("?");

    let title = format!(
        " {}: \"{}\" ({}/{}) \u{2014} {} files ",
        review_kind.title(),
        truncate_right(label, 30),
        current,
        total,
        file_count,
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    f.render_widget(block, area);
}

fn render_file_item(
    items: &[ReviewFileItem],
    idx: usize,
    is_cursor: bool,
    width: u16,
) -> Line<'static> {
    let Some(item) = items.get(idx) else {
        return Line::raw("");
    };

    let marker = if item.stashed {
        "\u{2717} "
    } else if is_cursor {
        "\u{25b8} "
    } else {
        "  "
    };

    let label_style = if item.stashed {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::CROSSED_OUT)
    } else if is_cursor {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let max_width = width.saturating_sub(2) as usize;
    let path_display = truncate_left(&item.corpus_path, max_width.saturating_sub(2));

    Line::from(vec![
        Span::styled(marker.to_string(), label_style),
        Span::styled(path_display, label_style),
    ])
}
