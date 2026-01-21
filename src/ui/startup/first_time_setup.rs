//! First-Time Setup Modal
//!
//! Handles database creation when no database file exists.
//! Shows a welcome dialog and creates the database with the latest schema.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use ratatui::backend::Backend;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Terminal;
use std::path::Path;

use crate::corpus::db::Database;
use crate::corpus::mutations::MigrationRegistry;
use crate::witch::confirm_decision;

/// Handle first-time setup when no database exists.
///
/// Shows a dialog prompting the user to create a new database, then
/// initializes it with the latest schema version (no migrations needed).
pub fn handle_first_time_setup<B: Backend>(
    terminal: &mut Terminal<B>,
    db_path: &Path,
) -> Result<()> {
    // Ensure parent directory exists
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let db_display = db_path.to_string_lossy();

    // Render first-time setup dialog
    loop {
        terminal.draw(|f| {
            let area = f.area();

            let dialog_width = 65.min(area.width.saturating_sub(4));
            let dialog_height = 14.min(area.height.saturating_sub(4));

            let dialog_area = Rect {
                x: (area.width.saturating_sub(dialog_width)) / 2,
                y: (area.height.saturating_sub(dialog_height)) / 2,
                width: dialog_width,
                height: dialog_height,
            };

            f.render_widget(Clear, dialog_area);

            let lines = vec![
                Line::from(""),
                Line::from("Welcome to MLA!").style(
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                ),
                Line::from(""),
                Line::from("No database found. MLA will create a new one at:").style(
                    Style::default().fg(Color::White),
                ),
                Line::from(""),
                Line::from(format!("  {}", db_display)).style(
                    Style::default().fg(Color::Yellow),
                ),
                Line::from(""),
                Line::from("After setup, your corpus will be scanned.").style(
                    Style::default().fg(Color::DarkGray),
                ),
                Line::from(""),
                Line::from("[Enter] Create Database    [Esc] Exit")
                    .style(Style::default().fg(Color::Cyan)),
            ];

            let paragraph = Paragraph::new(lines)
                .block(
                    Block::default()
                        .title(" First-Time Setup ")
                        .title_alignment(Alignment::Center)
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Cyan)),
                )
                .alignment(Alignment::Center);

            f.render_widget(paragraph, dialog_area);
        })?;

        // Wait for user input
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => {
                    let _witness = confirm_decision();
                    break;
                }
                KeyCode::Esc => {
                    return Err(anyhow::anyhow!("Setup cancelled by user"));
                }
                _ => {}
            }
        }
    }

    // Show "creating database" status
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
            Line::from(""),
            Line::from("Creating database...").style(
                Style::default().fg(Color::Cyan),
            ),
            Line::from(""),
        ])
        .block(
            Block::default()
                .title(" Setup ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Center);

        f.render_widget(paragraph, dialog_area);
    })?;

    // Create database and set to latest schema version
    let db = Database::open(db_path)?;
    let registry = MigrationRegistry::new();
    db.set_schema_version(registry.latest_version())?;

    // Show completion message
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
            Line::from(""),
            Line::from("Database created successfully!").style(
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Line::from(""),
            Line::from("Starting initial corpus scan...")
                .style(Style::default().fg(Color::DarkGray)),
        ])
        .block(
            Block::default()
                .title(" Setup Complete ")
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green)),
        )
        .alignment(Alignment::Center);

        f.render_widget(paragraph, dialog_area);
    })?;

    std::thread::sleep(std::time::Duration::from_millis(800));
    Ok(())
}
