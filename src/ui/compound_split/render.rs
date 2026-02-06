//! Rendering for compound tag split modal.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use super::CompoundSplitState;
use crate::ui::widgets::centered_rect_fixed;

/// Render the compound split modal.
pub fn render(f: &mut Frame, area: Rect, state: &CompoundSplitState) {
    // Modal dimensions - slightly taller to accommodate matching_parts info
    let modal_width = 60.min(area.width.saturating_sub(4));
    let modal_height = 22.min(area.height.saturating_sub(2));
    let modal_area = centered_rect_fixed(modal_width, modal_height, area);

    // Clear background
    f.render_widget(Clear, modal_area);

    // Title with progress indicator
    let title = format!(
        " Compound Tag ({}/{}) ",
        state.group_index + 1,
        state.total_groups
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);

    // Layout: info + matching parts (for artist) + split parts + controls
    let has_matching_info = state.data.tag_name.to_lowercase() == "artist";
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),                          // Tag info
            Constraint::Length(if has_matching_info { 3 } else { 0 }), // Matching parts
            Constraint::Min(3),                             // Split parts list
            Constraint::Length(2),                          // Controls hint
        ])
        .split(inner);

    // Render tag info section
    render_tag_info(f, chunks[0], state);

    // Render matching parts info (for artist tag only)
    if has_matching_info {
        render_matching_info(f, chunks[1], state);
    }

    // Render split parts list
    render_split_parts(f, chunks[2], state);

    // Render controls hint
    render_controls(f, chunks[3], state);
}

fn render_tag_info(f: &mut Frame, area: Rect, state: &CompoundSplitState) {
    let lines = vec![
        Line::from(vec![
            Span::styled("Tag: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&state.data.tag_name, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled("Original: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("\"{}\"", &state.data.compound_value),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(vec![
            Span::styled("Affects: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(
                    "{} track{}",
                    state.data.inodes.len(),
                    if state.data.inodes.len() == 1 { "" } else { "s" }
                ),
                Style::default().fg(Color::White),
            ),
        ]),
    ];

    let para = Paragraph::new(lines);
    f.render_widget(para, area);
}

fn render_matching_info(f: &mut Frame, area: Rect, state: &CompoundSplitState) {
    let (message, message_color, recommendation) = if state.data.suggests_split() {
        // Parts exist as standalone artists - likely a collaboration
        let matched = state.data.matching_parts.join(", ");
        (
            format!("Known artists: {}", matched),
            Color::Green,
            "Likely a collaboration - consider splitting",
        )
    } else if state.data.suggests_canonicalize() {
        // No parts exist standalone - likely a band name
        (
            "No matching standalone artists".to_string(),
            Color::Yellow,
            "Likely a band name - consider keeping as-is",
        )
    } else {
        // Non-artist tag or no matching info
        return;
    };

    let lines = vec![
        Line::from(vec![
            Span::styled(message, Style::default().fg(message_color)),
        ]),
        Line::from(vec![
            Span::styled(recommendation, Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
        ]),
    ];

    let para = Paragraph::new(lines);
    f.render_widget(para, area);
}

fn render_split_parts(f: &mut Frame, area: Rect, state: &CompoundSplitState) {
    // Header
    let header_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    let header = Paragraph::new("Will split into:")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(header, header_area);

    // List area
    let list_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height.saturating_sub(1),
    };

    let items: Vec<ListItem> = state
        .data
        .split_parts
        .iter()
        .enumerate()
        .map(|(idx, part)| {
            let is_selected = idx == state.cursor;
            let prefix = if is_selected { "> " } else { "  " };

            let style = if is_selected {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(format!("{}{}", prefix, part)).style(style)
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, list_area);
}

fn render_controls(f: &mut Frame, area: Rect, state: &CompoundSplitState) {
    // Show contextual hints based on whether this is an artist tag
    let is_artist = state.data.tag_name.to_lowercase() == "artist";

    let mut hints = vec![
        Span::styled("[Enter]", Style::default().fg(Color::Green)),
        Span::raw(" Split  "),
    ];

    // Show canonicalize option for artist tags
    if is_artist {
        hints.push(Span::styled("[C]", Style::default().fg(Color::Yellow)));
        hints.push(Span::raw(" Keep as-is  "));
    }

    hints.extend([
        Span::styled("[Tab]", Style::default().fg(Color::Cyan)),
        Span::raw(" Next  "),
        Span::styled("[^A]", Style::default().fg(Color::Magenta)),
        Span::raw(" All  "),
        Span::styled("[^R]", Style::default().fg(Color::Blue)),
        Span::raw(" Review  "),
        Span::styled("[Esc]", Style::default().fg(Color::Red)),
        Span::raw(" Cancel"),
    ]);

    let para = Paragraph::new(Line::from(hints)).alignment(Alignment::Center);
    f.render_widget(para, area);
}
