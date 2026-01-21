//! Deployment Preview UI
//!
//! Shows a comprehensive preview of deployment status for all libraries.
//! Displays healthy, to_deploy, stale, leftover, and conflict counts.
//! Allows the librarian to confirm (generate mutations) or cancel.
//!
//! TODO: This module is disabled while corpus::deploy is being updated.
//! Requires FullDeploymentStatus from the deploy module.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::ui::widgets::{LateralView, UnifiedTitleBar};

/// Actions returned from the deployment preview
#[derive(Debug, Clone)]
pub enum DeploymentPreviewAction {
    /// No action needed
    None,
    /// Confirm and generate mutations, transition to session review
    Confirm,
    /// Cancel and return to main menu
    Cancel,
    /// Cycle to next view in lateral ring (Tab)
    CycleNext,
    /// Cycle to previous view in lateral ring (Shift-Tab)
    CyclePrev,
}

/// State for the deployment preview
///
/// TODO: Re-enable when corpus::deploy is available.
/// Previously held Vec<FullDeploymentStatus> from the deploy module.
#[derive(Debug)]
pub struct DeploymentPreviewState {
    /// Session ID for mutations
    pub session_id: String,
}

impl DeploymentPreviewState {
    /// Create a new deployment preview state
    ///
    /// TODO: Restore full signature when corpus::deploy is re-enabled:
    /// pub fn new(statuses: Vec<FullDeploymentStatus>, session_id: String) -> Self
    pub fn new_stub(session_id: String) -> Self {
        Self { session_id }
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> DeploymentPreviewAction {
        match key.code {
            // Tab/Shift-Tab for lateral view cycling
            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    DeploymentPreviewAction::CyclePrev
                } else {
                    DeploymentPreviewAction::CycleNext
                }
            }
            KeyCode::BackTab => DeploymentPreviewAction::CyclePrev,
            KeyCode::Esc => DeploymentPreviewAction::Cancel,
            _ => DeploymentPreviewAction::None,
        }
    }

    /// Render the deployment preview
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Layout: Title bar at top (3 rows for borders), content below
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(UnifiedTitleBar::height()), // Title bar with borders
                Constraint::Min(10),                           // Content
            ])
            .split(area);

        // Render unified title bar
        let titlebar = UnifiedTitleBar::new(LateralView::Deploy);
        titlebar.render(f, main_chunks[0]);

        // Render disabled message
        let lines = vec![
            Line::from(""),
            Line::from("Deployment preview is temporarily disabled."),
            Line::from(""),
            Line::from("The deploy module is being updated to use the new"),
            Line::from("transaction API and library health signals."),
            Line::from(""),
            Line::from("Press Tab to cycle views or Esc to exit."),
        ];

        let para = Paragraph::new(lines)
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().borders(Borders::ALL).title("Deployment Preview (Disabled)"));
        f.render_widget(para, main_chunks[1]);
    }
}
