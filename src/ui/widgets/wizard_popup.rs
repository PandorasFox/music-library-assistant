//! Wizard Popup: stateless floating info box anchored near the cursor row.
//!
//! Renders a small bordered `Paragraph` to the right of the list area.
//! Falls back to below-cursor positioning if no horizontal room remains.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};
/// Horizontal overhead: 2 border chars + 2 interior padding chars.
const BORDER_H_PAD: u16 = 4;
/// Border padding: 1 top border + 1 bottom border.
const BORDER_V_PAD: u16 = 2;
/// Padding between popup right edge and list right border.
const RIGHT_PAD: u16 = 3;
/// Minimum useful popup width (content chars, not including borders).
const MIN_CONTENT_WIDTH: u16 = 12;

pub struct WizardPopup;

impl WizardPopup {
    /// Render a bordered popup anchored next to the cursor item's text.
    ///
    /// - `content`: lines to display inside the popup
    /// - `anchor_x`: X coordinate just after the cursor item's text
    /// - `anchor_y`: Y coordinate of the cursor row
    /// - `list_area`: the Rect of the list pane (popup clamped within)
    pub fn render(
        f: &mut Frame,
        content: &[Line<'_>],
        anchor_x: u16,
        anchor_y: u16,
        list_area: Rect,
    ) {
        if content.is_empty() {
            return;
        }

        let content_height = content.len() as u16;
        let content_width = content
            .iter()
            .map(|line| line.width() as u16)
            .max()
            .unwrap_or(0)
            .max(MIN_CONTENT_WIDTH);

        let list_right = list_area.x + list_area.width;
        let max_popup_width = list_right.saturating_sub(anchor_x).saturating_sub(RIGHT_PAD);
        let popup_width = (content_width + BORDER_H_PAD).min(max_popup_width);
        let popup_height = (content_height + BORDER_V_PAD).min(list_area.height);

        if popup_width < MIN_CONTENT_WIDTH + BORDER_H_PAD {
            return; // Not enough room
        }

        let popup_x = anchor_x;

        // Vertically center the popup on anchor_y, clamped within list bounds.
        let half_h = popup_height / 2;
        let ideal_y = anchor_y.saturating_sub(half_h);
        let max_y = list_area.y + list_area.height.saturating_sub(popup_height);
        let popup_y = ideal_y.max(list_area.y).min(max_y);

        let area = Rect::new(popup_x, popup_y, popup_width, popup_height);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta))
            .padding(ratatui::widgets::Padding::horizontal(1));

        // Clear underneath so the popup renders on top of list content.
        f.render_widget(Clear, area);
        f.render_widget(
            Paragraph::new(content.to_vec())
                .block(block)
                .wrap(Wrap { trim: false }),
            area,
        );
    }
}

/// Compute the popup Rect without rendering (for testing).
pub fn compute_popup_rect(
    content_lines: usize,
    max_content_width: u16,
    anchor_x: u16,
    anchor_y: u16,
    list_area: Rect,
) -> Option<Rect> {
    if content_lines == 0 {
        return None;
    }

    let content_height = content_lines as u16;
    let content_width = max_content_width.max(MIN_CONTENT_WIDTH);

    let list_right = list_area.x + list_area.width;
    let max_popup_width = list_right.saturating_sub(anchor_x).saturating_sub(RIGHT_PAD);
    let popup_width = (content_width + BORDER_H_PAD).min(max_popup_width);
    let popup_height = (content_height + BORDER_V_PAD).min(list_area.height);

    if popup_width < MIN_CONTENT_WIDTH + BORDER_H_PAD {
        return None;
    }

    let half_h = popup_height / 2;
    let ideal_y = anchor_y.saturating_sub(half_h);
    let max_y = list_area.y + list_area.height.saturating_sub(popup_height);
    let popup_y = ideal_y.max(list_area.y).min(max_y);

    Some(Rect::new(anchor_x, popup_y, popup_width, popup_height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_anchored_to_text_centered() {
        let list_area = Rect::new(0, 0, 80, 20);
        // Text ends at x=30, popup content is 20 wide
        let r = compute_popup_rect(3, 20, 30, 10, list_area).unwrap();
        assert_eq!(r.x, 30); // starts right after text
        // Centered: height=5, half=2, ideal_y=8, clamped within [0, 15]
        assert_eq!(r.y, 8);
        assert_eq!(r.width, 24); // 20 + 4 (border + padding)
        assert_eq!(r.height, 5); // 3 + 2 border
    }

    #[test]
    fn popup_clamped_to_list_bottom() {
        let list_area = Rect::new(0, 0, 80, 15);
        let r = compute_popup_rect(5, 20, 30, 14, list_area).unwrap();
        assert!(r.y + r.height <= list_area.y + list_area.height);
    }

    #[test]
    fn popup_clamped_to_list_top() {
        let list_area = Rect::new(0, 5, 80, 20);
        // anchor_y=6, height=5, half=2, ideal_y=4 → clamped to 5
        let r = compute_popup_rect(3, 20, 30, 6, list_area).unwrap();
        assert_eq!(r.y, 5); // clamped to list_area.y
    }

    #[test]
    fn popup_too_narrow_returns_none() {
        let list_area = Rect::new(0, 0, 80, 20);
        // anchor_x near right edge, not enough room for MIN_CONTENT_WIDTH + BORDER_H_PAD
        let r = compute_popup_rect(3, 20, 62, 10, list_area);
        assert!(r.is_none());
    }

    #[test]
    fn empty_content_returns_none() {
        let r = compute_popup_rect(0, 0, 30, 5, Rect::new(0, 0, 80, 20));
        assert!(r.is_none());
    }
}
