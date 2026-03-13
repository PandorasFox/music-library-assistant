//! ModalButtons: trait-based button abstraction for modal dialogs.
//!
//! Provides a generic trait that button enums implement, replacing per-modal
//! left/right navigation, rendering, and action dispatch boilerplate.
//!
//! # Example
//!
//! ```ignore
//! #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
//! enum MyButton {
//!     DoThing,
//!     #[default]
//!     Cancel,
//! }
//!
//! impl ModalButtons for MyButton {
//!     type Context = MyModalData;
//!     type Action = MyAction;
//!
//!     fn all() -> &'static [Self] {
//!         &[Self::DoThing, Self::Cancel]
//!     }
//!
//!     fn label(&self, ctx: &Self::Context) -> Cow<'static, str> {
//!         match self {
//!             Self::DoThing => format!("Do {} things", ctx.count).into(),
//!             Self::Cancel => "Cancel".into(),
//!         }
//!     }
//!
//!     fn color(&self, _ctx: &Self::Context) -> Color {
//!         match self {
//!             Self::DoThing => Color::Green,
//!             Self::Cancel => Color::White,
//!         }
//!     }
//!
//!     fn enabled(&self, ctx: &Self::Context) -> bool {
//!         match self {
//!             Self::DoThing => ctx.count > 0,
//!             Self::Cancel => true,
//!         }
//!     }
//!
//!     fn action(&self, _ctx: &Self::Context) -> Self::Action {
//!         match self {
//!             Self::DoThing => MyAction::Confirm,
//!             Self::Cancel => MyAction::Cancel,
//!         }
//!     }
//! }
//! ```

use std::borrow::Cow;

use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::Frame;

use super::modal::{render_button_row, ConfirmationButton};
use super::resolution_layout::ButtonRects;

/// Trait for modal button enums. Provides labels, colors, enablement,
/// and action mapping — all parameterized by a `Context` type that carries
/// whatever data the buttons need for dynamic behavior.
pub trait ModalButtons: Default + Copy + PartialEq + 'static {
    /// Data needed to compute labels, colors, and enablement.
    type Context;
    /// Action produced when a button is confirmed.
    type Action;

    /// All button variants in left-to-right display order.
    fn all() -> &'static [Self];

    /// Display label for this button. May be dynamic based on context.
    fn label(&self, ctx: &Self::Context) -> Cow<'static, str>;

    /// Accent color for this button.
    fn color(&self, ctx: &Self::Context) -> Color;

    /// Whether this button is currently selectable.
    /// Disabled buttons are skipped during navigation and grayed out.
    fn enabled(&self, ctx: &Self::Context) -> bool;

    /// The action produced when this button is confirmed.
    fn action(&self, ctx: &Self::Context) -> Self::Action;
}

/// Persistent state for a modal button row.
#[derive(Debug, Clone)]
pub struct ButtonRowState<B: ModalButtons> {
    pub selected: B,
    pub button_rects: ButtonRects,
}

impl<B: ModalButtons> Default for ButtonRowState<B> {
    fn default() -> Self {
        Self {
            selected: B::default(),
            button_rects: ButtonRects::new(),
        }
    }
}

impl<B: ModalButtons> ButtonRowState<B> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Navigate left, skipping disabled buttons.
    pub fn nav_left(&mut self, ctx: &B::Context) {
        let all = B::all();
        let Some(current_idx) = all.iter().position(|b| *b == self.selected) else {
            return;
        };
        // Scan leftward for the next enabled button.
        for i in (0..current_idx).rev() {
            if all[i].enabled(ctx) {
                self.selected = all[i];
                return;
            }
        }
        // No enabled button to the left — stay put.
    }

    /// Navigate right, skipping disabled buttons.
    pub fn nav_right(&mut self, ctx: &B::Context) {
        let all = B::all();
        let Some(current_idx) = all.iter().position(|b| *b == self.selected) else {
            return;
        };
        // Scan rightward for the next enabled button.
        for i in (current_idx + 1)..all.len() {
            if all[i].enabled(ctx) {
                self.selected = all[i];
                return;
            }
        }
        // No enabled button to the right — stay put.
    }

    /// Ensure the selected button is enabled. If not, find the nearest enabled one
    /// (preferring rightward, then leftward). Call after context changes.
    pub fn clamp(&mut self, ctx: &B::Context) {
        if self.selected.enabled(ctx) {
            return;
        }
        let all = B::all();
        let current_idx = all.iter().position(|b| *b == self.selected).unwrap_or(0);
        // Try rightward first.
        for i in (current_idx + 1)..all.len() {
            if all[i].enabled(ctx) {
                self.selected = all[i];
                return;
            }
        }
        // Then leftward.
        for i in (0..current_idx).rev() {
            if all[i].enabled(ctx) {
                self.selected = all[i];
                return;
            }
        }
        // All disabled — keep current (render will show all grayed out).
    }

    /// Get the action for the currently selected button, if it's enabled.
    pub fn confirm(&self, ctx: &B::Context) -> Option<B::Action> {
        if self.selected.enabled(ctx) {
            Some(self.selected.action(ctx))
        } else {
            None
        }
    }

    /// Handle a click hit-test against stored button rects.
    /// Returns the action if a click hit an enabled button.
    pub fn handle_click(&mut self, x: u16, y: u16, ctx: &B::Context) -> Option<B::Action> {
        let all = B::all();
        if let Some(name) = self.button_rects.hit_test(x, y) {
            if let Some(idx) = name.parse::<usize>().ok() {
                if let Some(&button) = all.get(idx) {
                    if button.enabled(ctx) {
                        self.selected = button;
                        return Some(button.action(ctx));
                    }
                }
            }
        }
        None
    }

    /// Render the button row and update click target rects.
    ///
    /// `focused` controls whether the selected button is visually highlighted.
    pub fn render(&mut self, f: &mut Frame, area: Rect, ctx: &B::Context, focused: bool) {
        let all = B::all();
        self.button_rects.clear();

        let buttons: Vec<ConfirmationButton> = all
            .iter()
            .enumerate()
            .filter(|(_, b)| b.enabled(ctx))
            .map(|(i, b)| {
                let is_selected = focused && *b == self.selected;
                // Store index as the button rect key for click detection.
                // We'll set the actual rect after rendering.
                let _ = i; // used below in rect registration
                ConfirmationButton::new(b.label(ctx).into_owned(), b.color(ctx))
                    .selected(is_selected)
            })
            .collect();

        render_button_row(f, area, &buttons);

        // Register button rects for click detection.
        // render_button_row centers buttons — we need to compute the rects.
        // For now, register the full area per enabled button with even splits.
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
                self.button_rects.set(idx.to_string(), btn_rect);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum TestButton {
        Action,
        #[default]
        Cancel,
    }

    struct TestCtx {
        has_items: bool,
    }

    impl ModalButtons for TestButton {
        type Context = TestCtx;
        type Action = &'static str;

        fn all() -> &'static [Self] {
            &[Self::Action, Self::Cancel]
        }

        fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
            match self {
                Self::Action => "Do It".into(),
                Self::Cancel => "Cancel".into(),
            }
        }

        fn color(&self, _ctx: &Self::Context) -> Color {
            match self {
                Self::Action => Color::Green,
                Self::Cancel => Color::White,
            }
        }

        fn enabled(&self, ctx: &Self::Context) -> bool {
            match self {
                Self::Action => ctx.has_items,
                Self::Cancel => true,
            }
        }

        fn action(&self, _ctx: &Self::Context) -> &'static str {
            match self {
                Self::Action => "confirmed",
                Self::Cancel => "cancelled",
            }
        }
    }

    #[test]
    fn nav_right_skips_to_end() {
        let ctx = TestCtx { has_items: true };
        let mut state = ButtonRowState::<TestButton>::new();
        state.selected = TestButton::Action;
        state.nav_right(&ctx);
        assert_eq!(state.selected, TestButton::Cancel);
    }

    #[test]
    fn nav_right_at_end_stays() {
        let ctx = TestCtx { has_items: true };
        let mut state = ButtonRowState::<TestButton>::new();
        state.selected = TestButton::Cancel;
        state.nav_right(&ctx);
        assert_eq!(state.selected, TestButton::Cancel);
    }

    #[test]
    fn nav_left_skips_disabled() {
        let ctx = TestCtx { has_items: false };
        let mut state = ButtonRowState::<TestButton>::new();
        state.selected = TestButton::Cancel;
        state.nav_left(&ctx);
        // Action is disabled, so we stay on Cancel.
        assert_eq!(state.selected, TestButton::Cancel);
    }

    #[test]
    fn nav_left_to_enabled() {
        let ctx = TestCtx { has_items: true };
        let mut state = ButtonRowState::<TestButton>::new();
        state.selected = TestButton::Cancel;
        state.nav_left(&ctx);
        assert_eq!(state.selected, TestButton::Action);
    }

    #[test]
    fn clamp_moves_off_disabled() {
        let ctx = TestCtx { has_items: false };
        let mut state = ButtonRowState::<TestButton>::new();
        state.selected = TestButton::Action;
        state.clamp(&ctx);
        assert_eq!(state.selected, TestButton::Cancel);
    }

    #[test]
    fn clamp_stays_on_enabled() {
        let ctx = TestCtx { has_items: true };
        let mut state = ButtonRowState::<TestButton>::new();
        state.selected = TestButton::Action;
        state.clamp(&ctx);
        assert_eq!(state.selected, TestButton::Action);
    }

    #[test]
    fn confirm_disabled_returns_none() {
        let ctx = TestCtx { has_items: false };
        let state = ButtonRowState::<TestButton> {
            selected: TestButton::Action,
            button_rects: ButtonRects::new(),
        };
        assert!(state.confirm(&ctx).is_none());
    }

    #[test]
    fn confirm_enabled_returns_action() {
        let ctx = TestCtx { has_items: true };
        let state = ButtonRowState::<TestButton> {
            selected: TestButton::Action,
            button_rects: ButtonRects::new(),
        };
        assert_eq!(state.confirm(&ctx), Some("confirmed"));
    }

    // Three-button test to verify skip-over behavior.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum ThreeButton {
        Left,
        Middle,
        #[default]
        Right,
    }

    struct ThreeCtx {
        middle_enabled: bool,
    }

    impl ModalButtons for ThreeButton {
        type Context = ThreeCtx;
        type Action = &'static str;

        fn all() -> &'static [Self] {
            &[Self::Left, Self::Middle, Self::Right]
        }

        fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
            match self {
                Self::Left => "Left".into(),
                Self::Middle => "Middle".into(),
                Self::Right => "Right".into(),
            }
        }

        fn color(&self, _ctx: &Self::Context) -> Color {
            Color::White
        }

        fn enabled(&self, ctx: &Self::Context) -> bool {
            match self {
                Self::Middle => ctx.middle_enabled,
                _ => true,
            }
        }

        fn action(&self, _ctx: &Self::Context) -> &'static str {
            match self {
                Self::Left => "left",
                Self::Middle => "middle",
                Self::Right => "right",
            }
        }
    }

    #[test]
    fn nav_right_skips_disabled_middle() {
        let ctx = ThreeCtx { middle_enabled: false };
        let mut state = ButtonRowState::<ThreeButton>::new();
        state.selected = ThreeButton::Left;
        state.nav_right(&ctx);
        assert_eq!(state.selected, ThreeButton::Right);
    }

    #[test]
    fn nav_left_skips_disabled_middle() {
        let ctx = ThreeCtx { middle_enabled: false };
        let mut state = ButtonRowState::<ThreeButton>::new();
        state.selected = ThreeButton::Right;
        state.nav_left(&ctx);
        assert_eq!(state.selected, ThreeButton::Left);
    }

    #[test]
    fn nav_through_enabled_middle() {
        let ctx = ThreeCtx { middle_enabled: true };
        let mut state = ButtonRowState::<ThreeButton>::new();
        state.selected = ThreeButton::Left;
        state.nav_right(&ctx);
        assert_eq!(state.selected, ThreeButton::Middle);
        state.nav_right(&ctx);
        assert_eq!(state.selected, ThreeButton::Right);
    }
}
