//! Corpus Browser Variant
//!
//! Browse files and directories with persistent search bar and tag editor launch.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use mm_meta::signals::packing_category::PackingCategory;
use mm_ui::directory_browser::{BrowserAction, DirectoryBrowser};

use crate::packing_colors::PackingCategoryColor;
use crate::helpers::truncate_left;
use crate::input::InputAction;
use crate::widgets::detail_panel::{
    render_detail_panel, DetailField, DetailPanelParams, DetailWidget, PanelButton,
};
use crate::widgets::TextInputState;

use crate::tree_browser::actions::TreeBrowserAction;
use crate::tree_browser::entry::{DeployMarker, PackingMarker};
use crate::tree_browser::variants::VariantInputResult;
use crate::widgets::wizard::{WizardOffer, WizardState};
use crate::widgets::wizard_pane::WizardPaneState;

/// Focus state for corpus browser
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CorpusBrowserFocus {
    /// Focus is on the tree browser
    #[default]
    TreeBrowser,
    /// Focus is on the search bar
    SearchBar,
    /// Focus is on the config panel
    ConfigPanel,
}

/// Focus within the config panel
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelFocus {
    #[default]
    Fields,
    Buttons,
}

/// Search result state
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    /// Matching paths found (relative String paths)
    pub matches: Vec<String>,
    /// Index in entries list for first match (for auto-scroll)
    pub first_match_idx: Option<usize>,
}

impl SearchState {
    /// Clear search results
    pub fn clear(&mut self) {
        self.matches.clear();
        self.first_match_idx = None;
    }
}

/// Per-field help text for the directory config panel.
const DIR_FIELD_HELP: [&[&str]; 6] = [
    &["TODO"], // 0: Libraries
    &["TODO"], // 1: Can stash dupes
    &["TODO"], // 2: Interior dupes
    &["TODO"], // 3: Path schema
    &["TODO"], // 4: AcoustID lookup
    &["TODO"], // 5: Pinned release
];

/// State for the directory config panel.
#[derive(Debug)]
pub struct DirConfigPanelState {
    /// Which source directory we're editing (relative path).
    pub source_path: PathBuf,
    // Editable copies (None = inherit from parent):
    pub libraries: Vec<String>,
    pub can_stash_dupes: Option<bool>,
    pub interior_dupes: Option<bool>,
    pub path_schema: Option<String>,
    pub enable_acoustid: Option<bool>,
    pub pinned_release: Option<String>,
    // Originals for dirty checking:
    pub orig_libraries: Vec<String>,
    pub orig_can_stash_dupes: Option<bool>,
    pub orig_interior_dupes: Option<bool>,
    pub orig_path_schema: Option<String>,
    pub orig_enable_acoustid: Option<bool>,
    pub orig_pinned_release: Option<String>,
    // UI state:
    /// 0=libraries, 1=can_stash_dupes, 2=interior_dupes, 3=path_schema, 4=enable_acoustid, 5=pinned_release
    pub field_cursor: usize,
    pub focus: PanelFocus,
    pub button_cursor: usize,
    pub lib_cursor: Option<usize>,
    pub text_input: Option<TextInputState>,
    pub wizard_state: WizardState,
}

impl DirConfigPanelState {
    /// Whether the panel has any edits compared to the original values.
    pub fn has_edits(&self) -> bool {
        self.libraries != self.orig_libraries
            || self.can_stash_dupes != self.orig_can_stash_dupes
            || self.interior_dupes != self.orig_interior_dupes
            || self.path_schema != self.orig_path_schema
            || self.enable_acoustid != self.orig_enable_acoustid
            || self.pinned_release != self.orig_pinned_release
    }

    /// Render the config panel.
    pub fn render_config_panel(&self, f: &mut Frame, area: Rect) {
        let fields = vec![
            DetailField {
                label: "Libraries",
                widget: if self.libraries.is_empty() {
                    DetailWidget::Text { value: "(none)", edited: false }
                } else {
                    DetailWidget::StringItems {
                        items: &self.libraries,
                        cursor: self.lib_cursor,
                        edited: self.libraries != self.orig_libraries,
                    }
                },
            },
            DetailField {
                label: "Can stash dupes",
                widget: DetailWidget::OptBool {
                    value: self.can_stash_dupes,
                    edited: self.can_stash_dupes != self.orig_can_stash_dupes,
                },
            },
            DetailField {
                label: "Interior dupes",
                widget: DetailWidget::OptBool {
                    value: self.interior_dupes,
                    edited: self.interior_dupes != self.orig_interior_dupes,
                },
            },
            DetailField {
                label: "Path schema",
                widget: DetailWidget::Text {
                    value: self.path_schema.as_deref().unwrap_or("(inherit)"),
                    edited: self.path_schema != self.orig_path_schema,
                },
            },
            DetailField {
                label: "AcoustID lookup",
                widget: DetailWidget::OptBool {
                    value: self.enable_acoustid,
                    edited: self.enable_acoustid != self.orig_enable_acoustid,
                },
            },
            DetailField {
                label: "Pinned release",
                widget: DetailWidget::Text {
                    value: self.pinned_release.as_deref().unwrap_or("(none)"),
                    edited: self.pinned_release != self.orig_pinned_release,
                },
            },
        ];

        let buttons = vec![
            PanelButton {
                label: "Save",
                color: Color::Green,
                selected: self.focus == PanelFocus::Buttons && self.button_cursor == 0,
            },
            PanelButton {
                label: "Discard",
                color: Color::Red,
                selected: self.focus == PanelFocus::Buttons && self.button_cursor == 1,
            },
        ];

        let params = DetailPanelParams {
            title: &format!("Config: {}", self.source_path.display()),
            border_color: Color::Cyan,
            fields: &fields,
            field_cursor: self.field_cursor,
            buttons: &buttons,
            focus_on_buttons: self.focus == PanelFocus::Buttons,
            hint: None,
        };

        render_detail_panel(f, area, &params);
    }
}

/// Corpus browser variant state.
#[derive(Debug)]
pub struct CorpusBrowserVariant {
    /// Absolute path to corpus directory (for C key guard path comparison)
    corpus_dir: PathBuf,
    /// Relative path to corpus directory from archive root (for DirectoryBrowser relative paths)
    corpus_dir_rel: String,
    /// Current focus (tree browser, search bar, or config panel)
    focus: CorpusBrowserFocus,
    /// Text input state for search bar
    search_input: TextInputState,
    /// Search results
    search: SearchState,
    /// Whether we're in match selection mode (multiple matches on enter)
    match_selection_mode: bool,
    /// Index in matches list when selecting
    match_selection_idx: usize,
    /// Directory config panel state (open when Some)
    pub config_panel: Option<DirConfigPanelState>,
    /// Relative paths of dirs with staged config edits (for [*] marker).
    pending_edit_paths: HashSet<PathBuf>,
    /// Wizard state for Z-key packing info popup/pane.
    wizard_state: WizardState,
    /// Current wizard offer (built from cursor entry's packing marker).
    wizard_offer: Option<WizardOffer>,
    /// Scroll state for wizard pane.
    wizard_pane: WizardPaneState,
    /// Text input state for inline filter bar (Ctrl+/)
    filter_input: TextInputState,
    /// Whether the inline filter bar is actively accepting input
    filter_active: bool,

    // -- Marker lookup data (String relative paths for DirectoryBrowser) --
    /// Configured deployment source directories (relative paths within archive root).
    deploy_source_dirs: Vec<String>,
    /// Primary zone directories (relative paths: corpus root, stash).
    primary_zone_dirs: Vec<String>,
    /// Files with release packing matches (relative paths), refreshed on computations_generation bump.
    packing_file_paths: HashSet<String>,
    /// Directory → best packing category, refreshed on computations_generation bump.
    packing_dir_categories: HashMap<String, PackingCategory>,
}

impl CorpusBrowserVariant {
    /// Create a new corpus browser variant.
    pub fn new(
        corpus_dir: PathBuf,
        corpus_dir_rel: String,
        deploy_source_dirs: Vec<String>,
        primary_zone_dirs: Vec<String>,
    ) -> Self {
        Self {
            corpus_dir,
            corpus_dir_rel,
            focus: CorpusBrowserFocus::TreeBrowser,
            search_input: TextInputState::new(),
            search: SearchState::default(),
            match_selection_mode: false,
            match_selection_idx: 0,
            config_panel: None,
            pending_edit_paths: HashSet::new(),
            wizard_state: WizardState::default(),
            wizard_offer: None,
            wizard_pane: WizardPaneState::new(),
            filter_input: TextInputState::new(),
            filter_active: false,
            deploy_source_dirs,
            primary_zone_dirs,
            packing_file_paths: HashSet::new(),
            packing_dir_categories: HashMap::new(),
        }
    }

    /// Get the corpus directory path (absolute).
    pub fn corpus_dir(&self) -> &Path {
        &self.corpus_dir
    }

    /// Get the corpus directory relative path.
    pub fn corpus_dir_rel(&self) -> &str {
        &self.corpus_dir_rel
    }

    /// Set focus to config panel.
    pub fn set_focus_config_panel(&mut self) {
        self.focus = CorpusBrowserFocus::ConfigPanel;
    }

    /// Set focus to tree browser.
    pub fn set_focus_tree(&mut self) {
        self.focus = CorpusBrowserFocus::TreeBrowser;
    }

    /// Set the pending edit paths (relative paths of dirs with staged config edits).
    pub fn set_pending_edit_paths(&mut self, paths: HashSet<PathBuf>) {
        self.pending_edit_paths = paths;
    }

    /// Get pending edit paths for render.
    pub fn pending_edit_paths(&self) -> &HashSet<PathBuf> {
        &self.pending_edit_paths
    }

    /// Whether there are any pending dir config edits.
    pub fn has_pending_edits(&self) -> bool {
        !self.pending_edit_paths.is_empty()
    }

    // -- Marker lookup data (String relative paths for DirectoryBrowser) --

    /// Update packing marker data from a fresh `GetPackingDirs` response.
    pub fn set_packing_markers(
        &mut self,
        file_paths: HashSet<String>,
        dir_categories: HashMap<String, PackingCategory>,
    ) {
        self.packing_file_paths = file_paths;
        self.packing_dir_categories = dir_categories;
    }

    /// Compute the deploy marker for a relative path.
    pub fn deploy_marker_for(&self, path: &str) -> DeployMarker {
        for src in &self.deploy_source_dirs {
            if path == src {
                return DeployMarker::SourceRoot;
            }
        }
        for src in &self.deploy_source_dirs {
            if path.starts_with(src.as_str()) {
                return DeployMarker::Inherited;
            }
        }
        DeployMarker::None
    }

    /// Compute the packing marker for a relative path (file or directory).
    pub fn packing_marker_for(&self, path: &str, is_dir: bool) -> PackingMarker {
        if is_dir {
            self.packing_dir_categories
                .get(path)
                .map(|&cat| PackingMarker::Directory(cat))
                .unwrap_or(PackingMarker::None)
        } else if self.packing_file_paths.contains(path) {
            PackingMarker::Matched
        } else {
            PackingMarker::None
        }
    }

    /// Whether a relative path should be visually dimmed (not under any primary zone).
    pub fn is_dimmed(&self, path: &str) -> bool {
        !self.primary_zone_dirs.iter().any(|z| path.starts_with(z.as_str()))
    }

    /// Get the packing file paths set (for render helpers).
    pub fn packing_file_paths(&self) -> &HashSet<String> {
        &self.packing_file_paths
    }

    /// Get the packing dir categories map (for render helpers).
    pub fn packing_dir_categories(&self) -> &HashMap<String, PackingCategory> {
        &self.packing_dir_categories
    }

    /// Whether the inline filter bar is actively accepting input.
    pub fn filter_active(&self) -> bool {
        self.filter_active
    }

    /// Get a reference to the filter input state (for rendering).
    pub fn filter_input(&self) -> &TextInputState {
        &self.filter_input
    }

    /// Activate the inline filter bar.
    fn activate_filter(&mut self) {
        self.filter_active = true;
        self.filter_input.focused = true;
        self.focus = CorpusBrowserFocus::TreeBrowser;
    }

    /// Apply the current filter text to the browser.
    ///
    /// If non-empty, stores a `BrowserAction::RequestSearch` as pending for
    /// the app layer to dispatch as a server-side search query.
    fn apply_filter(&mut self, browser: &mut DirectoryBrowser) -> Option<BrowserAction> {
        let query = self.filter_input.value().trim().to_string();
        self.filter_active = false;
        self.filter_input.focused = false;
        if query.is_empty() {
            browser.clear_path_filter();
            None
        } else {
            Some(BrowserAction::RequestSearch(query))
        }
    }

    /// Deactivate the filter bar and clear any active filter.
    fn deactivate_filter(&mut self, browser: &mut DirectoryBrowser) {
        self.filter_active = false;
        self.filter_input.focused = false;
        self.filter_input.clear();
        browser.clear_path_filter();
    }

    /// Set search results from a server-side query response.
    ///
    /// Called by the app layer after dispatching `SearchCorpusFiles`.
    pub fn set_search_results(&mut self, matches: Vec<String>, browser: &DirectoryBrowser) {
        self.search.first_match_idx = matches
            .first()
            .and_then(|path| browser.entries.iter().position(|e| e.path == *path));
        self.search.matches = matches;
    }

    /// Set filter results from a server-side search query response.
    ///
    /// Called by the app layer after dispatching the filter's `RequestSearch`.
    pub fn set_filter_results(&mut self, matching_paths: HashSet<String>, browser: &mut DirectoryBrowser) {
        browser.set_path_filter(matching_paths);
    }

    /// Called when cursor moves — dismiss any active wizard.
    pub fn on_cursor_move(&mut self, _browser: &DirectoryBrowser) {
        self.wizard_state.dismiss();
        self.wizard_offer = None;
        self.wizard_pane.reset();
    }

    /// Handle Escape - dismiss wizard, clear filter/search, return focus to tree.
    pub fn handle_escape(&mut self, browser: &mut DirectoryBrowser) -> bool {
        // Wizard dismiss takes priority
        if !self.wizard_state.is_idle() {
            self.wizard_state.dismiss();
            self.wizard_offer = None;
            self.wizard_pane.reset();
            return true;
        }

        // Config panel escape
        if self.focus == CorpusBrowserFocus::ConfigPanel {
            if let Some(ref mut panel) = self.config_panel {
                // If in text input, cancel input
                if panel.text_input.is_some() {
                    panel.text_input = None;
                    return true;
                }
                // If focus is on buttons, go back to fields
                if panel.focus == PanelFocus::Buttons {
                    panel.focus = PanelFocus::Fields;
                    return true;
                }
            }
            // Close panel
            self.config_panel = None;
            self.focus = CorpusBrowserFocus::TreeBrowser;
            return true;
        }

        // Inline filter bar: Esc clears filter and deactivates
        if self.filter_active || browser.has_path_filter() {
            self.deactivate_filter(browser);
            return true;
        }

        if self.match_selection_mode {
            self.reset_match_selection();
            true
        } else if self.focus == CorpusBrowserFocus::SearchBar || !self.search_input.is_empty() {
            // Clear search and return to tree
            self.search_input.clear();
            self.search.clear();
            self.focus = CorpusBrowserFocus::TreeBrowser;
            true
        } else {
            false
        }
    }

    /// Check if variant wants to capture navigation keys.
    pub fn wants_navigation_keys(&self) -> bool {
        self.filter_active
            || self.focus == CorpusBrowserFocus::ConfigPanel
            || self.match_selection_mode
            || self.focus == CorpusBrowserFocus::SearchBar
            || !self.search.matches.is_empty()
            || self.wizard_state.is_showing_pane()
    }

    /// Handle variant-specific input actions.
    pub fn handle_input(
        &mut self,
        action: &InputAction,
        browser: &mut DirectoryBrowser,
    ) -> VariantInputResult {
        let none = VariantInputResult { tree_action: None, browser_action: None };

        // Config panel has priority when focused
        if self.focus == CorpusBrowserFocus::ConfigPanel {
            return VariantInputResult {
                tree_action: self.handle_config_panel_input(action),
                browser_action: None,
            };
        }

        // Inline filter bar captures all input when active
        if self.filter_active {
            match action {
                InputAction::Confirm => {
                    let browser_action = self.apply_filter(browser);
                    return VariantInputResult { tree_action: None, browser_action };
                }
                InputAction::Cancel => {
                    return none;
                }
                other => {
                    self.filter_input.handle_input(other);
                    return none;
                }
            }
        }

        // Wizard pane captures nav keys for scrolling
        if self.wizard_state.is_showing_pane()
            && self.wizard_pane.handle_input(action)
        {
            return none;
        }

        // Match selection mode has priority (modal overlay)
        if self.match_selection_mode {
            return VariantInputResult {
                tree_action: self.handle_match_selection_input(action, browser),
                browser_action: None,
            };
        }

        // Route based on focus
        let tree_action = match self.focus {
            CorpusBrowserFocus::SearchBar => self.handle_search_bar_input(action, browser),
            CorpusBrowserFocus::TreeBrowser => self.handle_tree_browser_input(action, browser),
            CorpusBrowserFocus::ConfigPanel => unreachable!(),
        };
        VariantInputResult { tree_action, browser_action: None }
    }

    /// Handle input when tree browser is focused.
    fn handle_tree_browser_input(
        &mut self,
        action: &InputAction,
        browser: &mut DirectoryBrowser,
    ) -> Option<TreeBrowserAction> {
        // If we have search results visible, capture navigation keys
        if !self.search.matches.is_empty() {
            match action {
                InputAction::NavUp => {
                    self.enter_match_and_up();
                    return None;
                }
                InputAction::NavDown => {
                    self.enter_match_and_down();
                    return None;
                }
                // Consume Left/Right to prevent tree navigation when search results visible
                InputAction::NavLeft | InputAction::NavRight => {
                    return None;
                }
                _ => {}
            }
        }

        match action {
            InputAction::Confirm => {
                if let Some(entry) = browser.current_entry() {
                    if entry.is_dir {
                        Some(TreeBrowserAction::EditDirectory(entry.path.clone()))
                    } else {
                        Some(TreeBrowserAction::EditFile(entry.path.clone()))
                    }
                } else {
                    None
                }
            }
            // C opens dir config panel on any corpus directory
            InputAction::Char('C') => {
                if let Some(entry) = browser.current_entry() {
                    if entry.is_dir && entry.path.starts_with(&self.corpus_dir_rel) {
                        return Some(TreeBrowserAction::OpenDirConfig(entry.path.clone()));
                    }
                }
                None
            }
            // R opens transaction review when pending dir config edits exist
            InputAction::Char('R') if self.has_pending_edits() => {
                Some(TreeBrowserAction::ReviewTransaction)
            }
            // Z opens/advances wizard popup/pane on entries with packing markers
            InputAction::Char('Z') => {
                if let Some(entry) = browser.current_entry() {
                    let marker = self.packing_marker_for(&entry.path, entry.is_dir);
                    if marker != PackingMarker::None {
                        let offer = self
                            .wizard_offer
                            .get_or_insert_with(|| Self::build_wizard_offer(&marker, &entry.name));
                        self.wizard_state.advance(offer);
                        if self.wizard_state.is_idle() {
                            self.wizard_offer = None;
                            self.wizard_pane.reset();
                        }
                    }
                }
                None
            }
            // Tab cycles through [MB] directories
            InputAction::CycleNext => {
                if self.cycle_packing_dirs(browser, true) {
                    None
                } else {
                    Some(TreeBrowserAction::CycleNext)
                }
            }
            InputAction::CyclePrev => {
                if self.cycle_packing_dirs(browser, false) {
                    None
                } else {
                    Some(TreeBrowserAction::CyclePrev)
                }
            }
            // Ctrl+/ activates inline filter
            InputAction::OpenFilter => {
                self.activate_filter();
                None
            }
            _ => None,
        }
    }

    /// Handle input when config panel is focused.
    fn handle_config_panel_input(&mut self, action: &InputAction) -> Option<TreeBrowserAction> {
        let panel = match self.config_panel {
            Some(ref mut p) => p,
            None => return None,
        };

        // Text input mode intercepts all keys
        if let Some(ref mut input) = panel.text_input {
            match action {
                InputAction::Confirm => {
                    let value = input.value().to_string();
                    if panel.field_cursor == 3 {
                        panel.path_schema = if value.is_empty() { None } else { Some(value) };
                    } else if panel.field_cursor == 5 {
                        panel.pinned_release = if value.is_empty() { None } else { Some(value) };
                    } else if !value.is_empty() {
                        if let Some(cursor) = panel.lib_cursor {
                            if cursor < panel.libraries.len() {
                                panel.libraries[cursor] = value;
                            }
                        } else {
                            panel.libraries.push(value);
                            panel.lib_cursor = Some(panel.libraries.len() - 1);
                        }
                    }
                    panel.text_input = None;
                    return None;
                }
                InputAction::Cancel => {
                    panel.text_input = None;
                    return None;
                }
                _ => {
                    input.handle_input(action);
                    return None;
                }
            }
        }

        // Button focus
        if panel.focus == PanelFocus::Buttons {
            match action {
                InputAction::NavLeft => {
                    if panel.button_cursor > 0 {
                        panel.button_cursor -= 1;
                    }
                    return None;
                }
                InputAction::NavRight => {
                    if panel.button_cursor < 1 {
                        panel.button_cursor += 1;
                    }
                    return None;
                }
                InputAction::NavUp => {
                    panel.focus = PanelFocus::Fields;
                    return None;
                }
                InputAction::Confirm => {
                    if panel.button_cursor == 0 {
                        return Some(TreeBrowserAction::SaveDirConfig);
                    } else {
                        return Some(TreeBrowserAction::CloseDirConfig);
                    }
                }
                InputAction::Cancel => {
                    panel.focus = PanelFocus::Fields;
                    return None;
                }
                _ => return None,
            }
        }

        // Field focus
        match action {
            InputAction::NavUp => {
                panel.wizard_state.dismiss();
                if panel.field_cursor == 0 {
                    if let Some(ref mut cursor) = panel.lib_cursor {
                        if *cursor > 0 {
                            *cursor -= 1;
                        } else {
                            panel.lib_cursor = None;
                        }
                    }
                } else {
                    panel.field_cursor -= 1;
                    panel.lib_cursor = None;
                }
                None
            }
            InputAction::NavDown => {
                panel.wizard_state.dismiss();
                if panel.field_cursor == 0 {
                    if !panel.libraries.is_empty() {
                        if let Some(ref mut cursor) = panel.lib_cursor {
                            if *cursor + 1 < panel.libraries.len() {
                                *cursor += 1;
                            } else {
                                panel.field_cursor = 1;
                                panel.lib_cursor = None;
                            }
                        } else {
                            panel.lib_cursor = Some(0);
                        }
                    } else {
                        panel.field_cursor = 1;
                    }
                } else if panel.field_cursor < 5 {
                    panel.field_cursor += 1;
                }
                None
            }
            InputAction::CycleNext => {
                panel.focus = PanelFocus::Buttons;
                panel.button_cursor = 0;
                None
            }
            InputAction::Confirm | InputAction::Toggle => {
                match panel.field_cursor {
                    0 => {
                        if let Some(cursor) = panel.lib_cursor {
                            if cursor < panel.libraries.len() {
                                let mut input = TextInputState::new();
                                input.set_value(panel.libraries[cursor].clone());
                                panel.text_input = Some(input);
                            }
                        }
                    }
                    1 => {
                        panel.can_stash_dupes = cycle_opt_bool(panel.can_stash_dupes);
                    }
                    2 => {
                        panel.interior_dupes = cycle_opt_bool(panel.interior_dupes);
                    }
                    3 => {
                        let mut input = TextInputState::new();
                        if let Some(ref schema) = panel.path_schema {
                            input.set_value(schema.clone());
                        }
                        panel.text_input = Some(input);
                    }
                    4 => {
                        panel.enable_acoustid = cycle_opt_bool(panel.enable_acoustid);
                    }
                    5 => {
                        let mut input = TextInputState::new();
                        if let Some(ref release) = panel.pinned_release {
                            input.set_value(release.clone());
                        }
                        panel.text_input = Some(input);
                    }
                    _ => {}
                }
                None
            }
            InputAction::Char('n') => {
                if panel.field_cursor == 0 {
                    panel.lib_cursor = None;
                    panel.text_input = Some(TextInputState::new());
                }
                None
            }
            InputAction::Char('x') => {
                if panel.field_cursor == 0 {
                    if let Some(cursor) = panel.lib_cursor {
                        if cursor < panel.libraries.len() {
                            panel.libraries.remove(cursor);
                            if panel.libraries.is_empty() {
                                panel.lib_cursor = None;
                            } else if cursor >= panel.libraries.len() {
                                panel.lib_cursor = Some(panel.libraries.len() - 1);
                            }
                        }
                    }
                }
                None
            }
            InputAction::Char('z') | InputAction::Char('Z') => {
                let help = DIR_FIELD_HELP[panel.field_cursor];
                if !help.is_empty() {
                    let lines: Vec<Line<'static>> =
                        help.iter().map(|s| Line::raw(s.to_string())).collect();
                    let offer = WizardOffer::Popup(lines);
                    panel.wizard_state.advance(&offer);
                }
                None
            }
            InputAction::Cancel => Some(TreeBrowserAction::CloseDirConfig),
            _ => None,
        }
    }

    /// Handle input when search bar is focused.
    fn handle_search_bar_input(
        &mut self,
        action: &InputAction,
        browser: &mut DirectoryBrowser,
    ) -> Option<TreeBrowserAction> {
        match action {
            InputAction::Confirm => {
                self.handle_search_enter(browser);
                None
            }
            InputAction::Cancel => {
                self.search_input.clear();
                self.search.clear();
                self.focus = CorpusBrowserFocus::TreeBrowser;
                self.search_input.focused = false;
                None
            }
            InputAction::CycleNext => {
                if let Some(suggestion) = self.get_suggestion() {
                    self.search_input.set_value(suggestion);
                }
                None
            }
            InputAction::NavUp => {
                if !self.search.matches.is_empty() {
                    self.enter_match_and_up();
                }
                None
            }
            InputAction::NavDown => {
                if !self.search.matches.is_empty() {
                    self.enter_match_and_down();
                }
                None
            }
            InputAction::NavLeft | InputAction::NavRight | InputAction::Home | InputAction::End => {
                self.search_input.handle_input(action);
                None
            }
            InputAction::Char(c) => {
                self.search_input.insert_char(*c);
                self.reset_match_selection();
                None
            }
            InputAction::Backspace => {
                self.search_input.backspace();
                if self.search_input.is_empty() {
                    self.search.clear();
                }
                self.reset_match_selection();
                None
            }
            InputAction::Delete => {
                self.search_input.delete();
                if self.search_input.is_empty() {
                    self.search.clear();
                }
                self.reset_match_selection();
                None
            }
            InputAction::KillToStart => {
                self.search_input.clear();
                self.search.clear();
                self.reset_match_selection();
                None
            }
            _ => None,
        }
    }

    /// Handle Enter in search bar
    fn handle_search_enter(&mut self, browser: &mut DirectoryBrowser) {
        let match_count = self.search.matches.len();

        if match_count == 0 {
            return;
        }

        if match_count == 1 {
            let target = self.search.matches[0].clone();
            browser.navigate_to_path(&target);
            self.search_input.clear();
            self.search.clear();
            self.focus = CorpusBrowserFocus::TreeBrowser;
            self.search_input.focused = false;
        } else {
            self.match_selection_mode = true;
            self.match_selection_idx = 0;
        }
    }

    /// Handle input during match selection mode.
    fn handle_match_selection_input(
        &mut self,
        action: &InputAction,
        browser: &mut DirectoryBrowser,
    ) -> Option<TreeBrowserAction> {
        match action {
            InputAction::NavUp => {
                self.enter_match_and_up();
                None
            }
            InputAction::NavDown => {
                self.enter_match_and_down();
                None
            }
            InputAction::Confirm => {
                self.confirm_match_selection(browser);
                None
            }
            InputAction::Cancel => {
                self.reset_match_selection();
                None
            }
            _ => None,
        }
    }

    // =========================================================================
    // Wizard (Packing Info)
    // =========================================================================

    /// Build a wizard offer from a packing marker.
    fn build_wizard_offer(marker: &PackingMarker, name: &str) -> WizardOffer {
        match marker {
            PackingMarker::None => unreachable!("called with None marker"),
            PackingMarker::Matched => {
                WizardOffer::Popup(vec![
                    Line::styled(
                        "MusicBrainz Match",
                        ratatui::style::Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Line::raw(""),
                    Line::styled(
                        format!("♪ {}", name),
                        ratatui::style::Style::default().fg(Color::White),
                    ),
                    Line::styled(
                        "Assigned to a release via packing",
                        ratatui::style::Style::default().fg(Color::DarkGray),
                    ),
                ])
            }
            PackingMarker::Directory(cat) => {
                let symbol = cat.marker_symbol();
                let label = cat.label();
                let color = cat.color();

                let popup = vec![
                    Line::styled(
                        format!("{} {}", symbol, label),
                        ratatui::style::Style::default()
                            .fg(color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Line::raw(""),
                    Line::styled(
                        format!(" {}", name),
                        ratatui::style::Style::default().fg(Color::Blue),
                    ),
                    Line::raw(""),
                    Line::styled(
                        "Z again for details",
                        ratatui::style::Style::default().fg(Color::DarkGray),
                    ),
                ];

                use crate::widgets::rich_text::{RichBlock, RichSpan};

                let pane_content = vec![
                    RichBlock::Heading(format!("{} {}", symbol, label)),
                    RichBlock::Blank,
                    RichBlock::Paragraph(vec![
                        RichSpan::new(
                            format!("Directory: {}", name),
                            ratatui::style::Style::default().fg(Color::White),
                        ),
                    ]),
                    RichBlock::Blank,
                    RichBlock::Paragraph(vec![
                        RichSpan::new(
                            "Category indicates the best release match",
                            ratatui::style::Style::default().fg(Color::DarkGray),
                        ),
                    ]),
                    RichBlock::Paragraph(vec![
                        RichSpan::new(
                            "quality found in this directory's files.",
                            ratatui::style::Style::default().fg(Color::DarkGray),
                        ),
                    ]),
                    RichBlock::Blank,
                    RichBlock::Paragraph(vec![
                        RichSpan::new(
                            "Open the packing browser for full details.",
                            ratatui::style::Style::default().fg(Color::DarkGray),
                        ),
                    ]),
                ];

                WizardOffer::Both {
                    popup,
                    pane_title: format!("[MB{}] {}", symbol, name),
                    pane_content,
                }
            }
        }
    }

    /// Cycle to the next/previous [MB] directory. Returns true if cycled.
    fn cycle_packing_dirs(&mut self, browser: &mut DirectoryBrowser, forward: bool) -> bool {
        let packing_cats = &self.packing_dir_categories;
        let predicate = |entry: &mm_ui::directory_browser::BrowserEntry| {
            entry.is_dir && packing_cats.contains_key(&entry.path)
        };

        let target = if forward {
            browser.find_next_matching(browser.cursor, predicate)
        } else {
            browser.find_prev_matching(browser.cursor, predicate)
        };

        if let Some(target_idx) = target {
            browser.set_cursor(target_idx);
            self.on_cursor_move(browser);
            true
        } else {
            false
        }
    }

    /// Get wizard state for render access.
    pub fn wizard_state(&self) -> WizardState {
        self.wizard_state
    }

    /// Get wizard offer for render access.
    pub fn wizard_offer(&self) -> Option<&WizardOffer> {
        self.wizard_offer.as_ref()
    }

    /// Get wizard pane state for render access.
    pub fn wizard_pane_mut(&mut self) -> &mut WizardPaneState {
        &mut self.wizard_pane
    }

    // =========================================================================
    // Search helpers
    // =========================================================================

    /// Get suggestion for tab completion (first match filename).
    fn get_suggestion(&self) -> Option<String> {
        self.search
            .matches
            .first()
            .and_then(|p| p.rsplit('/').next().map(|s| s.to_string()))
    }

    fn reset_match_selection(&mut self) {
        self.match_selection_mode = false;
        self.match_selection_idx = 0;
    }

    fn enter_match_and_up(&mut self) {
        if !self.match_selection_mode {
            self.match_selection_mode = true;
            self.match_selection_idx = 0;
        }
        if self.match_selection_idx > 0 {
            self.match_selection_idx -= 1;
        }
    }

    fn enter_match_and_down(&mut self) {
        if !self.match_selection_mode {
            self.match_selection_mode = true;
            self.match_selection_idx = 0;
        }
        if self.match_selection_idx + 1 < self.search.matches.len() {
            self.match_selection_idx += 1;
        }
    }

    fn confirm_match_selection(&mut self, browser: &mut DirectoryBrowser) {
        if let Some(path) = self.search.matches.get(self.match_selection_idx).cloned() {
            browser.navigate_to_path(&path);
        }
        self.reset_match_selection();
        self.search_input.clear();
        self.search.clear();
        self.focus = CorpusBrowserFocus::TreeBrowser;
        self.search_input.focused = false;
    }

    // =========================================================================
    /// Render variant-specific overlays.
    pub fn render_overlays(&self, f: &mut Frame, area: Rect) {
        if self.match_selection_mode {
            self.render_match_modal(f, area);
        }
    }

    /// Render match selection modal.
    fn render_match_modal(&self, f: &mut Frame, area: Rect) {
        let matches = &self.search.matches;
        if matches.is_empty() {
            return;
        }

        let max_width = matches
            .iter()
            .map(|p| p.len())
            .max()
            .unwrap_or(30)
            .min(60) as u16
            + 6;
        let height = (matches.len() as u16 + 4).min(area.height - 4);

        let x = area.x + (area.width.saturating_sub(max_width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let modal_area = Rect::new(x, y, max_width, height);

        let visible_height = (height - 4) as usize;
        let scroll_offset = self.match_selection_idx.saturating_sub(visible_height / 2);

        let lines: Vec<Line> = matches
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(visible_height)
            .map(|(idx, path)| {
                let path_str = truncate_left(path, (max_width - 6) as usize);
                let indicator = if idx == self.match_selection_idx {
                    "▷ "
                } else {
                    "  "
                };
                let style = if idx == self.match_selection_idx {
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };
                Line::styled(format!("{}{}", indicator, path_str), style)
            })
            .collect();

        let help_line = Line::styled(
            "↑↓ navigate  Enter select  Esc cancel",
            Style::default().fg(Color::DarkGray),
        );

        let mut all_lines = lines;
        all_lines.push(Line::raw(""));
        all_lines.push(help_line);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title(format!(
                "Select Match ({}/{})",
                self.match_selection_idx + 1,
                matches.len()
            ));

        f.render_widget(Clear, modal_area);
        f.render_widget(Paragraph::new(all_lines).block(block), modal_area);
    }
}

/// Cycle an Option<bool> through None → Some(true) → Some(false) → None.
fn cycle_opt_bool(v: Option<bool>) -> Option<bool> {
    match v {
        None => Some(true),
        Some(true) => Some(false),
        Some(false) => None,
    }
}
