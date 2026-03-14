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

// Re-export interaction + action from mm-ui
pub use mm_ui::view_state::lateral::deploy::{DeployAction, DeployInputCtx, DeployInteraction};

use mm_ui::domain_types::DeployTab;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

use crate::helpers::format_si;
use crate::widgets::{Modal, ModalStyle};

/// Data for the Deploy lateral view tab.
#[derive(Debug)]
pub enum DeployViewData {
    /// Nothing to deploy — show centered "up to date" modal with library file counts.
    UpToDate {
        library_file_counts: Vec<(String, usize)>,
    },
    /// Actionable deploy preview (existing UI).
    Preview {
        cached_data: DeployModalData,
    },
}

impl DeployViewData {
    /// Path of the currently selected item (for status bar).
    pub fn selected_path(&self, interaction: &DeployInteraction) -> Option<&str> {
        match self {
            DeployViewData::UpToDate { .. } => None,
            DeployViewData::Preview { cached_data } => {
                DeploymentPreviewState::selected_path_static(
                    cached_data,
                    interaction.active_tab,
                    interaction.tab_scroll[interaction.active_tab.index()],
                )
            }
        }
    }

    /// Max scroll position for the given tab.
    pub fn max_scroll_for_tab(&self, active_tab: DeployTab) -> usize {
        match self {
            DeployViewData::UpToDate { .. } => 0,
            DeployViewData::Preview { cached_data } => {
                let count = match active_tab {
                    DeployTab::Healthy => cached_data.healthy.len(),
                    DeployTab::New => cached_data.new_by_dir.len(),
                    DeployTab::Conflicts => cached_data.conflicts.len(),
                    DeployTab::Leftover => cached_data.leftover_by_dir.len(),
                    DeployTab::Stale => cached_data.stale.len(),
                };
                count.saturating_sub(1)
            }
        }
    }

    /// Render the Deploy view.
    pub fn render(&self, f: &mut Frame, area: Rect, interaction: &DeployInteraction) {
        match self {
            DeployViewData::UpToDate {
                library_file_counts,
            } => {
                render_up_to_date(f, area, library_file_counts);
            }
            DeployViewData::Preview { cached_data } => {
                DeploymentPreviewState::render_static(
                    cached_data,
                    interaction.active_tab,
                    interaction.tab_scroll,
                    f,
                    area,
                );
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
