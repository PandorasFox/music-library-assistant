//! Wizard system: data trait + types for item-level info containers.
//!
//! The wizard system provides two display modes for contextual item information:
//! - **Popup**: Small floating box anchored near the cursor row
//! - **Pane**: Full scrollable right-side panel with its own focus
//!
//! Items declare what they offer via `WizardItem` (derived with `#[derive(WizardItem)]`).
//! The Z key cycles through available containers based on `WizardOffer` shape.
//!
//! This module is purely about data declaration — no rendering, no behavior.
//! Rendering lives in `wizard_popup` and `wizard_pane`. Behavioral composition
//! happens at the `StandardList` level via `T: WizardItem + ListEntry` bounds.

use ratatui::text::Line;

/// Guaranteed-non-empty wizard content. The enum shape enforces that if you
/// return a `WizardOffer`, you've provided at least one container.
///
/// All data is owned (no lifetime params) for derive macro ergonomics.
#[derive(Debug, Clone)]
pub enum WizardOffer {
    /// Small popup only. Z shows it, Z again dismisses.
    Popup(Vec<Line<'static>>),

    /// Full pane only. Z opens pane directly.
    Pane {
        title: String,
        lines: Vec<Line<'static>>,
    },

    /// Both containers. Z shows popup first, second Z escalates to pane.
    Both {
        popup: Vec<Line<'static>>,
        pane_title: String,
        pane_lines: Vec<Line<'static>>,
    },
}

/// Data trait: what wizard info containers does this item declare?
///
/// Derived via `#[derive(WizardItem)]`. Independent of `ListEntry`.
///
/// - `None` = this specific item has no wizard (header rows, separators)
/// - `Some` = at least one container, enforced by `WizardOffer`'s shape
pub trait WizardItem {
    fn wizard(&self, width: u16) -> Option<WizardOffer>;
}

/// Z-key state machine. Driven mechanically by `WizardOffer` variant.
/// Lives in `StandardListState` (or directly in tree browser variants).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WizardState {
    #[default]
    Idle,
    ShowingPopup,
    ShowingPane,
}

impl WizardState {
    /// Advance state on Z press, given the current item's offer.
    pub fn advance(&mut self, offer: &WizardOffer) {
        *self = match (*self, offer) {
            // From Idle: enter the first available container
            (Self::Idle, WizardOffer::Popup(_)) => Self::ShowingPopup,
            (Self::Idle, WizardOffer::Pane { .. }) => Self::ShowingPane,
            (Self::Idle, WizardOffer::Both { .. }) => Self::ShowingPopup,

            // From Popup: dismiss if popup-only, escalate if Both
            (Self::ShowingPopup, WizardOffer::Popup(_)) => Self::Idle,
            (Self::ShowingPopup, WizardOffer::Both { .. }) => Self::ShowingPane,

            // From Pane: always dismiss
            (Self::ShowingPane, _) => Self::Idle,

            // Defensive: any other combination → Idle
            _ => Self::Idle,
        };
    }

    /// Cursor moved or Esc pressed — dismiss any active wizard.
    pub fn dismiss(&mut self) {
        *self = Self::Idle;
    }

    pub fn is_idle(self) -> bool {
        self == Self::Idle
    }

    pub fn is_showing_pane(self) -> bool {
        self == Self::ShowingPane
    }

    pub fn is_showing_popup(self) -> bool {
        self == Self::ShowingPopup
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn popup_offer() -> WizardOffer {
        WizardOffer::Popup(vec![Line::raw("test")])
    }

    fn pane_offer() -> WizardOffer {
        WizardOffer::Pane {
            title: "Title".into(),
            lines: vec![Line::raw("detail")],
        }
    }

    fn both_offer() -> WizardOffer {
        WizardOffer::Both {
            popup: vec![Line::raw("summary")],
            pane_title: "Detail".into(),
            pane_lines: vec![Line::raw("full detail")],
        }
    }

    // === advance() transition table ===

    #[test]
    fn idle_popup_shows_popup() {
        let mut s = WizardState::Idle;
        s.advance(&popup_offer());
        assert_eq!(s, WizardState::ShowingPopup);
    }

    #[test]
    fn idle_pane_shows_pane() {
        let mut s = WizardState::Idle;
        s.advance(&pane_offer());
        assert_eq!(s, WizardState::ShowingPane);
    }

    #[test]
    fn idle_both_shows_popup() {
        let mut s = WizardState::Idle;
        s.advance(&both_offer());
        assert_eq!(s, WizardState::ShowingPopup);
    }

    #[test]
    fn popup_popup_dismisses() {
        let mut s = WizardState::ShowingPopup;
        s.advance(&popup_offer());
        assert_eq!(s, WizardState::Idle);
    }

    #[test]
    fn popup_both_escalates_to_pane() {
        let mut s = WizardState::ShowingPopup;
        s.advance(&both_offer());
        assert_eq!(s, WizardState::ShowingPane);
    }

    #[test]
    fn pane_popup_dismisses() {
        let mut s = WizardState::ShowingPane;
        s.advance(&popup_offer());
        assert_eq!(s, WizardState::Idle);
    }

    #[test]
    fn pane_pane_dismisses() {
        let mut s = WizardState::ShowingPane;
        s.advance(&pane_offer());
        assert_eq!(s, WizardState::Idle);
    }

    #[test]
    fn pane_both_dismisses() {
        let mut s = WizardState::ShowingPane;
        s.advance(&both_offer());
        assert_eq!(s, WizardState::Idle);
    }

    #[test]
    fn popup_with_pane_offer_defensive() {
        // ShowingPopup with a Pane-only offer shouldn't happen in practice,
        // but the defensive arm returns Idle.
        let mut s = WizardState::ShowingPopup;
        s.advance(&pane_offer());
        assert_eq!(s, WizardState::Idle);
    }

    // === dismiss() ===

    #[test]
    fn dismiss_from_idle() {
        let mut s = WizardState::Idle;
        s.dismiss();
        assert_eq!(s, WizardState::Idle);
    }

    #[test]
    fn dismiss_from_popup() {
        let mut s = WizardState::ShowingPopup;
        s.dismiss();
        assert_eq!(s, WizardState::Idle);
    }

    #[test]
    fn dismiss_from_pane() {
        let mut s = WizardState::ShowingPane;
        s.dismiss();
        assert_eq!(s, WizardState::Idle);
    }

    // === Predicate helpers ===

    #[test]
    fn predicates() {
        assert!(WizardState::Idle.is_idle());
        assert!(!WizardState::Idle.is_showing_popup());
        assert!(!WizardState::Idle.is_showing_pane());

        assert!(WizardState::ShowingPopup.is_showing_popup());
        assert!(WizardState::ShowingPane.is_showing_pane());
    }
}
