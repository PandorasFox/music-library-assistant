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

use crate::ui::input::InputAction;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

use crate::ui::helpers::format_si;
use crate::ui::widgets::{Modal, ModalStyle};

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
    Preview(Box<DeploymentPreviewState>),
}

impl DeployViewState {
    /// Handle semantic input action for the Deploy view.
    pub fn handle_input(&mut self, action: &InputAction) -> DeployAction {
        match self {
            DeployViewState::UpToDate { .. } => match action {
                InputAction::CycleNext => DeployAction::CycleNext,
                InputAction::CyclePrev => DeployAction::CyclePrev,
                InputAction::Cancel => DeployAction::RequestQuit,
                _ => DeployAction::None,
            },
            DeployViewState::Preview(preview) => {
                match action {
                    InputAction::CycleNext => DeployAction::CycleNext,
                    InputAction::CyclePrev => DeployAction::CyclePrev,
                    InputAction::Cancel => DeployAction::RequestQuit,
                    InputAction::Confirm => DeployAction::Confirm,
                    // Left/Right switch deploy tabs, Up/Down/PgUp/PgDn scroll
                    InputAction::NavLeft => {
                        preview.active_tab = preview.active_tab.prev();
                        DeployAction::None
                    }
                    InputAction::NavRight => {
                        preview.active_tab = preview.active_tab.next();
                        DeployAction::None
                    }
                    InputAction::NavUp => {
                        let idx = preview.active_tab.index();
                        preview.tab_scroll[idx] = preview.tab_scroll[idx].saturating_sub(1);
                        DeployAction::None
                    }
                    InputAction::NavDown => {
                        let idx = preview.active_tab.index();
                        let max_scroll = preview.max_scroll_for_current_tab();
                        if preview.tab_scroll[idx] < max_scroll {
                            preview.tab_scroll[idx] += 1;
                        }
                        DeployAction::None
                    }
                    InputAction::PageUp => {
                        let idx = preview.active_tab.index();
                        preview.tab_scroll[idx] = preview.tab_scroll[idx].saturating_sub(10);
                        DeployAction::None
                    }
                    InputAction::PageDown => {
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
            DeployViewState::UpToDate {
                library_file_counts,
            } => {
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
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];

    if !library_file_counts.is_empty() {
        content.push(Line::from(""));

        // Find max name width for alignment
        let max_name = library_file_counts
            .iter()
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
        .style(ModalStyle {
            border_color: Color::Green,
            background: Color::Reset,
            title_style: Style::default().fg(Color::Green),
        })
        .fixed_size(width, height)
        .centered()
        .render(f, area);
}
