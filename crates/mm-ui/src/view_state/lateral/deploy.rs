//! Deploy view state: data + interaction bundled.
//!
//! Tab selection + per-tab scroll position. Data is either "up to date" or
//! a full preview with tabbed signal lists.

use mm_meta::views::cluster_deploy::DeployModalData;

use crate::domain_types::DeployTab;
use crate::input::InputAction;
use crate::route::DeployRoute;
use crate::view_state::ViewCore;

// ============================================================================
// DeployViewData — server-fetched data
// ============================================================================

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
                selected_path_static(
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
}

/// Determine which tab to start on based on data content.
pub fn initial_tab(cached_data: &DeployModalData) -> DeployTab {
    if !cached_data.new.is_empty() {
        DeployTab::New
    } else {
        DeployTab::Healthy
    }
}

/// Path of the currently selected item given raw cached data + position.
pub fn selected_path_static(
    cached_data: &DeployModalData,
    active_tab: DeployTab,
    scroll: usize,
) -> Option<&str> {
    match active_tab {
        DeployTab::Healthy => cached_data
            .healthy
            .get(scroll)
            .map(|f| f.corpus_path.as_str()),
        DeployTab::New => cached_data
            .new_by_dir
            .get(scroll)
            .map(|d| d.directory.as_str()),
        DeployTab::Conflicts => cached_data
            .conflicts
            .get(scroll)
            .map(|c| c.deploy_path.as_str()),
        DeployTab::Leftover => cached_data
            .leftover_by_dir
            .get(scroll)
            .map(|d| d.directory.as_str()),
        DeployTab::Stale => cached_data
            .stale
            .get(scroll)
            .map(|f| f.library_path.as_str()),
    }
}

// ============================================================================
// DeployInteraction — UI navigation state
// ============================================================================

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

    /// Handle input in Preview mode with max_scroll context.
    /// Returns `Some(Confirm)` on Enter, `None` for navigation/unhandled.
    fn handle_input_preview(
        &mut self,
        action: &InputAction,
        max_scroll: usize,
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
                if self.tab_scroll[idx] < max_scroll {
                    self.tab_scroll[idx] += 1;
                }
                None
            }
            InputAction::PageUp => {
                self.tab_scroll[idx] = self.tab_scroll[idx].saturating_sub(10);
                None
            }
            InputAction::PageDown => {
                self.tab_scroll[idx] = (self.tab_scroll[idx] + 10).min(max_scroll);
                None
            }
            _ => None,
        }
    }
}

// ============================================================================
// DeployViewState — bundled data + interaction
// ============================================================================

/// Complete view state for the deploy view.
pub struct DeployViewState {
    pub data: DeployViewData,
    pub interaction: DeployInteraction,
}

impl DeployViewState {
    /// Create for preview mode with data.
    pub fn preview(cached_data: DeployModalData) -> Self {
        let tab = initial_tab(&cached_data);
        Self {
            data: DeployViewData::Preview { cached_data },
            interaction: DeployInteraction::new_with_tab(tab),
        }
    }

    /// Create for up-to-date mode.
    pub fn up_to_date(library_file_counts: Vec<(String, usize)>) -> Self {
        Self {
            data: DeployViewData::UpToDate { library_file_counts },
            interaction: DeployInteraction::new(),
        }
    }

    /// Handle a semantic input action.
    pub fn handle_input(&mut self, action: &InputAction) -> Option<DeployAction> {
        match &self.data {
            DeployViewData::UpToDate { .. } => None,
            DeployViewData::Preview { .. } => {
                let max_scroll = self.data.max_scroll_for_tab(self.interaction.active_tab);
                self.interaction.handle_input_preview(action, max_scroll)
            }
        }
    }

    /// Route serialization.
    pub fn to_route(&self) -> DeployRoute {
        self.interaction.to_route()
    }

    /// Restore tab/scroll position from a route.
    pub fn apply_route(&mut self, route: &DeployRoute) {
        if let Some(tab) = route.tab {
            self.interaction.active_tab = tab;
        }
        if let Some(scroll) = route.scroll {
            self.interaction.tab_scroll[self.interaction.active_tab.index()] = scroll;
        }
    }
}
