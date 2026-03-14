//! Wizard Pane: scrollable right-side info panel rendering.
//!
//! State machine lives in mm-ui. This module provides `render_wizard_pane()`.

pub use mm_ui::wizard_pane::WizardPaneState;

use crate::widgets::rich_text::{render_rich, RichBlock};
use ratatui::{
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

/// Render the wizard pane as a bordered, scrollable panel.
pub fn render_wizard_pane(
    f: &mut Frame,
    area: Rect,
    title: &str,
    content: &[RichBlock],
    state: &mut WizardPaneState,
    focused: bool,
) {
    let border_color = if focused {
        Color::Magenta
    } else {
        Color::DarkGray
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(format!(" {title} "))
        .title_style(Style::default().fg(if focused {
            Color::Magenta
        } else {
            Color::White
        }));

    let inner = block.inner(area);

    // Render rich blocks to flat lines at inner width
    let lines = render_rich(content, inner.width);

    state.content_height = lines.len();
    state.visible_height = inner.height as usize;

    // Clamp scroll
    let max_scroll = state
        .content_height
        .saturating_sub(state.visible_height);
    if state.scroll > max_scroll {
        state.scroll = max_scroll;
    }

    let visible_lines: Vec<_> = lines
        .into_iter()
        .skip(state.scroll)
        .take(state.visible_height)
        .collect();

    f.render_widget(
        Paragraph::new(visible_lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}
