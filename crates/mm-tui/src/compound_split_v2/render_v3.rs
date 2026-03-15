//! Compound Split V3 -- StandardList + WizardItem + DecisionField render.
//!
//! Replaces the old three-pane layout with a StandardList that uses
//! WizardItem (Pane mode) for file details. DecisionField shows the
//! pre-filled split parts for the current group.

use std::collections::BTreeSet;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear};
use ratatui::Frame;

use mm_meta::views::canonicity_compound::{CompoundSplitCluster, CompoundSplitResolutionData};
use mm_ui::geometry::FocusPane;
use mm_ui::modal_buttons::ButtonRowState;
use mm_ui::rich_text::{RichBlock, RichSpan};
use mm_ui::resolutions::compound_split::{CompoundSplitButton, CompoundSplitButtonCtx};
use mm_ui::standard_list::{ListEntry, StandardListState};
use mm_ui::wizard::{WizardItem, WizardOffer};

use crate::helpers::truncate_right;
use crate::widgets::modal_buttons::render_buttons;
use crate::widgets::modal_frame::render_decision_field_widget;
use crate::widgets::standard_list::render_standard_list;

// ============================================================================
// CompoundFileItem -- display wrapper for files in the StandardList
// ============================================================================

/// Display wrapper for files affected by a compound split.
pub struct CompoundFileItem {
    pub display_name: String,
    pub inode: i64,
}

impl WizardItem for CompoundFileItem {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let content = vec![RichBlock::Paragraph(vec![RichSpan::new(
            &self.display_name,
            Style::default().fg(Color::White),
        )])];
        Some(WizardOffer::Pane {
            title: format!("File: {}", self.display_name),
            content,
        })
    }
}

impl ListEntry for CompoundFileItem {
    type Action = ();
    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<()> {
        None
    }
}

/// Build the items vec from a group's files.
pub fn build_items(group: &CompoundSplitCluster) -> Vec<CompoundFileItem> {
    group
        .files
        .iter()
        .map(|f| CompoundFileItem {
            display_name: f.display_name.clone(),
            inode: f.inode,
        })
        .collect()
}

// ============================================================================
// Render function
// ============================================================================

/// Render the Compound Split V3 resolution view.
///
/// Layout: Title (3) + DecisionField (3) + StandardList with wizard (min) + Buttons (3)
pub fn render(
    f: &mut Frame,
    area: Rect,
    data: &CompoundSplitResolutionData,
    current_group: usize,
    list: &mut StandardListState,
    buttons: &mut ButtonRowState<CompoundSplitButton>,
    field: &mm_ui::decision_field::DecisionField,
    focus: FocusPane,
    safe_mode: bool,
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
    render_title_bar(f, vertical[0], data, current_group, safe_mode);

    // --- Decision field ---
    render_decision_field_widget(f, vertical[1], field, focus);

    // --- StandardList ---
    let group = data.groups.get(current_group);
    let items: Vec<CompoundFileItem> = group.map(|g| build_items(g)).unwrap_or_default();

    let list_focused = focus == FocusPane::List;

    let list_title = {
        let current = current_group + 1;
        let total = data.groups.len();
        let compound = group.map(|g| g.compound_value.as_str()).unwrap_or("?");
        format!("{} ({}/{}) \u{2014} [Z] file details", compound, current, total)
    };

    render_standard_list(
        list,
        f,
        vertical[2],
        &items,
        |idx, is_cursor, _is_selected, width| render_file_item(&items, idx, is_cursor, width),
        &list_title,
        list_focused,
    );

    // --- Buttons ---
    let ctx = CompoundSplitButtonCtx {
        has_files: !items.is_empty(),
        current_group_index: current_group,
    };
    let button_focused = focus == FocusPane::Buttons;
    render_buttons(buttons, f, vertical[3], &ctx, button_focused);
}

fn render_title_bar(
    f: &mut Frame,
    area: Rect,
    data: &CompoundSplitResolutionData,
    current_group: usize,
    safe_mode: bool,
) {
    let group = data.groups.get(current_group);
    let current = current_group + 1;
    let total = data.groups.len();
    let file_count = group.map_or(0, |g| g.files.len());
    let mode_label = if safe_mode { "Safe" } else { "Review" };

    let tag_name = group.map(|g| g.tag_name.as_str()).unwrap_or("?");
    let title = format!(
        " Split \"{}\" ({}/{}) \u{2014} {} files \u{2014} {} ",
        tag_name, current, total, file_count, mode_label,
    );

    let border_color = if safe_mode { Color::Green } else { Color::Yellow };

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    f.render_widget(block, area);
}

fn render_file_item(
    items: &[CompoundFileItem],
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
    let display = truncate_right(&item.display_name, max_width);

    Line::from(vec![
        Span::styled(marker.to_string(), label_style),
        Span::styled(display, label_style),
    ])
}
