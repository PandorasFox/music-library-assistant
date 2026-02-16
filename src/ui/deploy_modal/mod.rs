//! Deployment Modal
//!
//! Provides the interactive workflow for deploying corpus files to libraries.
//! The Deploy view is a lateral tab showing either:
//! - "Deployment up to date :)" with per-library file counts (when nothing to deploy)
//! - Full deployment preview with tabbed signal lists (when there's work)

pub mod preview;
pub mod types;

pub use preview::DeploymentPreviewState;
pub use types::DeployModalData;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

use crate::ui::helpers::format_si;
use crate::ui::widgets::Modal;

/// Actions returned from the Deploy lateral view.
#[derive(Debug, Clone)]
pub enum DeployAction {
    /// No action needed.
    None,
    /// Cycle to next lateral view (Tab).
    CycleNext,
    /// Cycle to previous lateral view (Shift-Tab).
    CyclePrev,
    /// User confirmed deployment — generate mutations (Preview only).
    Confirm,
    /// Request quit (Esc).
    RequestQuit,
}

/// State for the Deploy lateral view tab.
#[derive(Debug)]
pub enum DeployViewState {
    /// Nothing to deploy — show centered "up to date" modal with library file counts.
    UpToDate {
        library_file_counts: Vec<(String, usize)>,
    },
    /// Actionable deploy preview (existing UI).
    Preview(DeploymentPreviewState),
}

impl DeployViewState {
    /// Handle key input for the Deploy view.
    pub fn handle_key(&mut self, key: KeyEvent) -> DeployAction {
        match self {
            DeployViewState::UpToDate { .. } => {
                match key.code {
                    KeyCode::Tab => {
                        if key.modifiers.contains(KeyModifiers::SHIFT) {
                            DeployAction::CyclePrev
                        } else {
                            DeployAction::CycleNext
                        }
                    }
                    KeyCode::BackTab => DeployAction::CyclePrev,
                    KeyCode::Esc => DeployAction::RequestQuit,
                    _ => DeployAction::None,
                }
            }
            DeployViewState::Preview(preview) => {
                match key.code {
                    KeyCode::Tab => {
                        if key.modifiers.contains(KeyModifiers::SHIFT) {
                            DeployAction::CyclePrev
                        } else {
                            DeployAction::CycleNext
                        }
                    }
                    KeyCode::BackTab => DeployAction::CyclePrev,
                    KeyCode::Esc => DeployAction::RequestQuit,
                    KeyCode::Enter => DeployAction::Confirm,
                    // Left/Right switch deploy tabs, Up/Down/PgUp/PgDn scroll
                    KeyCode::Left => {
                        preview.active_tab = preview.active_tab.prev();
                        DeployAction::None
                    }
                    KeyCode::Right => {
                        preview.active_tab = preview.active_tab.next();
                        DeployAction::None
                    }
                    KeyCode::Up => {
                        let idx = preview.active_tab.index();
                        preview.tab_scroll[idx] = preview.tab_scroll[idx].saturating_sub(1);
                        DeployAction::None
                    }
                    KeyCode::Down => {
                        let idx = preview.active_tab.index();
                        let max_scroll = preview.max_scroll_for_current_tab();
                        if preview.tab_scroll[idx] < max_scroll {
                            preview.tab_scroll[idx] += 1;
                        }
                        DeployAction::None
                    }
                    KeyCode::PageUp => {
                        let idx = preview.active_tab.index();
                        preview.tab_scroll[idx] = preview.tab_scroll[idx].saturating_sub(10);
                        DeployAction::None
                    }
                    KeyCode::PageDown => {
                        let idx = preview.active_tab.index();
                        let max_scroll = preview.max_scroll_for_current_tab();
                        preview.tab_scroll[idx] = (preview.tab_scroll[idx] + 10).min(max_scroll);
                        DeployAction::None
                    }
                    _ => DeployAction::None,
                }
            }
        }
    }

    /// Path of the currently selected item (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        match self {
            DeployViewState::UpToDate { .. } => None,
            DeployViewState::Preview(p) => p.selected_path(),
        }
    }

    /// Render the Deploy view.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        match self {
            DeployViewState::UpToDate { library_file_counts } => {
                render_up_to_date(f, area, library_file_counts);
            }
            DeployViewState::Preview(preview) => {
                preview.render(f, area);
            }
        }
    }
}

/// Render the "up to date" modal with per-library file counts.
fn render_up_to_date(f: &mut Frame, area: Rect, library_file_counts: &[(String, usize)]) {
    let mut content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Deployment up to date :)",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    if !library_file_counts.is_empty() {
        content.push(Line::from(""));

        // Find max name width for alignment
        let max_name = library_file_counts.iter()
            .map(|(name, _)| name.len())
            .max()
            .unwrap_or(0);

        for (name, count) in library_file_counts {
            let count_str = format_si(*count);
            content.push(Line::from(vec![
                Span::styled(
                    format!("  {:<width$}  ", name, width = max_name),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("{:>6}", count_str),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    let height = (content.len() as u16) + 4; // +4 for borders and padding
    let width = 40;

    Modal::new()
        .title(" Deploy ")
        .content(content)
        .fixed_size(width, height)
        .centered()
        .render(f, area);
}
