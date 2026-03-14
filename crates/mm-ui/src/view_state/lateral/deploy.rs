//! Deploy view interaction state.
//!
//! Tab selection + per-tab scroll position. DeployTab already lives in mm-ui.

use crate::domain_types::DeployTab;
use crate::input::InputAction;
use crate::route::DeployRoute;
use crate::view_state::ViewCore;

/// Interaction state for the deploy view.
pub struct DeployInteraction {
    pub active_tab: DeployTab,
    pub tab_scroll: [usize; 5],
}

/// Action produced by the deploy view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployAction {
    /// User confirmed deployment in Preview mode.
    Confirm,
}

/// Data context for input handling.
pub struct DeployInputCtx {
    /// Max scroll position for the current tab (data-dependent).
    pub max_scroll: usize,
}

impl ViewCore for DeployInteraction {
    type Route = DeployRoute;
    type Action = DeployAction;
    type Data = ();

    fn from_route(route: &DeployRoute) -> Self {
        let active_tab = route.tab.unwrap_or_default();
        let mut tab_scroll = [0usize; 5];
        if let Some(scroll) = route.scroll {
            tab_scroll[active_tab.index()] = scroll;
        }
        Self {
            active_tab,
            tab_scroll,
        }
    }

    fn to_route(&self) -> DeployRoute {
        DeployRoute {
            tab: if self.active_tab == DeployTab::Healthy {
                None
            } else {
                Some(self.active_tab)
            },
            scroll: {
                let s = self.tab_scroll[self.active_tab.index()];
                if s > 0 { Some(s) } else { None }
            },
        }
    }

    fn handle_input(&mut self, _action: &InputAction, _data: &()) -> Option<DeployAction> {
        None
    }
}

impl DeployInteraction {
    /// Create default interaction (Healthy tab, no scroll).
    pub fn new() -> Self {
        Self {
            active_tab: DeployTab::default(),
            tab_scroll: [0; 5],
        }
    }

    /// Create with a specific initial tab based on data availability.
    pub fn new_with_tab(tab: DeployTab) -> Self {
        Self {
            active_tab: tab,
            tab_scroll: [0; 5],
        }
    }

    /// Handle input in Preview mode with data context.
    /// Returns `Some(Confirm)` on Enter, `None` for navigation/unhandled.
    pub fn handle_input_preview(
        &mut self,
        action: &InputAction,
        ctx: &DeployInputCtx,
    ) -> Option<DeployAction> {
        let idx = self.active_tab.index();
        match action {
            InputAction::Confirm => Some(DeployAction::Confirm),
            InputAction::NavLeft => {
                self.active_tab = self.active_tab.prev();
                None
            }
            InputAction::NavRight => {
                self.active_tab = self.active_tab.next();
                None
            }
            InputAction::NavUp => {
                self.tab_scroll[idx] = self.tab_scroll[idx].saturating_sub(1);
                None
            }
            InputAction::NavDown => {
                if self.tab_scroll[idx] < ctx.max_scroll {
                    self.tab_scroll[idx] += 1;
                }
                None
            }
            InputAction::PageUp => {
                self.tab_scroll[idx] = self.tab_scroll[idx].saturating_sub(10);
                None
            }
            InputAction::PageDown => {
                self.tab_scroll[idx] = (self.tab_scroll[idx] + 10).min(ctx.max_scroll);
                None
            }
            _ => None,
        }
    }
}
