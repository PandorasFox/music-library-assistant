//! StandardList rendering — logic re-exported from mm-ui.

pub use mm_ui::standard_list::{
    ListEntry, ListFocus, ListInputResult, StandardListConfig, StandardListState,
};

use ratatui::{
    layout::{Layout, Constraint, Direction, Rect},
    style::{Color, Style, Modifier},
    text::Line,
    widgets::Clear,
    Frame,
};

use super::{
    WizardItem, WizardOffer, WizardState,
    rich_text::RichBlock,
    wizard_pane::render_wizard_pane,
    wizard_popup::WizardPopup,
};

/// Render a StandardList with wizard integration.
///
/// `render_item` closure receives (index, is_cursor, is_selected, available_width)
/// and returns a styled `Line` for that row.
pub fn render_standard_list<T: WizardItem>(
    state: &mut StandardListState,
    f: &mut Frame,
    area: Rect,
    items: &[T],
    render_item: impl Fn(usize, bool, bool, u16) -> Line<'static>,
    title: &str,
    focused: bool,
) {
    let item_count = items.len();

    // Check wizard state for pane split.
    let offer = items
        .get(state.cursor)
        .and_then(|i| i.wizard(area.width));

    let has_pane_data = matches!(
        (&state.wizard_state, &offer),
        (WizardState::ShowingPane, Some(WizardOffer::Pane { .. }))
            | (WizardState::ShowingPane, Some(WizardOffer::Both { .. }))
    );

    let (list_area, pane_area, pane_is_overlay) = if has_pane_data {
        let normal_pane_width = area.width * 40 / 100;
        let needs_overlay = state.config.pane_min_width > 0
            && normal_pane_width < state.config.pane_min_width;

        if needs_overlay {
            // Overlay: list renders full width, pane overlaps from the right.
            let pane_w = state.config.pane_min_width.min(area.width.saturating_sub(4));
            let pane_x = area.x + area.width - pane_w;
            let pane_rect = Rect::new(pane_x, area.y, pane_w, area.height);
            (area, Some(pane_rect), true)
        } else {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(60),
                    Constraint::Percentage(40),
                ])
                .split(area);
            (chunks[0], Some(chunks[1]), false)
        }
    } else {
        (area, None, false)
    };

    // Render list.
    render_list_inner(state, f, list_area, items, &render_item, title, focused, item_count);

    // Render wizard pane if active.
    if let Some(pane_rect) = pane_area {
        if let Some(ref offer) = offer {
            let (pane_title, pane_content) = match offer {
                WizardOffer::Pane { title, content } => (title.as_str(), content.as_slice()),
                WizardOffer::Both {
                    pane_title,
                    pane_content,
                    ..
                } => (pane_title.as_str(), pane_content.as_slice()),
                _ => ("", &[] as &[RichBlock]),
            };
            // Clear underneath when pane overlays the list.
            if pane_is_overlay {
                f.render_widget(Clear, pane_rect);
            }
            let pane_focused = state.focus == ListFocus::WizardPane;
            render_wizard_pane(
                f,
                pane_rect,
                pane_title,
                pane_content,
                &mut state.wizard_pane,
                pane_focused,
            );
        }
    }

    // Render wizard popup overlay if active.
    let has_popup_data = matches!(
        (&state.wizard_state, &offer),
        (WizardState::ShowingPopup, Some(WizardOffer::Popup(_)))
            | (WizardState::ShowingPopup, Some(WizardOffer::Both { .. }))
    );

    if has_popup_data {
        if let Some(ref offer) = offer {
            let popup_lines = match offer {
                WizardOffer::Popup(lines) => lines.as_slice(),
                WizardOffer::Both { popup, .. } => popup.as_slice(),
                _ => &[],
            };

            // Compute anchor: cursor row's screen position + text width.
            let cursor_screen_row = state.cursor.saturating_sub(state.scroll) as u16;
            let anchor_y = list_area.y + 1 + cursor_screen_row; // +1 for border
            let cursor_line = render_item(
                state.cursor,
                true,
                state.selected.contains(&state.cursor),
                list_area.width.saturating_sub(2), // inner width
            );
            let text_width = cursor_line.width() as u16;
            let anchor_x = list_area.x + 1 + text_width + 1; // +1 border, +1 gap

            WizardPopup::render(f, popup_lines, anchor_x, anchor_y, list_area);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_list_inner<T>(
    state: &mut StandardListState,
    f: &mut Frame,
    area: Rect,
    _items: &[T],
    render_item: &impl Fn(usize, bool, bool, u16) -> Line<'static>,
    title: &str,
    focused: bool,
    item_count: usize,
) {
    let border_color = if focused { Color::White } else { Color::DarkGray };
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(if title.is_empty() {
            String::new()
        } else {
            format!(" {title} ")
        })
        .title_style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));

    let inner = block.inner(area);
    f.render_widget(block, area);

    state.visible_height = inner.height as usize;

    // Clamp cursor.
    if item_count > 0 && state.cursor >= item_count {
        state.cursor = item_count - 1;
    }
    state.recompute_scroll(item_count);

    // Click targets.
    state.click_targets.clear();
    state.click_targets.set_list_area(inner);

    let content_width = inner.width;

    for (line_idx, idx) in (state.scroll..)
        .take(state.visible_height)
        .enumerate()
    {
        if idx >= item_count {
            break;
        }

        let is_cursor = idx == state.cursor;
        let is_selected = state.selected.contains(&idx);
        let line = render_item(idx, is_cursor, is_selected, content_width);

        let y = inner.y + line_idx as u16;
        state.click_targets.add_row(idx.to_string(), y);

        let line_area = Rect::new(inner.x, y, content_width, 1);

        // Render the line content.
        f.render_widget(
            ratatui::widgets::Paragraph::new(line),
            line_area,
        );
    }
}
