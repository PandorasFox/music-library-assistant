//! Disc Extraction Resolution Modal (V3)
//!
//! Single-load layout with StandardList + ButtonRow.

use std::collections::BTreeSet;

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear},
    Frame,
};
use mm_meta::domain_queries::{DiscExtractionGroup, DiscExtractionModalData};
use mm_ui::resolution_state::ResolutionData;
use mm_ui::resolutions::disc_extraction::DiscExtractionState;
use mm_ui::rich_text::{RichBlock, RichSpan};
use mm_ui::standard_list::ListEntry;
use mm_ui::wizard::{WizardItem, WizardOffer};
use crate::helpers::truncate_right;

// ============================================================================
// WizardItem wrapper for StandardList
// ============================================================================

/// Display wrapper for files in a Disc Extraction group (StandardList).
pub struct DiscFileListItem {
    pub path: String,
    pub source_tag: String,
    pub original_value: String,
    pub cleaned_value: String,
    pub inode: i64,
}

impl WizardItem for DiscFileListItem {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let content = vec![
            RichBlock::Paragraph(vec![RichSpan::new(
                &format!("{}: \"{}\" \u{2192} \"{}\"", self.source_tag, self.original_value, self.cleaned_value),
                Style::default().fg(Color::Cyan),
            )]),
            RichBlock::Paragraph(vec![RichSpan::new(
                &self.path,
                Style::default().fg(Color::White),
            )]),
        ];
        Some(WizardOffer::Pane {
            title: format!("File: {}", self.source_tag),
            content,
        })
    }
}

impl ListEntry for DiscFileListItem {
    type Action = ();
    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<()> {
        None
    }
}

/// Build items vec from a disc extraction group's files.
pub fn build_file_items(group: &DiscExtractionGroup) -> Vec<DiscFileListItem> {
    group.files.iter().map(|f| DiscFileListItem {
        path: f.path.clone(),
        source_tag: f.source_tag.clone(),
        original_value: f.original_value.clone(),
        cleaned_value: f.cleaned_value.clone(),
        inode: f.inode,
    }).collect()
}

// ============================================================================
// Render function
// ============================================================================

/// Render the Disc Extraction resolution view.
///
/// Layout: Title (3) + StandardList with wizard (min) + Buttons (3)
pub fn render_v3(
    f: &mut Frame,
    area: Rect,
    state: &mut DiscExtractionState,
) {
    let data = &state.data.inner;
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
    render_disc_extraction_title(f, vertical[0], data, current_group, &state.data.disc_tag_name);

    // --- StandardList ---
    let group = data.groups.get(current_group);
    let items: Vec<DiscFileListItem> = group
        .map(build_file_items)
        .unwrap_or_default();

    let list_focused = state.frame.focus_pane == mm_ui::geometry::FocusPane::List;

    let list_title = {
        let current = current_group + 1;
        let total = data.groups.len();
        let file_count = items.len();
        let desc = group.map(|g| g.description.as_str()).unwrap_or("?");
        format!("{} ({} files) [group {}/{}] \u{2014} [Z] details", desc, file_count, current, total)
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

fn render_disc_extraction_title(
    f: &mut Frame,
    area: Rect,
    data: &DiscExtractionModalData,
    current_group: usize,
    disc_tag_name: &str,
) {
    let group = data.groups.get(current_group);
    let current = current_group + 1;
    let total = data.groups.len();
    let file_count = group.map_or(0, |g| g.files.len());
    let disc_value = group.map(|g| g.disc_value.as_str()).unwrap_or("?");

    let title = format!(
        " Disc Extraction ({}/{}) \u{2014} {} files, {} = \"{}\" ",
        current, total, file_count, disc_tag_name, disc_value,
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    f.render_widget(block, area);
}

fn render_file_item(
    items: &[DiscFileListItem],
    idx: usize,
    is_cursor: bool,
    width: u16,
) -> Line<'static> {
    let Some(item) = items.get(idx) else {
        return Line::raw("");
    };

    let marker = if is_cursor { "\u{25b8} " } else { "  " };
    let label_style = if is_cursor {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let max_width = width.saturating_sub(2) as usize;
    let transform = format!("{}: \"{}\" \u{2192} \"{}\"", item.source_tag, item.original_value, item.cleaned_value);
    let transform_max = max_width.saturating_sub(5);
    let display = truncate_right(&transform, transform_max);

    Line::from(vec![
        Span::styled(marker.to_string(), label_style),
        Span::styled(display.to_string(), label_style),
        Span::styled(
            format!(" \u{2014} {}", truncate_right(&item.path, max_width.saturating_sub(transform_max + 5))),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}
