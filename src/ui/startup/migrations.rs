//! Schema Update UI
//!
//! Render helpers for the SchemaUpdate startup view.
//!
//! ## Architecture
//!
//! Schema reconciliation is orchestrated by the Witch, which:
//! 1. Checks if schema update is needed via `needs_schema_update()`
//! 2. Gets pending schema descriptions for UI display
//! 3. Queues reconciliation as `Task::Maintenance(DbMaintenanceTask::SchemaReconciliation)` via rayon
//! 4. Reconciliation executes via `execute_maintenance()` in execution.rs
//!
//! The schema update view is an ActiveView variant driven by the main event loop.
//! Key handling goes through dispatch_action; phase transitions happen in tick.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::ui::active_view::{SchemaUpdatePhase, SchemaUpdateState};

/// Render the schema update view based on current phase.
pub fn render_schema_update_view(f: &mut ratatui::Frame, area: Rect, state: &SchemaUpdateState) {
    match state.phase {
        SchemaUpdatePhase::Approval => render_approval(f, area, &state.descriptions),
        SchemaUpdatePhase::Running => render_progress(f, area, &state.descriptions),
        SchemaUpdatePhase::Complete => render_complete(f, area, state.descriptions.len()),
    }
}

/// Render the schema update approval dialog.
fn render_approval(f: &mut ratatui::Frame, area: Rect, pending: &[String]) {
    let change_count = pending.len();

    let dialog_width = 60.min(area.width.saturating_sub(4));
    let dialog_height = (change_count as u16 + 12).min(area.height.saturating_sub(4));

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let mut lines = vec![
        ratatui::text::Line::from(""),
        ratatui::text::Line::from("MM needs to update your database schema.")
            .style(Style::default().fg(Color::White)),
        ratatui::text::Line::from(""),
        ratatui::text::Line::from("Pending changes:").style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
    ];

    for desc in pending {
        lines.push(ratatui::text::Line::from(format!("  {}", desc)));
    }

    lines.push(ratatui::text::Line::from(""));
    lines.push(
        ratatui::text::Line::from("This cannot be interrupted once started.")
            .style(Style::default().fg(Color::Yellow)),
    );
    lines.push(ratatui::text::Line::from(""));
    lines.push(
        ratatui::text::Line::from("[Enter] Proceed    [Esc] Exit")
            .style(Style::default().fg(Color::Cyan)),
    );

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Schema Update Required ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .alignment(Alignment::Center);

    f.render_widget(paragraph, dialog_area);
}

/// Render the schema update progress dialog.
fn render_progress(f: &mut ratatui::Frame, area: Rect, pending: &[String]) {
    let change_count = pending.len();

    let dialog_width = 60.min(area.width.saturating_sub(4));
    let dialog_height = (change_count as u16 + 10).min(area.height.saturating_sub(4));

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let mut lines = vec![
        ratatui::text::Line::from(""),
        ratatui::text::Line::from("Applying schema changes:").style(
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        ratatui::text::Line::from(""),
    ];

    for desc in pending {
        lines.push(ratatui::text::Line::from(format!("  {}", desc)));
    }

    lines.push(ratatui::text::Line::from(""));
    lines.push(ratatui::text::Line::from("Reconciling...").style(Style::default().fg(Color::Cyan)));

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Schema Update ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .alignment(Alignment::Center);

    f.render_widget(paragraph, dialog_area);
}

/// Render the schema update complete dialog.
fn render_complete(f: &mut ratatui::Frame, area: Rect, change_count: usize) {
    let dialog_width = 50.min(area.width.saturating_sub(4));
    let dialog_height = 7;

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let paragraph = Paragraph::new(vec![
        ratatui::text::Line::from(""),
        ratatui::text::Line::from(format!(
            "{} schema change(s) applied successfully",
            change_count
        ))
        .style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        ratatui::text::Line::from(""),
        ratatui::text::Line::from("Starting application...")
            .style(Style::default().fg(Color::DarkGray)),
    ])
    .block(
        Block::default()
            .title(" Schema Update Complete ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green)),
    )
    .alignment(Alignment::Center);

    f.render_widget(paragraph, dialog_area);
}
