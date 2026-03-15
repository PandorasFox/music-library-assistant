//! Directory Cluster Resolution V3 — StandardList + Buttons render.
//!
//! Simpler V3 pattern (no DecisionField): directories in a StandardList,
//! wizard pane shows files in the selected directory, buttons at the bottom.

use std::collections::BTreeSet;

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear},
    Frame,
};

use mm_meta::views::cluster_deploy::DirectoryClusterModalData;
use mm_ui::rich_text::{RichBlock, RichSpan};
use mm_ui::standard_list::ListEntry;
use mm_ui::wizard::{WizardItem, WizardOffer};

use crate::helpers::truncate_right;

// ============================================================================
// WizardItem wrapper
// ============================================================================

/// Display wrapper for a directory within a cluster (V3 StandardList).
pub struct ClusterDirItem {
    pub path_suffix: String,
    pub format_summary: String,
    pub file_count: usize,
    pub paths: Vec<String>,
    pub can_stash: bool,
}

impl WizardItem for ClusterDirItem {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let mut content = vec![
            RichBlock::Paragraph(vec![
                RichSpan::new(
                    &format!("{} \u{2014} {}", self.format_summary, self.file_count),
                    Style::default().fg(Color::Cyan),
                ),
            ]),
        ];

        // Show file paths
        for path in &self.paths {
            content.push(RichBlock::Paragraph(vec![RichSpan::new(
                path,
                Style::default().fg(Color::White),
            )]));
        }

        Some(WizardOffer::Pane {
            title: self.path_suffix.clone(),
            content,
        })
    }
}

impl ListEntry for ClusterDirItem {
    type Action = ();
    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<()> {
        None
    }
}

/// Build items from a cluster's directories.
pub fn build_dir_items(cluster: &mm_meta::views::cluster_deploy::DirectoryClusterEntry) -> Vec<ClusterDirItem> {
    cluster.directories.iter().map(|d| ClusterDirItem {
        path_suffix: d.path_suffix.clone(),
        format_summary: d.format_summary.clone(),
        file_count: d.paths.len(),
        paths: d.paths.clone(),
        can_stash: d.can_stash_dupes,
    }).collect()
}

// ============================================================================
// Render
// ============================================================================

/// Render the Directory Cluster V3 resolution view.
///
/// Layout: Title (3) + StandardList with wizard (min) + Buttons (3)
pub fn render_v3(
    f: &mut Frame,
    area: Rect,
    data: &DirectoryClusterModalData,
    current_cluster: usize,
    list: &mut mm_ui::standard_list::StandardListState,
    buttons: &mut mm_ui::modal_buttons::ButtonRowState<mm_ui::resolutions::directory_cluster::DirectoryClusterButton>,
    focus: mm_ui::geometry::FocusPane,
) {
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
    render_cluster_title(f, vertical[0], data, current_cluster);

    // --- StandardList ---
    let cluster = data.clusters.get(current_cluster);
    let items: Vec<ClusterDirItem> = cluster
        .map(build_dir_items)
        .unwrap_or_default();

    let list_focused = focus == mm_ui::geometry::FocusPane::List;

    let list_title = {
        let current = current_cluster + 1;
        let total = data.clusters.len();
        let key = cluster.map(|c| c.cluster_key.as_str()).unwrap_or("?");
        let overlap_count = cluster.map(|c| c.overlap_count).unwrap_or(0);
        let dir_count = items.len();
        let plural = if overlap_count == 1 { "" } else { "s" };
        format!(
            "{} ({} dirs, {} overlap{}) [cluster {}/{}] \u{2014} [Z] details",
            key, dir_count, overlap_count, plural, current, total
        )
    };

    crate::widgets::standard_list::render_standard_list(
        list,
        f,
        vertical[1],
        &items,
        |idx, is_cursor, _is_selected, width| render_dir_item(&items, idx, is_cursor, width),
        &list_title,
        list_focused,
    );

    // --- Buttons ---
    let ctx = mm_ui::resolutions::directory_cluster::DirectoryClusterButtonCtx {
        has_directories: !items.is_empty(),
        cluster_index: current_cluster,
    };
    let button_focused = focus == mm_ui::geometry::FocusPane::Buttons;
    crate::widgets::modal_buttons::render_buttons(buttons, f, vertical[2], &ctx, button_focused);
}

fn render_cluster_title(
    f: &mut Frame,
    area: Rect,
    data: &DirectoryClusterModalData,
    current_cluster: usize,
) {
    let cluster = data.clusters.get(current_cluster);
    let current = current_cluster + 1;
    let total = data.clusters.len();
    let overlap_count = cluster.map_or(0, |c| c.overlap_count);
    let dir_count = cluster.map_or(0, |c| c.directories.len());

    let title = format!(
        " Directory Overlap ({}/{}) \u{2014} {} dirs, {} overlaps ",
        current, total, dir_count, overlap_count,
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    f.render_widget(block, area);
}

fn render_dir_item(
    items: &[ClusterDirItem],
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
    // Show: path_suffix — format (N files)
    let suffix_info = format!(" \u{2014} {} ({} files)", item.format_summary, item.file_count);
    let path_max = max_width.saturating_sub(suffix_info.len() + 2);
    let path_display = truncate_right(&item.path_suffix, path_max);

    let stash_indicator = if !item.can_stash {
        Span::styled(" [no stash]", Style::default().fg(Color::DarkGray))
    } else {
        Span::raw("")
    };

    Line::from(vec![
        Span::styled(marker.to_string(), label_style),
        Span::styled(path_display.to_string(), label_style),
        Span::styled(suffix_info, Style::default().fg(Color::Cyan)),
        stash_indicator,
    ])
}
