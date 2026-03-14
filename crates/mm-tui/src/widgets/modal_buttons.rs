//! ModalButtons rendering — logic re-exported from mm-ui.

pub use mm_ui::modal_buttons::{ButtonRowState, ModalButtons};

use ratatui::layout::Rect;
use ratatui::Frame;

use super::modal::{render_button_row, ConfirmationButton};

/// Render a button row and update click target rects.
///
/// `focused` controls whether the selected button is visually highlighted.
pub fn render_buttons<B: ModalButtons>(
    state: &mut ButtonRowState<B>,
    f: &mut Frame,
    area: Rect,
    ctx: &B::Context,
    focused: bool,
) {
    let all = B::all();
    state.button_rects.clear();

    let buttons: Vec<ConfirmationButton> = all
        .iter()
        .enumerate()
        .filter(|(_, b)| b.enabled(ctx))
        .map(|(_, b)| {
            let is_selected = focused && *b == state.selected;
            ConfirmationButton::new(b.label(ctx).into_owned(), b.color(ctx))
                .selected(is_selected)
        })
        .collect();

    render_button_row(f, area, &buttons);

    // Register button rects for click detection.
    let enabled_indices: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, b)| b.enabled(ctx))
        .map(|(i, _)| i)
        .collect();

    if !enabled_indices.is_empty() {
        let btn_width = area.width / enabled_indices.len() as u16;
        for (pos, &idx) in enabled_indices.iter().enumerate() {
            let btn_rect = Rect::new(
                area.x + (pos as u16 * btn_width),
                area.y,
                btn_width,
                area.height,
            );
            state.button_rects.set(idx.to_string(), btn_rect);
        }
    }
}
