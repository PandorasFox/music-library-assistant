//! Startup UI
//!
//! This module contains the UI components for startup flows:
//! - First-time setup (directory picker, DB setup dialog) — runs pre-loop in `run_tui()`
//! - Startup maintenance (non-interactive progress for schema reconciliation / vacuum)
//! - Intake confirmation (index unindexed files)
//!
//! Schema reconciliation and vacuum are auto-run by the Witch during startup.
//! The UI simply observes startup_state transitions and renders progress.
//! First-time setup runs as a blocking pre-loop step: `run_tui()` checks
//! `WitchStartupState::AwaitingSetup`, runs the setup UI, then sends
//! `CompleteSetup` to the Witch before entering the main event loop.
//!
//! Note: Progress screen (startup eyeballing, content analysis) is now
//! handled by the unified `progress_screen` module.

pub(crate) mod first_time_setup;
pub mod intake_confirmation;
pub mod migrations;
pub mod vacuum;

pub use first_time_setup::handle_db_setup_dialog;
pub use first_time_setup::run_directory_picker;
pub use intake_confirmation::{IntakeConfirmationAction, IntakeConfirmationState, IntakeSource};

use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// Render the non-interactive startup maintenance view.
///
/// Shown while the Witch auto-runs schema reconciliation or vacuum.
pub fn render_startup_maintenance(
    f: &mut ratatui::Frame,
    area: Rect,
    app: &super::App,
) {
    use crate::witch::WitchClient;
    let status = app.witch.witch_status();

    let (title, lines) = match status.startup_state {
        crate::witch::WitchStartupState::Reconciling => {
            let mut lines = vec![
                ratatui::text::Line::from(""),
                ratatui::text::Line::from("Reconciling database schema...")
                    .style(Style::default().fg(Color::Cyan)),
                ratatui::text::Line::from(""),
            ];
            // Show what's being reconciled if available via work status
            if status.work.pending > 0 || status.work.total_processed > 0 {
                lines.push(
                    ratatui::text::Line::from("This may take a moment.")
                        .style(Style::default().fg(Color::DarkGray)),
                );
            }
            (" Schema Update ", lines)
        }
        crate::witch::WitchStartupState::Vacuuming => {
            let lines = vec![
                ratatui::text::Line::from(""),
                ratatui::text::Line::from("Compacting database...")
                    .style(Style::default().fg(Color::Cyan)),
                ratatui::text::Line::from(""),
                ratatui::text::Line::from("This may take a moment.")
                    .style(Style::default().fg(Color::DarkGray)),
            ];
            (" Compacting ", lines)
        }
        _ => {
            let lines = vec![
                ratatui::text::Line::from(""),
                ratatui::text::Line::from("Starting up...")
                    .style(Style::default().fg(Color::Cyan)),
            ];
            (" Starting ", lines)
        }
    };

    let dialog_width = 50.min(area.width.saturating_sub(4));
    let dialog_height = (lines.len() as u16 + 3).min(area.height.saturating_sub(4));

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    f.render_widget(Clear, dialog_area);

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .title(title)
                .title_alignment(Alignment::Center)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Center);

    f.render_widget(paragraph, dialog_area);
}
