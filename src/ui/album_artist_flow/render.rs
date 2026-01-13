//! Album Artist Flow Rendering

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::phase_selector::PhaseSelectorState;
use super::types::AlbumArtistPhase;

/// Render the phase selector popup.
pub fn render_phase_selector(frame: &mut Frame, state: &PhaseSelectorState) {
    let area = frame.area();

    // Center the popup (50x14)
    let popup_width = 50u16;
    let popup_height = 14u16;
    let popup_x = area.width.saturating_sub(popup_width) / 2;
    let popup_y = area.height.saturating_sub(popup_height) / 2;

    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the popup area
    frame.render_widget(Clear, popup_area);

    // Draw popup border
    let block = Block::default()
        .title(" Album Artist Resolution ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    frame.render_widget(block, popup_area);

    // Inner area for content
    let inner = Rect::new(
        popup_area.x + 2,
        popup_area.y + 1,
        popup_area.width.saturating_sub(4),
        popup_area.height.saturating_sub(2),
    );

    // Split into header, phases, and hint
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Header
            Constraint::Min(8),    // Phases
            Constraint::Length(1), // Hint
        ])
        .split(inner);

    // Header
    let header = Paragraph::new("Select a phase to run:")
        .style(Style::default().fg(Color::White));
    frame.render_widget(header, chunks[0]);

    // Phases
    render_phases(frame, state, chunks[1]);

    // Hint
    let hint = Paragraph::new("↑↓ Navigate  Enter Select  Esc Cancel")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(hint, chunks[2]);
}

/// Render the phase list (single-select).
fn render_phases(frame: &mut Frame, state: &PhaseSelectorState, area: Rect) {
    let phases = AlbumArtistPhase::all();
    let cursor = state.cursor();

    let mut lines = Vec::new();

    for (i, phase) in phases.iter().enumerate() {
        let is_focused = cursor == i;

        // Arrow indicator for focused item
        let indicator = if is_focused { ">" } else { " " };

        // Style based on focus
        let style = if is_focused {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        // Phase name line
        let name_line = Line::from(vec![
            Span::styled(format!("  {} ", indicator), style),
            Span::styled(phase.name(), style),
        ]);
        lines.push(name_line);

        // Description line (dimmed)
        let desc_style = Style::default().fg(Color::DarkGray);
        let desc_line = Line::from(Span::styled(
            format!("      {}", phase.description()),
            desc_style,
        ));
        lines.push(desc_line);

        // Blank line between phases
        if i < phases.len() - 1 {
            lines.push(Line::from(""));
        }
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}
