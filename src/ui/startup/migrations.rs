//! Database Migration UI
//!
//! Render helpers for the MigrationApproval startup view.
//!
//! ## Architecture
//!
//! Migrations are orchestrated by the Witch, which:
//! 1. Checks if migrations are needed via `needs_migrations()`
//! 2. Gets pending migration descriptions for UI display
//! 3. Queues migrations as `Task::Migration` tasks via rayon
//! 4. Migrations execute via `execute_migration()` in execution.rs
//!
//! The migration view is an ActiveView variant driven by the main event loop.
//! Key handling goes through dispatch_action; phase transitions happen in tick.

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::ui::active_view::{MigrationApprovalState, MigrationPhase};

/// Render the migration approval view based on current phase.
pub fn render_migration_view(
    f: &mut ratatui::Frame,
    area: Rect,
    state: &MigrationApprovalState,
) {
    match state.phase {
        MigrationPhase::Approval => render_migration_approval(f, area, &state.descriptions),
        MigrationPhase::Running => render_migration_progress(f, area, &state.descriptions),
        MigrationPhase::Complete => render_migration_complete(f, area, state.descriptions.len()),
    }
}

/// Render the migration approval dialog.
fn render_migration_approval(f: &mut ratatui::Frame, area: Rect, pending: &[String]) {
    let migration_count = pending.len();

    let dialog_width = 60.min(area.width.saturating_sub(4));
    let dialog_height = (migration_count as u16 + 12).min(area.height.saturating_sub(4));

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let mut lines = vec![
        ratatui::text::Line::from(""),
        ratatui::text::Line::from("MM needs to upgrade your database.").style(
            Style::default().fg(Color::White),
        ),
        ratatui::text::Line::from(""),
        ratatui::text::Line::from("Pending migrations:").style(
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
    ];

    for desc in pending {
        lines.push(ratatui::text::Line::from(format!("  • {}", desc)));
    }

    lines.push(ratatui::text::Line::from(""));
    lines.push(
        ratatui::text::Line::from("⚠ This cannot be interrupted once started.")
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
                .title(" Database Migration Required ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .alignment(Alignment::Center);

    f.render_widget(paragraph, dialog_area);
}

/// Render the migration progress dialog.
fn render_migration_progress(f: &mut ratatui::Frame, area: Rect, pending: &[String]) {
    let migration_count = pending.len();

    let dialog_width = 60.min(area.width.saturating_sub(4));
    let dialog_height = (migration_count as u16 + 10).min(area.height.saturating_sub(4));

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let mut lines = vec![
        ratatui::text::Line::from(""),
        ratatui::text::Line::from("Running migrations:").style(
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        ratatui::text::Line::from(""),
    ];

    for desc in pending {
        lines.push(ratatui::text::Line::from(format!("  • {}", desc)));
    }

    lines.push(ratatui::text::Line::from(""));
    lines.push(
        ratatui::text::Line::from("Migrating...")
            .style(Style::default().fg(Color::Cyan)),
    );

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Database Migration ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .alignment(Alignment::Center);

    f.render_widget(paragraph, dialog_area);
}

/// Render the migration complete dialog.
fn render_migration_complete(f: &mut ratatui::Frame, area: Rect, migration_count: usize) {
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
        ratatui::text::Line::from(format!("✓ {} migration(s) completed successfully", migration_count))
            .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ratatui::text::Line::from(""),
        ratatui::text::Line::from("Starting application...")
            .style(Style::default().fg(Color::DarkGray)),
    ])
    .block(
        Block::default()
            .title(" Migration Complete ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Green)),
    )
    .alignment(Alignment::Center);

    f.render_widget(paragraph, dialog_area);
}
