//! Directory Cluster Resolution V3 — StandardList + Buttons render.
//!
//! Simpler V3 pattern (no DecisionField): directories in a StandardList,
//! wizard pane shows files in the selected directory, buttons at the bottom.
//! Directories are radio-selectable (Space) to choose which one to stash.

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear},
    Frame,
};

use mm_ui::resolution_state::ResolutionData;
use mm_ui::resolutions::directory_cluster::{
    build_dir_items, ClusterDirItem, DirectoryClusterState,
};

use crate::helpers::truncate_right;

// ============================================================================
// Render
// ============================================================================

/// Render the Directory Cluster V3 resolution view.
///
/// Layout: Title (3) + StandardList with wizard (min) + Buttons (3)
pub fn render_v3(f: &mut Frame, area: Rect, state: &mut DirectoryClusterState) {
    let data = &state.data.inner;
    let current_cluster = state.data.current_cluster;

    let padded = mm_ui::geometry::padded_rect(area);
    f.render_widget(Clear, padded);

    let vertical = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(3), // Title bar
            ratatui::layout::Constraint::Min(5),    // StandardList
            ratatui::layout::Constraint::Length(3), // Buttons
        ])
        .split(padded);

    // --- Title bar ---
    render_cluster_title(f, vertical[0], data, current_cluster);

    // --- StandardList ---
    let cluster = data.clusters.get(current_cluster);
    let items: Vec<ClusterDirItem> = cluster.map(build_dir_items).unwrap_or_default();

    let list_focused = state.frame.focus_pane == mm_ui::geometry::FocusPane::List;

    let list_title = {
        let current = current_cluster + 1;
        let total = data.clusters.len();
        let key = cluster.map(|c| c.cluster_key.as_str()).unwrap_or("?");
        let overlap_count = cluster.map(|c| c.overlap_count).unwrap_or(0);
        let dir_count = items.len();
        let plural = if overlap_count == 1 { "" } else { "s" };
        format!(
            "{} ({} dirs, {} overlap{}) [cluster {}/{}] \u{2014} [Space] select [Z] details",
            key, dir_count, overlap_count, plural, current, total
        )
    };

    crate::widgets::standard_list::render_standard_list(
        &mut state.list,
        f,
        vertical[1],
        &items,
        |idx, is_cursor, is_selected, width| {
            render_dir_item(&items, idx, is_cursor, is_selected, width)
        },
        &list_title,
        list_focused,
    );

    // --- Buttons ---
    let ctx = state.data.button_ctx();
    let button_focused = state.frame.focus_pane == mm_ui::geometry::FocusPane::Buttons;
    crate::widgets::modal_buttons::render_buttons(
        &mut state.frame.buttons,
        f,
        vertical[2],
        &ctx,
        button_focused,
    );
}

fn render_cluster_title(
    f: &mut Frame,
    area: Rect,
    data: &mm_meta::views::cluster_deploy::DirectoryClusterModalData,
    current_cluster: usize,
) {
    let cluster = data.clusters.get(current_cluster);
    let current = current_cluster + 1;
    let total = data.clusters.len();
    let overlap_count = cluster.map_or(0, |c| c.overlap_count);
    let dir_count = cluster.map_or(0, |c| c.directories.len());

    let nav_hint = if total > 1 { " [Tab] next" } else { "" };

    let title = format!(
        " Directory Overlap ({}/{}) \u{2014} {} dirs, {} overlaps{} ",
        current, total, dir_count, overlap_count, nav_hint,
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
    is_selected: bool,
    width: u16,
) -> Line<'static> {
    let Some(item) = items.get(idx) else {
        return Line::raw("");
    };

    // Checkbox indicator: [x] selected, [ ] unselected, blank for non-stashable
    let checkbox = if !item.can_stash {
        "    "
    } else if is_selected {
        "[x] "
    } else {
        "[ ] "
    };

    let cursor_marker = if is_cursor { "\u{25b8} " } else { "  " };

    let label_style = if is_cursor {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let checkbox_style = if is_selected {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let max_width = width.saturating_sub(2) as usize;
    let suffix_info = format!(" \u{2014} {} ({} files)", item.format_summary, item.file_count);
    // cursor(2) + checkbox(4) = 6 prefix chars
    let path_max = max_width.saturating_sub(suffix_info.len() + 6);
    let path_display = truncate_right(&item.path_suffix, path_max);

    let not_stashable = if !item.can_stash {
        Span::styled(" [not stashable]", Style::default().fg(Color::DarkGray))
    } else {
        Span::raw("")
    };

    Line::from(vec![
        Span::styled(cursor_marker.to_string(), label_style),
        Span::styled(checkbox.to_string(), checkbox_style),
        Span::styled(path_display.to_string(), label_style),
        Span::styled(suffix_info, Style::default().fg(Color::Cyan)),
        not_stashable,
    ])
}
