//! Tree Browser Module
//!
//! Unified tree browser with variant modes. Currently only CorpusBrowser exists.
//! Uses DirectoryBrowser (mm-ui) for backend-agnostic tree navigation, with
//! variant-specific marker lookups for deploy/packing/dimming.

mod actions;
mod entry;
mod input;
mod render;
pub mod variants;

use std::collections::HashSet;

use mm_ui::directory_browser::DirectoryBrowser;

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::input::InputAction;
use crate::widgets::{AlbumArtCache, AlbumArtPicker};

pub use actions::TreeBrowserAction;
pub use entry::{DeployMarker, PackingMarker};
pub use variants::{BrowserVariant, CorpusBrowserVariant};

/// Unified tree browser state.
///
/// Instantiate with a specific variant - no unvarianted mode exists.
#[derive(Debug)]
pub struct TreeBrowserState {
    /// Backend-agnostic tree browser (entries, cursor, scroll, search).
    pub(crate) browser: DirectoryBrowser,
    /// Variant-specific state and behavior
    variant: BrowserVariant,
    /// Click targets for tree entries (set during render).
    pub click_targets: crate::widgets::ListClickTargets,
    /// Pending browser action from last handle_input (e.g. RequestExpand).
    /// Consumed by the app layer after handle_input returns.
    pending_browser_action: Option<mm_ui::directory_browser::BrowserAction>,
}

impl TreeBrowserState {
    /// Create a CorpusBrowser for browsing files and directories.
    ///
    /// `corpus_dir_rel` is the corpus directory relative to archive root (e.g. "corpus").
    /// `deploy_source_dirs` are relative paths for deploy source markers.
    /// `primary_zone_dirs` are relative paths for dimming non-zone entries.
    pub fn corpus_browser(
        corpus_dir: std::path::PathBuf,
        corpus_dir_rel: String,
        deploy_source_dirs: Vec<String>,
        primary_zone_dirs: Vec<String>,
    ) -> Self {
        let browser = DirectoryBrowser::new("corpus");
        let variant = BrowserVariant::CorpusBrowser(CorpusBrowserVariant::new(
            corpus_dir,
            corpus_dir_rel,
            deploy_source_dirs,
            primary_zone_dirs,
        ));

        Self {
            browser,
            variant,
            click_targets: Default::default(),
            pending_browser_action: None,
        }
    }

    /// Handle a semantic input action.
    ///
    /// Returns the domain action (for the app-level action handler).
    /// Browser-level actions (RequestExpand, Collapse) are handled internally
    /// or stored as pending for the app layer to dispatch via `take_pending_browser_action()`.
    pub fn handle_input(&mut self, action: &InputAction) -> Option<TreeBrowserAction> {
        let result = input::handle_input(action, &mut self.browser, &mut self.variant);
        if let Some(ba) = result.browser_action {
            match ba {
                mm_ui::directory_browser::BrowserAction::Collapse => {
                    self.browser.collapse_at_cursor();
                }
                other => {
                    self.pending_browser_action = Some(other);
                }
            }
        }
        result.tree_action
    }

    /// Take the pending browser action, if any (e.g. RequestExpand after Right arrow).
    ///
    /// The app layer should call this after handle_input and dispatch the
    /// appropriate protocol query if it returns Some.
    pub fn take_pending_browser_action(&mut self) -> Option<mm_ui::directory_browser::BrowserAction> {
        self.pending_browser_action.take()
    }

    /// Render the browser.
    pub fn render(
        &mut self,
        f: &mut Frame,
        area: Rect,
        art_picker: &mut AlbumArtPicker,
        art_cache: &mut AlbumArtCache,
        resolver: &mm_meta::paths::PathResolver,
    ) {
        render::render(
            f,
            area,
            &mut self.browser,
            &mut self.variant,
            art_picker,
            art_cache,
            &mut self.click_targets,
            resolver,
        );
    }

    /// Get the relative path of the currently selected entry (if any).
    pub fn selected_path(&self) -> Option<&str> {
        self.browser.current_entry().map(|e| e.path.as_str())
    }

    /// Get a reference to the DirectoryBrowser.
    pub fn browser(&self) -> &DirectoryBrowser {
        &self.browser
    }

    /// Get a mutable reference to the DirectoryBrowser.
    pub fn browser_mut(&mut self) -> &mut DirectoryBrowser {
        &mut self.browser
    }

    /// Get a reference to the variant.
    pub fn variant(&self) -> &BrowserVariant {
        &self.variant
    }

    /// Get a mutable reference to the variant.
    pub fn variant_mut(&mut self) -> &mut BrowserVariant {
        &mut self.variant
    }

    // =========================================================================
    // Config Panel
    // =========================================================================

    /// Set the config panel state and switch focus to it.
    pub fn set_config_panel(&mut self, panel: variants::corpus::DirConfigPanelState) {
        let BrowserVariant::CorpusBrowser(ref mut v) = self.variant;
        v.config_panel = Some(panel);
        v.set_focus_config_panel();
    }

    /// Clear the config panel and return focus to tree.
    pub fn clear_config_panel(&mut self) {
        let BrowserVariant::CorpusBrowser(ref mut v) = self.variant;
        v.config_panel = None;
        v.set_focus_tree();
    }

    /// Get a reference to the config panel state, if open.
    pub fn config_panel(&self) -> Option<&variants::corpus::DirConfigPanelState> {
        let BrowserVariant::CorpusBrowser(ref v) = self.variant;
        v.config_panel.as_ref()
    }

    /// Handle mouse click for cursor selection.
    pub fn handle_click(&mut self, x: u16, y: u16) {
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                self.browser.set_cursor(idx);
            }
        }
    }

    /// Set pending edit paths for visual markers.
    pub fn set_pending_edit_paths(&mut self, paths: HashSet<std::path::PathBuf>) {
        let BrowserVariant::CorpusBrowser(ref mut v) = self.variant;
        v.set_pending_edit_paths(paths);
    }
}
