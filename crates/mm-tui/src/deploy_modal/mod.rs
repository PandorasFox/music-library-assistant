//! Deployment Modal
//!
//! Provides the interactive workflow for deploying corpus files to libraries.
//! The Deploy view is a lateral tab showing either:
//! - "Deployment up to date :)" with per-library file counts (when nothing to deploy)
//! - Full deployment preview with tabbed signal lists (when there's work)

pub mod preview;
pub mod types;

pub use types::DeployModalData;

// All data + interaction + action types live in mm-ui.
pub use mm_ui::view_state::lateral::deploy::{
    DeployAction, DeployInteraction, DeployViewData, DeployViewState,
};

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

use crate::helpers::format_si;
use crate::widgets::{Modal, ModalStyle};

/// Render the Deploy view, dispatching to either up-to-date or preview.
pub fn render_deploy_view(f: &mut Frame, area: Rect, state: &DeployViewState) {
    match &state.data {
        DeployViewData::UpToDate {
            library_file_counts,
        } => {
            render_up_to_date(f, area, library_file_counts);
        }
        DeployViewData::Preview { cached_data } => {
            preview::render_preview(
                cached_data,
                state.interaction.active_tab,
                state.interaction.tab_scroll,
                f,
                area,
            );
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
