//! Database Migration UI
//!
//! Handles prompting the user to approve database schema migrations
//! when upgrading to a new version of MLA.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Terminal;

use crate::config;
use crate::corpus::db::Database;
use crate::corpus::mutations::MigrationRegistry;
use crate::witch::confirm_startup_migration;

/// Check for database migrations and run them with user approval.
///
/// For existing databases, checks for pending migrations and prompts user
/// to approve them (DecisionWitness pattern).
///
/// NOTE: First-time setup (no database exists) is handled separately by
/// `handle_first_time_setup` in this module.
pub fn check_and_run_migrations<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
) -> Result<()> {
    // Get database path
    let db_path = match config::get_db_path() {
        Ok(p) => p,
        Err(_) => return Ok(()), // No database path configured, skip
    };

    // First-time setup is handled by first_time_setup.rs
    if !db_path.exists() {
        return super::handle_first_time_setup(terminal, &db_path);
    }

    // Existing database: open and check for migrations
    // NOTE: This uses Database::open() (not read_only) because migrations
    // need write access. This runs before daemon exists and is explicitly
    // witnessed via confirm_decision() below.
    let db = match Database::open(&db_path) {
        Ok(d) => d,
        Err(_) => return Ok(()), // Can't open database, skip migrations
    };

    // Check if migrations are needed
    let registry = MigrationRegistry::new();
    if !registry.needs_migration(&db) {
        return Ok(());
    }

    // Get pending migration descriptions
    let pending = registry.pending_descriptions(&db);
    let migration_count = pending.len();

    // Render approval dialog and wait for user input
    loop {
        terminal.draw(|f| {
            let area = f.area();

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
                ratatui::text::Line::from("MLA needs to upgrade your database.").style(
                    Style::default().fg(Color::White),
                ),
                ratatui::text::Line::from(""),
                ratatui::text::Line::from("Pending migrations:").style(
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
            ];

            for desc in &pending {
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
        })?;

        // Wait for user input
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => {
                    // User approved - proceed with migrations
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

    // Show "running migrations" status
    terminal.draw(|f| {
        let area = f.area();

        let dialog_width = 60.min(area.width.saturating_sub(4));
        let dialog_height = (migration_count as u16 + 8).min(area.height.saturating_sub(4));

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

        for desc in &pending {
            lines.push(ratatui::text::Line::from(format!("  • {}", desc)));
        }

        lines.push(ratatui::text::Line::from(""));
        lines.push(
            ratatui::text::Line::from("Please wait...")
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
    })?;

    // Execute migrations (create witness to prove user approved)
    let witness = confirm_startup_migration();
    let result = registry.apply_all_pending(&db, &witness);

    match result {
        Ok(count) => {
            // Show completion message briefly
            terminal.draw(|f| {
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
                    ratatui::text::Line::from(format!("✓ {} migration(s) completed successfully", count))
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
            })?;

            // Brief pause to show completion
            std::thread::sleep(std::time::Duration::from_millis(800));
            Ok(())
        }
        Err(e) => {
            // Show error and wait for keypress
            terminal.draw(|f| {
                let area = f.area();
                let dialog_width = 60.min(area.width.saturating_sub(4));
                let dialog_height = 10;

                let dialog_area = Rect {
                    x: (area.width.saturating_sub(dialog_width)) / 2,
                    y: (area.height.saturating_sub(dialog_height)) / 2,
                    width: dialog_width,
                    height: dialog_height,
                };

                f.render_widget(Clear, dialog_area);

                let paragraph = Paragraph::new(vec![
                    ratatui::text::Line::from(""),
                    ratatui::text::Line::from("✗ Migration failed")
                        .style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    ratatui::text::Line::from(""),
                    ratatui::text::Line::from(format!("{}", e))
                        .style(Style::default().fg(Color::Red)),
                    ratatui::text::Line::from(""),
                    ratatui::text::Line::from("Press any key to continue...")
                        .style(Style::default().fg(Color::DarkGray)),
                ])
                .block(
                    Block::default()
                        .title(" Migration Error ")
                        .title_alignment(Alignment::Center)
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Red)),
                )
                .alignment(Alignment::Center);

                f.render_widget(paragraph, dialog_area);
            })?;

            // Wait for any keypress
            loop {
                if let Ok(Event::Key(_)) = event::read() {
                    break;
                }
            }

            // EXIT - we shall not continue without all migrations being applied
            Err(anyhow::anyhow!("Migration failed: {}. Cannot continue with incompatible database schema.", e))
        }
    }
}
