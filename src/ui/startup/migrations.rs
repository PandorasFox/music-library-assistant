//! Database Migration UI
//!
//! Handles prompting the user to approve database schema migrations
//! when upgrading to a new version of MM.
//!
//! ## Architecture
//!
//! Migrations are orchestrated by the Witch, which:
//! 1. Checks if migrations are needed via `needs_migrations()`
//! 2. Gets pending migration descriptions for UI display
//! 3. Queues migrations as `Task::Migration` tasks via rayon
//! 4. Migrations execute via `execute_migration()` in execution.rs
//!
//! The migration UI loop runs until all migrations complete, ticking the
//! Witch each frame to process task results.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Terminal;
use std::time::Duration;

use crate::witch::Witch;

/// Result of running the migrations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// Migrations completed successfully.
    Completed,
    /// No migrations were needed.
    NotNeeded,
}

/// Run migrations with Witch orchestration.
///
/// This function:
/// 1. Shows approval dialog with pending migration descriptions
/// 2. On Enter: queues migrations via Witch and ticks until complete
/// 3. On Esc: returns error to abort startup
///
/// Returns `MigrationOutcome::Completed` when migrations finish.
pub fn run_migrations<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    witch: &mut Witch,
) -> Result<MigrationOutcome> {
    // Get pending migration descriptions
    let pending = witch.pending_migration_descriptions();
    if pending.is_empty() {
        return Ok(MigrationOutcome::NotNeeded);
    }
    let migration_count = pending.len();

    // Phase 1: Show approval dialog and wait for user input
    loop {
        terminal.draw(|f| {
            render_migration_approval(f, &pending);
        })?;

        // Wait for user input
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => {
                    // User approved - queue migrations through Witch
                    witch.queue_pending_migrations();
                    break;
                }
                KeyCode::Esc => {
                    // User declined - exit application
                    return Err(anyhow::anyhow!("Migration cancelled by user"));
                }
                _ => {
                    // Ignore other keys
                }
            }
        }
    }

    // Phase 2: Tick Witch until migrations complete
    loop {
        terminal.draw(|f| {
            render_migration_progress(f, &pending, witch);
        })?;

        // Tick the Witch to process migration tasks
        witch.tick();

        // Check if all work is done
        if !witch.has_pending() {
            break;
        }

        // Brief sleep to avoid busy-loop
        std::thread::sleep(Duration::from_millis(50));
    }

    // Phase 3: Show completion briefly
    terminal.draw(|f| {
        render_migration_complete(f, migration_count);
    })?;
    std::thread::sleep(Duration::from_millis(800));

    // Invalidate read-only connection so it picks up new schema
    witch.invalidate_read_only_conn();

    Ok(MigrationOutcome::Completed)
}

/// Render the migration approval dialog.
fn render_migration_approval(f: &mut ratatui::Frame, pending: &[String]) {
    let area = f.area();
    let migration_count = pending.len();

    // Center the dialog
    let dialog_width = 60.min(area.width.saturating_sub(4));
    let dialog_height = (migration_count as u16 + 12).min(area.height.saturating_sub(4));

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    // Clear the area behind the dialog
    f.render_widget(Clear, dialog_area);

    // Build migration list text
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
fn render_migration_progress(f: &mut ratatui::Frame, pending: &[String], witch: &Witch) {
    let area = f.area();
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

    let status = witch.status();
    let progress_text = if status.session_queued > 0 {
        format!(
            "{} / {} migrations...",
            status.total_processed, status.session_queued
        )
    } else {
        "Preparing...".to_string()
    };

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
        ratatui::text::Line::from(progress_text)
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
fn render_migration_complete(f: &mut ratatui::Frame, migration_count: usize) {
    let area = f.area();
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
