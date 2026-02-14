//! Database Vacuum Prompt
//!
//! Checks free-page ratio at startup and prompts the operator to compact
//! the database when reclaimable space exceeds a configurable threshold.
//!
//! Runs between migrations and db_thread spawn — no concurrent access.
//! The actual VACUUM execution is witness-guarded via `Witch::execute_vacuum()`.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Terminal;
use std::path::Path;

use crate::witch::Witch;

/// Check database free-page ratio and prompt for VACUUM if above threshold.
///
/// - `threshold <= 0.0`: disabled, returns immediately.
/// - Ratio at or below threshold: no action needed.
/// - Above threshold: shows prompt; Enter runs VACUUM (witness-guarded), Esc skips.
pub fn check_and_prompt_vacuum<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    db_path: &Path,
    threshold: f64,
    witch: &mut Witch,
) -> Result<()> {
    if threshold <= 0.0 {
        return Ok(());
    }

    if !db_path.exists() {
        return Ok(());
    }

    // Open a temporary read-only connection just for the PRAGMA queries.
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;

    let page_count: u64 = conn.pragma_query_value(None, "page_count", |row| row.get(0))?;
    let freelist_count: u64 = conn.pragma_query_value(None, "freelist_count", |row| row.get(0))?;
    let page_size: u64 = conn.pragma_query_value(None, "page_size", |row| row.get(0))?;

    drop(conn);

    if page_count == 0 {
        return Ok(());
    }

    let ratio = freelist_count as f64 / page_count as f64;
    if ratio <= threshold {
        return Ok(());
    }

    let pct = (ratio * 100.0).round() as u64;
    let free_bytes = freelist_count * page_size;
    let free_mb = free_bytes as f64 / (1024.0 * 1024.0);

    // Prompt loop
    loop {
        terminal.draw(|f| {
            render_vacuum_prompt(f, pct, free_mb);
        })?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Enter => {
                    // Show "compacting..." while VACUUM runs
                    terminal.draw(|f| {
                        render_vacuum_progress(f);
                    })?;

                    // Execute VACUUM through the Witch (witness-guarded)
                    witch.execute_vacuum(db_path)?;

                    // Re-query to show reclaimed amount
                    let conn2 = rusqlite::Connection::open_with_flags(
                        db_path,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                    )?;
                    let new_page_count: u64 =
                        conn2.pragma_query_value(None, "page_count", |row| row.get(0))?;
                    let new_size_mb = (new_page_count * page_size) as f64 / (1024.0 * 1024.0);
                    drop(conn2);

                    terminal.draw(|f| {
                        render_vacuum_complete(f, free_mb, new_size_mb);
                    })?;
                    std::thread::sleep(std::time::Duration::from_millis(1200));

                    return Ok(());
                }
                KeyCode::Esc => {
                    return Ok(());
                }
                _ => {}
            }
        }
    }
}

fn render_vacuum_prompt(f: &mut ratatui::Frame, pct: u64, free_mb: f64) {
    let area = f.area();
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

fn render_vacuum_progress(f: &mut ratatui::Frame) {
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

fn render_vacuum_complete(f: &mut ratatui::Frame, reclaimed_mb: f64, new_size_mb: f64) {
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
