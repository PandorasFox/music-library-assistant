//! Intake Confirmation Modal
//!
//! Shows a dialog when unindexed files are detected, asking the user
//! to confirm indexing. This runs after eyeballing completes and before
//! transitioning to the metadata analysis phase.
//!
//! After confirmation, transitions immediately to the progress screen
//! which handles showing indexing + content analysis progress.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::ui::input::InputAction;
use crate::ui::widgets::centered_rect_fixed;

pub use mm_meta::views::startup_organize::{
    DirectoryGroup, IntakeConfirmationState, IntakeSource, UnindexedFileEntry,
};

use crate::db::types::Zone;

/// Action returned from handling input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntakeConfirmationAction {
    /// No action, continue showing modal
    None,
    /// User confirmed - queue mutations and transition to progress screen
    Confirmed,
    /// User skipped - proceed to Insights without indexing
    Skipped,
}

/// Compute total number of lines in the file list display.
fn total_list_lines(state: &IntakeConfirmationState) -> usize {
    let group_lines: usize = state
        .grouped_files
        .iter()
        .map(|g| 1 + g.filenames.len()) // 1 for directory header + files
        .sum();
    if state.multi_zone {
        // Add 2 lines per zone section header (label + blank separator)
        let zone_count = {
            let mut zones = Vec::new();
            for g in &state.grouped_files {
                if zones.last() != Some(&g.zone) {
                    zones.push(g.zone);
                }
            }
            zones.len()
        };
        group_lines + zone_count * 2
    } else {
        group_lines
    }
}

/// Handle semantic input action for intake confirmation.
pub fn handle_input(
    state: &mut IntakeConfirmationState,
    action: &InputAction,
    visible_height: usize,
) -> IntakeConfirmationAction {
    match action {
        InputAction::Confirm => IntakeConfirmationAction::Confirmed,
        InputAction::Cancel => IntakeConfirmationAction::Skipped,
        InputAction::NavUp => {
            state.scroll_offset = state.scroll_offset.saturating_sub(1);
            IntakeConfirmationAction::None
        }
        InputAction::NavDown => {
            let max_scroll = total_list_lines(state).saturating_sub(visible_height);
            if state.scroll_offset < max_scroll {
                state.scroll_offset += 1;
            }
            IntakeConfirmationAction::None
        }
        _ => IntakeConfirmationAction::None,
    }
}

/// Compute the visible height for the file list given an area.
/// Used by callers to pass to handle_input for scroll bounds.
pub fn compute_list_visible_height(area: Rect) -> usize {
    // Dialog sizing matches render()
    let dialog_height = 24.min(area.height.saturating_sub(2));
    // Subtract: border (2) + header (3) + footer (2)
    dialog_height.saturating_sub(7) as usize
}

/// Render the intake confirmation modal.
pub fn render(f: &mut Frame, area: Rect, state: &IntakeConfirmationState) {
    let dialog_width = 70.min(area.width.saturating_sub(4));
    let dialog_height = 24.min(area.height.saturating_sub(2));

    let dialog_area = centered_rect_fixed(dialog_width, dialog_height, area);
    f.render_widget(Clear, dialog_area);

    // Outer block with title
    let block = Block::default()
        .title(" Unindexed Files Detected ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let inner_area = block.inner(dialog_area);
    f.render_widget(block, dialog_area);

    // Split into header, file list, and footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(1),    // File list
            Constraint::Length(2), // Footer
        ])
        .split(inner_area);

    // Header
    let size_str = crate::ui::helpers::format_bytes(state.total_bytes);
    let header_lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{} files", state.file_count),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" to index  "),
            Span::styled(
                format!("({})", size_str),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(""),
    ];
    f.render_widget(Paragraph::new(header_lines), chunks[0]);

    // Build file list lines with directory grouping
    let mut list_lines: Vec<Line> = Vec::new();
    let mut last_zone: Option<Zone> = None;
    for group in &state.grouped_files {
        // Zone section header in multi-zone mode
        if state.multi_zone && last_zone != Some(group.zone) {
            if last_zone.is_some() {
                list_lines.push(Line::from("")); // separator between zones
            }
            let zone_label = match group.zone {
                Zone::Corpus => "--- Corpus ---",
                Zone::Inbox => "--- Inbox ---",
                _ => "---",
            };
            list_lines.push(Line::from(Span::styled(
                zone_label,
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )));
            last_zone = Some(group.zone);
        }
        // Directory header
        list_lines.push(Line::from(Span::styled(
            format!("{}/", group.display_path),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        // Files within directory
        for filename in &group.filenames {
            list_lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(filename.clone(), Style::default().fg(Color::White)),
            ]));
        }
    }

    // Apply scrolling
    let visible_height = chunks[1].height as usize;
    let total_lines = list_lines.len();
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll_offset = state.scroll_offset.min(max_scroll);

    let visible_lines: Vec<Line> = list_lines
        .into_iter()
        .skip(scroll_offset)
        .take(visible_height)
        .collect();

    // Show scroll indicator if needed
    let list_block = if total_lines > visible_height {
        Block::default()
            .title(format!(" [{}/{}] ", scroll_offset + 1, max_scroll + 1))
            .title_alignment(Alignment::Right)
    } else {
        Block::default()
    };

    f.render_widget(Paragraph::new(visible_lines).block(list_block), chunks[1]);

    // Footer
    let footer_lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("[Enter]", Style::default().fg(Color::Cyan)),
            Span::raw(" Index  "),
            Span::styled("[Esc]", Style::default().fg(Color::Cyan)),
            Span::raw(" Skip  "),
            Span::styled("[↑↓]", Style::default().fg(Color::DarkGray)),
            Span::raw(" Scroll"),
        ]),
    ];
    f.render_widget(
        Paragraph::new(footer_lines).alignment(Alignment::Center),
        chunks[2],
    );
}
