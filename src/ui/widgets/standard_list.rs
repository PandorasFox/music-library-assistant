//! StandardList: unified list state machine replacing bespoke list code.
//!
//! Provides cursor tracking, center-biased scrolling, click targets,
//! selection highlighting, multi-select, and wizard integration.
//!
//! Items implement two independent traits:
//! - `WizardItem` (derived) — what info containers the item declares
//! - `ListEntry` — what action the item produces on confirm
//!
//! StandardList composes both via `T: WizardItem + ListEntry` bounds.

use std::collections::BTreeSet;

use ratatui::{
    layout::{Layout, Constraint, Direction, Rect},
    style::{Color, Style, Modifier},
    text::Line,
    Frame,
};

use super::{
    ListClickTargets, WizardItem, WizardOffer, WizardState,
    wizard_pane::{WizardPaneState, render_wizard_pane},
    wizard_popup::WizardPopup,
};
use crate::ui::input::InputAction;

/// Behavioral trait for items in a StandardList.
///
/// Each view defines its own Action associated type. `WizardItem` is separate
/// (derived for data); this trait is manually implemented for action logic.
pub trait ListEntry {
    /// Action produced on Enter. Parent view interprets this.
    type Action;

    /// What happens on Enter. Receives the current multi-select set
    /// (empty if list doesn't support toggle, or nothing was toggled).
    /// `None` = item can't be confirmed (headers, info-only rows).
    fn on_confirm(&self, selected: &BTreeSet<usize>) -> Option<Self::Action>;

    /// Whether cursor can rest on this item during navigation.
    /// Return `false` for headers, separators, and other non-interactive rows.
    /// Default: `true`.
    fn is_selectable(&self) -> bool {
        true
    }
}

/// Which widget has focus within the StandardList.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListFocus {
    #[default]
    List,
    WizardPane,
}

/// Result from input handling. Generic over the item's Action type.
pub enum ListInputResult<A> {
    /// Input consumed (scroll, wizard toggle, etc.)
    Consumed,
    /// Cursor moved to a new item. Wizard state dismissed.
    CursorMoved,
    /// Space toggled selection on/off for cursor item.
    Toggled,
    /// Enter pressed on confirmable item. Carries the item's action.
    Confirm(A),
    /// Input not handled — caller should process (Cancel, CycleNext, etc.)
    Unhandled,
}

/// Configuration for constructing a StandardList.
#[derive(Debug, Clone)]
pub struct StandardListConfig {
    /// Whether items can be toggle-selected with Space.
    pub multi_select: bool,
}

impl Default for StandardListConfig {
    fn default() -> Self {
        Self {
            multi_select: false,
        }
    }
}

/// Unified list state machine.
pub struct StandardListState {
    pub cursor: usize,
    pub scroll: usize,
    pub click_targets: ListClickTargets,
    pub wizard_state: WizardState,
    pub wizard_pane: WizardPaneState,
    pub focus: ListFocus,
    pub config: StandardListConfig,
    /// Toggle-selected item indices (Space). Only used when config.multi_select.
    pub selected: BTreeSet<usize>,
    visible_height: usize,
}

impl StandardListState {
    pub fn new(config: StandardListConfig) -> Self {
        Self {
            cursor: 0,
            scroll: 0,
            click_targets: ListClickTargets::new(),
            wizard_state: WizardState::default(),
            wizard_pane: WizardPaneState::new(),
            focus: ListFocus::default(),
            config,
            selected: BTreeSet::new(),
            visible_height: 0,
        }
    }

    /// Total number of items the list can display (test-only; render sets this).
    #[cfg(test)]
    pub fn visible_height(&self) -> usize {
        self.visible_height
    }

    /// Set visible height (test-only; render sets this from inner area).
    #[cfg(test)]
    pub fn set_visible_height(&mut self, h: usize) {
        self.visible_height = h;
    }

    /// Recompute scroll position from cursor using center-biased model.
    fn recompute_scroll(&mut self, item_count: usize) {
        if item_count == 0 || self.visible_height == 0 {
            self.scroll = 0;
            return;
        }

        let half = self.visible_height / 2;
        let ideal_scroll = self.cursor.saturating_sub(half);
        let max_scroll = item_count.saturating_sub(self.visible_height);
        self.scroll = ideal_scroll.min(max_scroll);
    }

    /// Move cursor, recompute scroll, dismiss wizard.
    fn move_cursor(&mut self, new_cursor: usize, item_count: usize) {
        if item_count == 0 {
            return;
        }
        self.cursor = new_cursor.min(item_count.saturating_sub(1));
        self.recompute_scroll(item_count);
        self.wizard_state.dismiss();
        self.wizard_pane.reset();
        self.focus = ListFocus::List;
    }

    /// Find the next selectable item scanning upward from `from` (exclusive).
    fn find_selectable_up<T: ListEntry>(from: usize, items: &[T]) -> Option<usize> {
        (0..from).rev().find(|&i| items[i].is_selectable())
    }

    /// Find the next selectable item scanning downward from `from` (exclusive).
    fn find_selectable_down<T: ListEntry>(from: usize, items: &[T]) -> Option<usize> {
        ((from + 1)..items.len()).find(|&i| items[i].is_selectable())
    }

    /// Find the nearest selectable item at or after `pos`, falling back to before.
    fn find_nearest_selectable<T: ListEntry>(pos: usize, items: &[T]) -> Option<usize> {
        if pos < items.len() && items[pos].is_selectable() {
            return Some(pos);
        }
        // Search forward first
        if let Some(i) = (pos..items.len()).find(|&i| items[i].is_selectable()) {
            return Some(i);
        }
        // Then backward
        (0..pos).rev().find(|&i| items[i].is_selectable())
    }

    /// Handle input. Returns what happened.
    pub fn handle_input<T: WizardItem + ListEntry>(
        &mut self,
        action: &InputAction,
        items: &[T],
    ) -> ListInputResult<T::Action> {
        let item_count = items.len();

        match self.focus {
            ListFocus::WizardPane => self.handle_pane_input(action, items),
            ListFocus::List => self.handle_list_input(action, items, item_count),
        }
    }

    fn handle_pane_input<T: WizardItem + ListEntry>(
        &mut self,
        action: &InputAction,
        items: &[T],
    ) -> ListInputResult<T::Action> {
        match action {
            InputAction::Char('z') | InputAction::Char('Z') => {
                // Z in pane → advance wizard (ShowingPane → Idle)
                if let Some(offer) = items.get(self.cursor).and_then(|i| i.wizard(0)) {
                    self.wizard_state.advance(&offer);
                    if self.wizard_state.is_idle() {
                        self.focus = ListFocus::List;
                        self.wizard_pane.reset();
                    }
                } else {
                    self.wizard_state.dismiss();
                    self.focus = ListFocus::List;
                    self.wizard_pane.reset();
                }
                ListInputResult::Consumed
            }
            InputAction::Cancel => {
                self.wizard_state.dismiss();
                self.focus = ListFocus::List;
                self.wizard_pane.reset();
                ListInputResult::Consumed
            }
            _ => {
                if self.wizard_pane.handle_input(action) {
                    ListInputResult::Consumed
                } else {
                    ListInputResult::Unhandled
                }
            }
        }
    }

    fn handle_list_input<T: WizardItem + ListEntry>(
        &mut self,
        action: &InputAction,
        items: &[T],
        item_count: usize,
    ) -> ListInputResult<T::Action> {
        match action {
            InputAction::NavUp => {
                if let Some(next) = Self::find_selectable_up(self.cursor, items) {
                    self.move_cursor(next, item_count);
                    ListInputResult::CursorMoved
                } else {
                    ListInputResult::Consumed
                }
            }
            InputAction::NavDown => {
                if let Some(next) = Self::find_selectable_down(self.cursor, items) {
                    self.move_cursor(next, item_count);
                    ListInputResult::CursorMoved
                } else {
                    ListInputResult::Consumed
                }
            }
            InputAction::Home => {
                if let Some(first) = Self::find_nearest_selectable(0, items) {
                    self.move_cursor(first, item_count);
                }
                ListInputResult::CursorMoved
            }
            InputAction::End => {
                if item_count > 0 {
                    // Search backward from end for last selectable
                    let last = (0..item_count)
                        .rev()
                        .find(|&i| items[i].is_selectable())
                        .unwrap_or(item_count - 1);
                    self.move_cursor(last, item_count);
                }
                ListInputResult::CursorMoved
            }
            InputAction::PageUp => {
                let page = self.visible_height.saturating_sub(1).max(1);
                let target = self.cursor.saturating_sub(page);
                if let Some(pos) = Self::find_nearest_selectable(target, items) {
                    self.move_cursor(pos, item_count);
                }
                ListInputResult::CursorMoved
            }
            InputAction::PageDown => {
                let page = self.visible_height.saturating_sub(1).max(1);
                let target = (self.cursor + page).min(item_count.saturating_sub(1));
                if let Some(pos) = Self::find_nearest_selectable(target, items) {
                    self.move_cursor(pos, item_count);
                }
                ListInputResult::CursorMoved
            }
            InputAction::Char('z') | InputAction::Char('Z') => {
                if let Some(offer) = items.get(self.cursor).and_then(|i| i.wizard(0)) {
                    self.wizard_state.advance(&offer);
                    if self.wizard_state.is_showing_pane() {
                        self.focus = ListFocus::WizardPane;
                        self.wizard_pane.reset();
                    }
                    ListInputResult::Consumed
                } else {
                    ListInputResult::Consumed
                }
            }
            InputAction::Toggle => {
                if self.config.multi_select {
                    if self.selected.contains(&self.cursor) {
                        self.selected.remove(&self.cursor);
                    } else {
                        self.selected.insert(self.cursor);
                    }
                    ListInputResult::Toggled
                } else {
                    ListInputResult::Unhandled
                }
            }
            InputAction::Confirm => {
                if let Some(item) = items.get(self.cursor) {
                    if let Some(action) = item.on_confirm(&self.selected) {
                        ListInputResult::Confirm(action)
                    } else {
                        ListInputResult::Consumed
                    }
                } else {
                    ListInputResult::Consumed
                }
            }
            _ => ListInputResult::Unhandled,
        }
    }

    /// Render the list with wizard integration.
    ///
    /// `render_item` closure receives (index, is_cursor, is_selected, available_width)
    /// and returns a styled `Line` for that row.
    pub fn render<T: WizardItem>(
        &mut self,
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
            .get(self.cursor)
            .and_then(|i| i.wizard(area.width));

        let has_pane_data = match (&self.wizard_state, &offer) {
            (WizardState::ShowingPane, Some(WizardOffer::Pane { .. })) => true,
            (WizardState::ShowingPane, Some(WizardOffer::Both { .. })) => true,
            _ => false,
        };

        let (list_area, pane_area) = if has_pane_data {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(60),
                    Constraint::Percentage(40),
                ])
                .split(area);
            (chunks[0], Some(chunks[1]))
        } else {
            (area, None)
        };

        // Render list.
        self.render_list_inner(f, list_area, items, &render_item, title, focused, item_count);

        // Render wizard pane if active.
        if let Some(pane_rect) = pane_area {
            if let Some(ref offer) = offer {
                let (pane_title, pane_lines) = match offer {
                    WizardOffer::Pane { title, lines } => (title.as_str(), lines.as_slice()),
                    WizardOffer::Both {
                        pane_title,
                        pane_lines,
                        ..
                    } => (pane_title.as_str(), pane_lines.as_slice()),
                    _ => ("", &[] as &[Line<'_>]),
                };
                let pane_focused = self.focus == ListFocus::WizardPane;
                render_wizard_pane(
                    f,
                    pane_rect,
                    pane_title,
                    pane_lines,
                    &mut self.wizard_pane,
                    pane_focused,
                );
            }
        }

        // Render wizard popup overlay if active.
        let has_popup_data = match (&self.wizard_state, &offer) {
            (WizardState::ShowingPopup, Some(WizardOffer::Popup(_))) => true,
            (WizardState::ShowingPopup, Some(WizardOffer::Both { .. })) => true,
            _ => false,
        };

        if has_popup_data {
            if let Some(ref offer) = offer {
                let popup_lines = match offer {
                    WizardOffer::Popup(lines) => lines.as_slice(),
                    WizardOffer::Both { popup, .. } => popup.as_slice(),
                    _ => &[],
                };

                // Compute anchor: cursor row's screen position + text width.
                let cursor_screen_row = self.cursor.saturating_sub(self.scroll) as u16;
                let anchor_y = list_area.y + 1 + cursor_screen_row; // +1 for border
                let cursor_line = render_item(
                    self.cursor,
                    true,
                    self.selected.contains(&self.cursor),
                    list_area.width.saturating_sub(2), // inner width
                );
                let text_width = cursor_line.width() as u16;
                let anchor_x = list_area.x + 1 + text_width + 1; // +1 border, +1 gap

                WizardPopup::render(f, popup_lines, anchor_x, anchor_y, list_area);
            }
        }
    }

    fn render_list_inner<T>(
        &mut self,
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

        self.visible_height = inner.height as usize;

        // Clamp cursor.
        if item_count > 0 && self.cursor >= item_count {
            self.cursor = item_count - 1;
        }
        self.recompute_scroll(item_count);

        // Click targets.
        self.click_targets.clear();
        self.click_targets.set_list_area(inner);

        let content_width = inner.width;

        for (line_idx, idx) in (self.scroll..)
            .take(self.visible_height)
            .enumerate()
        {
            if idx >= item_count {
                break;
            }

            let is_cursor = idx == self.cursor;
            let is_selected = self.selected.contains(&idx);
            let line = render_item(idx, is_cursor, is_selected, content_width);

            let y = inner.y + line_idx as u16;
            self.click_targets.add_row(idx.to_string(), y);

            let line_area = Rect::new(inner.x, y, content_width, 1);

            // Render the line content.
            f.render_widget(
                ratatui::widgets::Paragraph::new(line),
                line_area,
            );
        }
    }

    /// Process a mouse click. Returns the item index if hit.
    /// Skips non-selectable items.
    pub fn handle_click<T: ListEntry>(
        &mut self,
        x: u16,
        y: u16,
        items: &[T],
    ) -> Option<usize> {
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < items.len() && items[idx].is_selectable() {
                    self.cursor = idx;
                    self.wizard_state.dismiss();
                    self.wizard_pane.reset();
                    self.focus = ListFocus::List;
                    return Some(idx);
                }
            }
        }
        None
    }

    /// Reset state for new items (e.g., when list content changes).
    #[cfg(test)]
    pub fn reset(&mut self) {
        self.cursor = 0;
        self.scroll = 0;
        self.selected.clear();
        self.wizard_state.dismiss();
        self.wizard_pane.reset();
        self.focus = ListFocus::List;
    }

    /// Ensure cursor is on a selectable item. Call after items change.
    pub fn clamp_cursor<T: ListEntry>(&mut self, items: &[T]) {
        if items.is_empty() {
            self.cursor = 0;
            return;
        }
        if self.cursor >= items.len() {
            self.cursor = items.len() - 1;
        }
        if !items[self.cursor].is_selectable() {
            if let Some(pos) = Self::find_nearest_selectable(self.cursor, items) {
                self.cursor = pos;
            }
        }
        self.recompute_scroll(items.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::text::Line;
    use crate::ui::widgets::wizard::{WizardOffer, WizardItem};

    // Simple test items.
    struct TestItem {
        label: String,
        wizard_offer: Option<WizardOffer>,
    }

    impl WizardItem for TestItem {
        fn wizard(&self, _width: u16) -> Option<WizardOffer> {
            self.wizard_offer.clone()
        }
    }

    impl ListEntry for TestItem {
        type Action = String;
        fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<String> {
            Some(self.label.clone())
        }
    }

    struct HeaderItem;

    impl WizardItem for HeaderItem {
        fn wizard(&self, _width: u16) -> Option<WizardOffer> {
            None
        }
    }

    impl ListEntry for HeaderItem {
        type Action = String;
        fn on_confirm(&self, _: &BTreeSet<usize>) -> Option<String> {
            None
        }
    }

    fn make_items(n: usize) -> Vec<TestItem> {
        (0..n)
            .map(|i| TestItem {
                label: format!("item_{i}"),
                wizard_offer: None,
            })
            .collect()
    }

    fn state_with_height(h: usize) -> StandardListState {
        let mut s = StandardListState::new(StandardListConfig::default());
        s.set_visible_height(h);
        s
    }

    // === Center-biased scroll model ===

    #[test]
    fn short_list_no_scroll() {
        let mut s = state_with_height(10);
        let items = make_items(5);
        s.cursor = 3;
        s.recompute_scroll(items.len());
        assert_eq!(s.scroll, 0);
    }

    #[test]
    fn cursor_top_half_no_scroll() {
        let mut s = state_with_height(10);
        s.cursor = 3; // half = 5, cursor < half
        s.recompute_scroll(20);
        assert_eq!(s.scroll, 0);
    }

    #[test]
    fn cursor_past_midpoint_centers() {
        let mut s = state_with_height(10);
        s.cursor = 10; // half = 5, ideal_scroll = 5, max_scroll = 10
        s.recompute_scroll(20);
        assert_eq!(s.scroll, 5);
    }

    #[test]
    fn cursor_near_bottom_pins() {
        let mut s = state_with_height(10);
        s.cursor = 18; // half = 5, ideal_scroll = 13, max_scroll = 10
        s.recompute_scroll(20);
        assert_eq!(s.scroll, 10);
    }

    #[test]
    fn cursor_at_last_item() {
        let mut s = state_with_height(10);
        s.cursor = 19; // half = 5, ideal_scroll = 14, max_scroll = 10
        s.recompute_scroll(20);
        assert_eq!(s.scroll, 10);
    }

    #[test]
    fn exact_fit_list() {
        let mut s = state_with_height(10);
        s.cursor = 5;
        s.recompute_scroll(10); // max_scroll = 0
        assert_eq!(s.scroll, 0);
    }

    #[test]
    fn two_item_list_in_large_viewport() {
        let mut s = state_with_height(10);
        s.cursor = 1;
        s.recompute_scroll(2); // max_scroll = 0
        assert_eq!(s.scroll, 0);
    }

    #[test]
    fn one_item_over_viewport() {
        let mut s = state_with_height(10);
        s.cursor = 10; // last item
        s.recompute_scroll(11); // max_scroll = 1
        // half=5, ideal=5, min(5,1) = 1
        assert_eq!(s.scroll, 1);
    }

    // === Cursor movement ===

    #[test]
    fn nav_up_at_top_stays() {
        let mut s = state_with_height(10);
        let items = make_items(5);
        let result = s.handle_input(&InputAction::NavUp, &items);
        assert_eq!(s.cursor, 0);
        assert!(matches!(result, ListInputResult::Consumed));
    }

    #[test]
    fn nav_down_moves() {
        let mut s = state_with_height(10);
        let items = make_items(5);
        let result = s.handle_input(&InputAction::NavDown, &items);
        assert_eq!(s.cursor, 1);
        assert!(matches!(result, ListInputResult::CursorMoved));
    }

    #[test]
    fn nav_down_at_bottom_stays() {
        let mut s = state_with_height(10);
        s.cursor = 4;
        let items = make_items(5);
        let result = s.handle_input(&InputAction::NavDown, &items);
        assert_eq!(s.cursor, 4);
        assert!(matches!(result, ListInputResult::Consumed));
    }

    #[test]
    fn home_end() {
        let mut s = state_with_height(10);
        s.cursor = 5;
        let items = make_items(20);
        s.handle_input(&InputAction::End, &items);
        assert_eq!(s.cursor, 19);
        s.handle_input(&InputAction::Home, &items);
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn page_up_down() {
        let mut s = state_with_height(10);
        let items = make_items(30);
        s.handle_input(&InputAction::PageDown, &items);
        assert_eq!(s.cursor, 9); // page = visible_height - 1 = 9
        s.handle_input(&InputAction::PageUp, &items);
        assert_eq!(s.cursor, 0);
    }

    // === Confirm ===

    #[test]
    fn confirm_on_actionable_item() {
        let mut s = state_with_height(10);
        let items = make_items(5);
        let result = s.handle_input(&InputAction::Confirm, &items);
        match result {
            ListInputResult::Confirm(action) => assert_eq!(action, "item_0"),
            _ => panic!("expected Confirm"),
        }
    }

    #[test]
    fn confirm_on_non_actionable_item() {
        let mut s = state_with_height(10);
        let items = vec![HeaderItem];
        let result = s.handle_input(&InputAction::Confirm, &items);
        assert!(matches!(result, ListInputResult::Consumed));
    }

    // === Toggle (multi-select) ===

    #[test]
    fn toggle_without_multi_select() {
        let mut s = state_with_height(10);
        let items = make_items(5);
        let result = s.handle_input(&InputAction::Toggle, &items);
        assert!(matches!(result, ListInputResult::Unhandled));
        assert!(s.selected.is_empty());
    }

    #[test]
    fn toggle_with_multi_select() {
        let mut s = StandardListState::new(StandardListConfig { multi_select: true });
        s.set_visible_height(10);
        let items = make_items(5);

        let result = s.handle_input(&InputAction::Toggle, &items);
        assert!(matches!(result, ListInputResult::Toggled));
        assert!(s.selected.contains(&0));

        // Toggle again → deselect
        let result = s.handle_input(&InputAction::Toggle, &items);
        assert!(matches!(result, ListInputResult::Toggled));
        assert!(!s.selected.contains(&0));
    }

    // === Wizard state ===

    #[test]
    fn z_on_item_with_popup() {
        let mut s = state_with_height(10);
        let items = vec![TestItem {
            label: "test".into(),
            wizard_offer: Some(WizardOffer::Popup(vec![Line::raw("info")])),
        }];

        s.handle_input(&InputAction::Char('z'), &items);
        assert_eq!(s.wizard_state, WizardState::ShowingPopup);

        // Z again → dismiss
        s.handle_input(&InputAction::Char('z'), &items);
        assert_eq!(s.wizard_state, WizardState::Idle);
    }

    #[test]
    fn z_on_item_with_pane_shifts_focus() {
        let mut s = state_with_height(10);
        let items = vec![TestItem {
            label: "test".into(),
            wizard_offer: Some(WizardOffer::Pane {
                title: "Detail".into(),
                lines: vec![Line::raw("info")],
            }),
        }];

        s.handle_input(&InputAction::Char('z'), &items);
        assert_eq!(s.wizard_state, WizardState::ShowingPane);
        assert_eq!(s.focus, ListFocus::WizardPane);
    }

    #[test]
    fn z_in_pane_dismisses() {
        let mut s = state_with_height(10);
        let items = vec![TestItem {
            label: "test".into(),
            wizard_offer: Some(WizardOffer::Pane {
                title: "Detail".into(),
                lines: vec![Line::raw("info")],
            }),
        }];

        // Enter pane.
        s.handle_input(&InputAction::Char('z'), &items);
        assert_eq!(s.focus, ListFocus::WizardPane);

        // Z again → dismiss.
        s.handle_input(&InputAction::Char('z'), &items);
        assert_eq!(s.wizard_state, WizardState::Idle);
        assert_eq!(s.focus, ListFocus::List);
    }

    #[test]
    fn cancel_in_pane_returns_to_list() {
        let mut s = state_with_height(10);
        s.wizard_state = WizardState::ShowingPane;
        s.focus = ListFocus::WizardPane;
        let items = make_items(5);

        s.handle_input(&InputAction::Cancel, &items);
        assert_eq!(s.wizard_state, WizardState::Idle);
        assert_eq!(s.focus, ListFocus::List);
    }

    #[test]
    fn cursor_move_dismisses_wizard() {
        let mut s = state_with_height(10);
        s.wizard_state = WizardState::ShowingPopup;
        let items = make_items(5);

        s.handle_input(&InputAction::NavDown, &items);
        assert_eq!(s.wizard_state, WizardState::Idle);
    }

    // === Unhandled ===

    #[test]
    fn unhandled_actions_pass_through() {
        let mut s = state_with_height(10);
        let items = make_items(5);
        let result = s.handle_input(&InputAction::Cancel, &items);
        assert!(matches!(result, ListInputResult::Unhandled));
    }

    // === Reset ===

    #[test]
    fn reset_clears_everything() {
        let mut s = state_with_height(10);
        s.cursor = 5;
        s.scroll = 3;
        s.selected.insert(2);
        s.wizard_state = WizardState::ShowingPopup;
        s.focus = ListFocus::WizardPane;

        s.reset();
        assert_eq!(s.cursor, 0);
        assert_eq!(s.scroll, 0);
        assert!(s.selected.is_empty());
        assert_eq!(s.wizard_state, WizardState::Idle);
        assert_eq!(s.focus, ListFocus::List);
    }

    // === Click ===

    #[test]
    fn click_sets_cursor() {
        let mut s = state_with_height(10);
        let items = make_items(5);
        s.click_targets.set_list_area(Rect::new(0, 0, 80, 20));
        s.click_targets.add_row("3", 3);

        let result = s.handle_click(10, 3, &items);
        assert_eq!(result, Some(3));
        assert_eq!(s.cursor, 3);
    }

    // === Header skipping ===

    enum MixedItem {
        Header,
        Entry(String),
    }

    impl WizardItem for MixedItem {
        fn wizard(&self, _: u16) -> Option<WizardOffer> {
            None
        }
    }

    impl ListEntry for MixedItem {
        type Action = String;
        fn on_confirm(&self, _: &BTreeSet<usize>) -> Option<String> {
            match self {
                Self::Header => None,
                Self::Entry(s) => Some(s.clone()),
            }
        }
        fn is_selectable(&self) -> bool {
            matches!(self, Self::Entry(_))
        }
    }

    fn mixed_items() -> Vec<MixedItem> {
        vec![
            MixedItem::Header,        // 0
            MixedItem::Entry("a".into()), // 1
            MixedItem::Entry("b".into()), // 2
            MixedItem::Header,        // 3
            MixedItem::Entry("c".into()), // 4
        ]
    }

    #[test]
    fn nav_down_skips_header() {
        let mut s = state_with_height(10);
        s.cursor = 2; // on "b"
        let items = mixed_items();
        s.handle_input(&InputAction::NavDown, &items);
        assert_eq!(s.cursor, 4); // skips header at 3, lands on "c"
    }

    #[test]
    fn nav_up_skips_header() {
        let mut s = state_with_height(10);
        s.cursor = 4; // on "c"
        let items = mixed_items();
        s.handle_input(&InputAction::NavUp, &items);
        assert_eq!(s.cursor, 2); // skips header at 3, lands on "b"
    }

    #[test]
    fn home_skips_leading_header() {
        let mut s = state_with_height(10);
        s.cursor = 4;
        let items = mixed_items();
        s.handle_input(&InputAction::Home, &items);
        assert_eq!(s.cursor, 1); // skips header at 0
    }

    #[test]
    fn clamp_cursor_finds_selectable() {
        let mut s = state_with_height(10);
        s.cursor = 3; // on header
        let items = mixed_items();
        s.clamp_cursor(&items);
        assert_eq!(s.cursor, 4); // nearest selectable forward
    }

    #[test]
    fn click_skips_non_selectable() {
        let mut s = state_with_height(10);
        let items = mixed_items();
        s.click_targets.set_list_area(Rect::new(0, 0, 80, 20));
        s.click_targets.add_row("0", 0); // header
        s.click_targets.add_row("1", 1); // entry

        // Click on header — should not select
        let result = s.handle_click(10, 0, &items);
        assert_eq!(result, None);

        // Click on entry — should select
        let result = s.handle_click(10, 1, &items);
        assert_eq!(result, Some(1));
    }
}
