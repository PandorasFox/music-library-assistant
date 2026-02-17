//! Database Vacuum Prompt
//!
//! Render helpers for the VacuumPrompt startup view.
//!
//! The VacuumPrompt view is an ActiveView variant driven by the main event loop.
//! Key handling goes through dispatch_action; phase transitions happen in tick.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::ui::active_view::{VacuumPhase, VacuumPromptState};

/// Render the vacuum prompt view based on current phase.
pub fn render_vacuum_view(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &VacuumPromptState,
) {
    match state.phase {
        VacuumPhase::Prompt => render_vacuum_prompt(f, area, state.pct, state.free_mb),
        VacuumPhase::Compacting => render_vacuum_progress(f, area),
        VacuumPhase::Complete { new_size_mb } => {
            render_vacuum_complete(f, area, state.free_mb, new_size_mb);
        }
    }
}

fn render_vacuum_prompt(f: &mut ratatui::Frame, area: Rect, pct: u64, free_mb: f64) {
    let dialog_width = 60.min(area.width.saturating_sub(4));
    let dialog_height = 10.min(area.height.saturating_sub(4));

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let lines = vec![
        ratatui::text::Line::from(""),
        ratatui::text::Line::from(format!(
            "Database has ~{}% reclaimable space ({:.0} MB).",
            pct, free_mb
        ))
        .style(Style::default().fg(Color::White)),
        ratatui::text::Line::from("Compacting takes a moment but reduces disk usage.").style(
            Style::default().fg(Color::DarkGray),
        ),
        ratatui::text::Line::from(""),
        ratatui::text::Line::from("[Enter] Compact    [Esc] Skip")
            .style(Style::default().fg(Color::Cyan)),
    ];

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Compact Database? ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Center);

    f.render_widget(paragraph, dialog_area);
}

fn render_vacuum_progress(f: &mut ratatui::Frame, area: Rect) {
    let dialog_width = 50.min(area.width.saturating_sub(4));
    let dialog_height = 7;

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let lines = vec![
        ratatui::text::Line::from(""),
        ratatui::text::Line::from("Compacting database...")
            .style(Style::default().fg(Color::Cyan)),
        ratatui::text::Line::from(""),
        ratatui::text::Line::from("This may take a moment.")
            .style(Style::default().fg(Color::DarkGray)),
    ];

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Compacting ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Center);

    f.render_widget(paragraph, dialog_area);
}

fn render_vacuum_complete(f: &mut ratatui::Frame, area: Rect, reclaimed_mb: f64, new_size_mb: f64) {
    let dialog_width = 50.min(area.width.saturating_sub(4));
    let dialog_height = 7;

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let lines = vec![
        ratatui::text::Line::from(""),
        ratatui::text::Line::from(format!("Reclaimed ~{:.0} MB", reclaimed_mb))
            .style(Style::default().fg(Color::Green)),
        ratatui::text::Line::from(format!("Database is now {:.0} MB", new_size_mb))
            .style(Style::default().fg(Color::DarkGray)),
    ];

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Compaction Complete ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green)),
        )
        .alignment(Alignment::Center);

    f.render_widget(paragraph, dialog_area);
}
