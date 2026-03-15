//! Tag Canonicity V3 — StandardList + WizardItem render.
//!
//! Replaces the old ModalFrame list+detail layout with a StandardList
//! that uses WizardItem (Pane mode) for file details. Z-key opens a
//! scrollable right pane showing files for the selected outlier variant.

use std::collections::BTreeSet;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear};
use ratatui::Frame;

use mm_meta::views::canonicity_compound::{
    CanonicityCluster, ResolutionFileInfo, TagCanonicityResolutionData,
};
use mm_ui::geometry::FocusPane;
use mm_ui::modal_buttons::ButtonRowState;
use mm_ui::rich_text::{RichBlock, RichSpan};
use mm_ui::resolutions::tag_canonicity::{CanonicityButton, CanonicityButtonCtx};
use mm_ui::standard_list::{ListEntry, StandardListState};
use mm_ui::wizard::{WizardItem, WizardOffer};

use crate::helpers::truncate_right;
use crate::widgets::modal_buttons::render_buttons;
use crate::widgets::modal_frame::render_decision_field_widget;
use crate::widgets::standard_list::render_standard_list;

// ============================================================================
// VariantListItem — display wrapper for outlier variants
// ============================================================================

/// Display wrapper for outlier variants in the StandardList.
pub struct VariantListItem {
    pub value: String,
    pub file_count: usize,
    pub files: Vec<ResolutionFileInfo>,
}

impl WizardItem for VariantListItem {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let content: Vec<RichBlock> = self
            .files
            .iter()
            .map(|f| {
                RichBlock::Paragraph(vec![RichSpan::new(
                    &f.display_name,
                    Style::default().fg(Color::White),
                )])
            })
            .collect();
        Some(WizardOffer::Pane {
            title: format!("Files for \"{}\" ({})", self.value, self.file_count),
            content,
        })
    }
}

impl ListEntry for VariantListItem {
    type Action = ();
    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<()> {
        None
    }
}

/// Build the items vec from a cluster's variants.
pub fn build_items(cluster: &CanonicityCluster) -> Vec<VariantListItem> {
    cluster
        .variants
        .iter()
        .map(|v| VariantListItem {
            value: v.value.clone(),
            file_count: v.files.len(),
            files: v.files.clone(),
        })
        .collect()
}

// ============================================================================
// Render function
// ============================================================================

/// Render the Tag Canonicity V3 resolution view.
///
/// Layout: Title (3) + DecisionField (3) + StandardList with wizard (min) + Buttons (3)
pub fn render(
    f: &mut Frame,
    area: Rect,
    data: &TagCanonicityResolutionData,
    current_cluster: usize,
    list: &mut StandardListState,
    buttons: &mut ButtonRowState<CanonicityButton>,
    field: &mm_ui::decision_field::DecisionField,
    focus: FocusPane,
    mode: mm_ui::resolutions::tag_canonicity::CanonicityMode,
) {
    let padded = mm_ui::geometry::padded_rect(area);
    f.render_widget(Clear, padded);

    // Vertical layout: title + field + list + buttons
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Title bar
            Constraint::Length(3), // Decision field
            Constraint::Min(5),   // StandardList
            Constraint::Length(3), // Buttons
        ])
        .split(padded);

    // --- Title bar ---
    render_title_bar(f, vertical[0], data, current_cluster, mode);

    // --- Decision field ---
    render_decision_field_widget(f, vertical[1], field, focus);

    // --- StandardList ---
    let cluster = data.clusters.get(current_cluster);
    let items: Vec<VariantListItem> = cluster.map(|c| build_items(c)).unwrap_or_default();

    let list_focused = focus == FocusPane::List;

    let list_title = {
        let current = current_cluster + 1;
        let total = data.clusters.len();
        format!("{} ({}/{}) \u{2014} [Z] file details", data.tag_name, current, total)
    };

    render_standard_list(
        list,
        f,
        vertical[2],
        &items,
        |idx, is_cursor, _is_selected, width| render_variant_item(&items, idx, is_cursor, width),
        &list_title,
        list_focused,
    );

    // --- Buttons ---
    let ctx = CanonicityButtonCtx {
        has_variants: items.len() > 0,
        current_cluster_index: current_cluster,
        mode,
        tag_name: data.tag_name.clone(),
    };
    let button_focused = focus == FocusPane::Buttons;
    render_buttons(buttons, f, vertical[3], &ctx, button_focused);
}

fn render_title_bar(
    f: &mut Frame,
    area: Rect,
    data: &TagCanonicityResolutionData,
    current_cluster: usize,
    mode: mm_ui::resolutions::tag_canonicity::CanonicityMode,
) {
    use mm_ui::resolutions::tag_canonicity::CanonicityMode;

    let cluster = data.clusters.get(current_cluster);
    let current = current_cluster + 1;
    let total = data.clusters.len();
    let variant_count = cluster.map_or(0, |c| c.variants.len());

    let verb = match mode {
        CanonicityMode::InconsistentAlbumArtist => "Album Artist",
        CanonicityMode::TagCanonicity => "Squash",
    };
    let title = if let Some(ref confirmed) = cluster.and_then(|c| c.confirmed_canonical.as_ref()) {
        format!(
            " {} \"{}\" ({}/{}) \u{2014} confirmed: \"{}\" ",
            verb, data.tag_name, current, total, confirmed,
        )
    } else {
        format!(
            " {} \"{}\" ({}/{}) \u{2014} {} variants ",
            verb, data.tag_name, current, total, variant_count,
        )
    };

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    f.render_widget(block, area);
}

fn render_variant_item(
    items: &[VariantListItem],
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
    let value_max = max_width.saturating_sub(15);
    let display = truncate_right(&item.value, value_max);
    let suffix = if item.file_count == 1 {
        "file"
    } else {
        "files"
    };

    Line::from(vec![
        Span::styled(marker.to_string(), label_style),
        Span::styled(format!("\"{}\"", display), label_style),
        Span::styled(
            format!(" ({} {})", item.file_count, suffix),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}
