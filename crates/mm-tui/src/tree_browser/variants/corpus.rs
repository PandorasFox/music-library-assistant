//! Corpus Browser Variant
//!
//! Browse files and directories with persistent search bar and tag editor launch.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::packing_colors::PackingCategoryColor;
use crate::helpers::truncate_left;
use crate::input::InputAction;
use crate::widgets::detail_panel::{
    render_detail_panel, DetailField, DetailPanelParams, DetailWidget, PanelButton,
};
use crate::widgets::TextInputState;

use crate::tree_browser::actions::TreeBrowserAction;
use crate::tree_browser::config::CorpusBrowserConfig;
use crate::tree_browser::entry::PackingMarker;
use crate::tree_browser::navigator::TreeNavigator;
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
    /// Directory being searched (root of search)
    pub search_root: Option<PathBuf>,
    /// Matching paths found (files and directories)
    pub matches: Vec<PathBuf>,
    /// Index in entries list for first match (for auto-scroll)
    pub first_match_idx: Option<usize>,
}

impl SearchState {
    /// Clear search results
    pub fn clear(&mut self) {
        self.search_root = None;
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
    /// 0=Save, 1=Discard
    pub button_cursor: usize,
    /// Selected library item within the libraries list (None = field label focused).
    pub lib_cursor: Option<usize>,
    /// Active text input for library editing/adding.
    pub text_input: Option<TextInputState>,
    /// Wizard popup state for Z-key help text.
    pub wizard_state: WizardState,
}

impl DirConfigPanelState {
    /// Whether any edits have been made.
    pub fn has_edits(&self) -> bool {
        self.libraries != self.orig_libraries
            || self.can_stash_dupes != self.orig_can_stash_dupes
            || self.interior_dupes != self.orig_interior_dupes
            || self.path_schema != self.orig_path_schema
            || self.enable_acoustid != self.orig_enable_acoustid
            || self.pinned_release != self.orig_pinned_release
    }

    /// Render the config panel using the detail_panel widget.
    pub fn render_config_panel(&self, f: &mut Frame, area: Rect) {
        let path_str = self.source_path.display().to_string();
        let title = format!(" Dir: {} ", path_str);
        let border_color = if self.has_edits() {
            Color::Yellow
        } else {
            Color::Cyan
        };

        let libs_edited = self.libraries != self.orig_libraries;
        let stash_edited = self.can_stash_dupes != self.orig_can_stash_dupes;
        let interior_edited = self.interior_dupes != self.orig_interior_dupes;
        let schema_edited = self.path_schema != self.orig_path_schema;
        let acoustid_edited = self.enable_acoustid != self.orig_enable_acoustid;
        let pinned_edited = self.pinned_release != self.orig_pinned_release;

        // If we're in text input mode, show the input line instead of the list
        let libs_for_display: Vec<String> = if let Some(ref input) = self.text_input {
            // Show current items + the input line
            let mut items = self.libraries.clone();
            if let Some(cursor) = self.lib_cursor {
                if cursor < items.len() {
                    items[cursor] = format!("{}|", input.value());
                }
            } else {
                items.push(format!("{}|", input.value()));
            }
            items
        } else {
            self.libraries.clone()
        };

        let schema_display = self.path_schema.as_deref().unwrap_or("(none)").to_string();
        let schema_display_with_cursor = if self.field_cursor == 3 && self.text_input.is_some() {
            if let Some(ref input) = self.text_input {
                format!("{}|", input.value())
            } else {
                schema_display.clone()
            }
        } else {
            schema_display.clone()
        };

        let pinned_display = self.pinned_release.as_deref().unwrap_or("(none)").to_string();
        let pinned_display_with_cursor = if self.field_cursor == 5 && self.text_input.is_some() {
            if let Some(ref input) = self.text_input {
                format!("{}|", input.value())
            } else {
                pinned_display.clone()
            }
        } else {
            pinned_display.clone()
        };

        let fields = [
            DetailField {
                label: "Libraries",
                widget: DetailWidget::StringItems {
                    items: &libs_for_display,
                    cursor: if self.field_cursor == 0 {
                        self.lib_cursor.or(if self.text_input.is_some() {
                            Some(libs_for_display.len().saturating_sub(1))
                        } else {
                            None
                        })
                    } else {
                        None
                    },
                    edited: libs_edited,
                },
            },
            DetailField {
                label: "Can stash dupes",
                widget: DetailWidget::OptBool {
                    value: self.can_stash_dupes,
                    edited: stash_edited,
                },
            },
            DetailField {
                label: "Interior dupes",
                widget: DetailWidget::OptBool {
                    value: self.interior_dupes,
                    edited: interior_edited,
                },
            },
            DetailField {
                label: "Path schema",
                widget: DetailWidget::Text {
                    value: &schema_display_with_cursor,
                    edited: schema_edited,
                },
            },
            DetailField {
                label: "AcoustID lookup",
                widget: DetailWidget::OptBool {
                    value: self.enable_acoustid,
                    edited: acoustid_edited,
                },
            },
            DetailField {
                label: "Pinned release",
                widget: DetailWidget::Text {
                    value: &pinned_display_with_cursor,
                    edited: pinned_edited,
                },
            },
        ];

        let buttons = [
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

        let has_help = !DIR_FIELD_HELP[self.field_cursor].is_empty();
        let z_suffix = if has_help { "  Z info" } else { "" };

        let hint: Option<String> = if self.text_input.is_some() {
            Some("Enter confirm  Esc cancel".to_string())
        } else if self.focus == PanelFocus::Buttons {
            Some("Enter select  Up fields".to_string())
        } else if self.field_cursor == 0 {
            Some(format!("n add  x del  Enter edit  Tab buttons{z_suffix}"))
        } else if self.field_cursor == 3 || self.field_cursor == 5 {
            Some(format!("Enter edit  Tab buttons{z_suffix}"))
        } else {
            // Bool fields: can_stash_dupes, interior_dupes, enable_acoustid
            Some(format!("Enter/Space toggle  Tab buttons{z_suffix}"))
        };

        render_detail_panel(
            f,
            area,
            &DetailPanelParams {
                title: &title,
                border_color,
                fields: &fields,
                field_cursor: self.field_cursor,
                buttons: &buttons,
                focus_on_buttons: self.focus == PanelFocus::Buttons,
                hint: hint.as_deref(),
            },
        );

        // Render wizard popup if showing
        if self.wizard_state.is_showing_popup() {
            let help = DIR_FIELD_HELP[self.field_cursor];
            if !help.is_empty() {
                use crate::widgets::wizard_popup::WizardPopup;
                let popup_lines: Vec<Line<'_>> =
                    help.iter().map(|s| Line::raw(s.to_string())).collect();
                // Anchor to field cursor row within the panel (border=1, each field ~1 row)
                let anchor_y = area.y + 1 + self.field_cursor as u16;
                let anchor_x = area.x + area.width / 2;
                WizardPopup::render(f, &popup_lines, anchor_x, anchor_y, area);
            }
        }
    }
}

/// Corpus browser variant state.
#[derive(Debug)]
pub struct CorpusBrowserVariant {
    /// Variant-specific configuration
    config: CorpusBrowserConfig,
    /// Absolute path to corpus directory (for C key guard)
    corpus_dir: PathBuf,
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
}

impl CorpusBrowserVariant {
    /// Create a new corpus browser variant.
    pub fn new(config: CorpusBrowserConfig, corpus_dir: PathBuf) -> Self {
        Self {
            config,
            corpus_dir,
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
        }
    }

    /// Get the corpus directory path.
    pub fn corpus_dir(&self) -> &Path {
        &self.corpus_dir
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

    /// Apply the current filter text to the tree navigator.
    fn apply_filter(&mut self, nav: &mut TreeNavigator) {
        let query = self.filter_input.value().trim().to_lowercase();
        if query.is_empty() {
            nav.clear_path_filter();
        } else {
            nav.apply_text_filter(&query);
        }
        self.filter_active = false;
        self.filter_input.focused = false;
    }

    /// Deactivate the filter bar and clear any active filter.
    fn deactivate_filter(&mut self, nav: &mut TreeNavigator) {
        self.filter_active = false;
        self.filter_input.focused = false;
        self.filter_input.clear();
        nav.clear_path_filter();
    }

    /// Called when cursor moves — dismiss any active wizard.
    pub fn on_cursor_move(&mut self, _nav: &TreeNavigator) {
        self.wizard_state.dismiss();
        self.wizard_offer = None;
        self.wizard_pane.reset();
    }

    /// Handle Escape - dismiss wizard, clear filter/search, return focus to tree.
    pub fn handle_escape(&mut self, nav: &mut TreeNavigator) -> bool {
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
        if self.filter_active || nav.has_path_filter() {
            self.deactivate_filter(nav);
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
    /// Returns true when search is active (search bar focused or has results),
    /// config panel is focused, filter is active, or wizard pane is showing.
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
        nav: &mut TreeNavigator,
    ) -> TreeBrowserAction {
        // Config panel has priority when focused
        if self.focus == CorpusBrowserFocus::ConfigPanel {
            return self.handle_config_panel_input(action);
        }

        // Inline filter bar captures all input when active
        if self.filter_active {
            match action {
                InputAction::Confirm => {
                    self.apply_filter(nav);
                    return TreeBrowserAction::None;
                }
                InputAction::Cancel => {
                    // Esc handled by handle_escape
                    return TreeBrowserAction::None;
                }
                other => {
                    self.filter_input.handle_input(other);
                    return TreeBrowserAction::None;
                }
            }
        }

        // Wizard pane captures nav keys for scrolling
        if self.wizard_state.is_showing_pane() {
            if self.wizard_pane.handle_input(action) {
                return TreeBrowserAction::None;
            }
            // Esc handled by handle_escape; other keys fall through to tree
        }

        // Match selection mode has priority (modal overlay)
        if self.match_selection_mode {
            return self.handle_match_selection_input(action, nav);
        }

        // Route based on focus
        match self.focus {
            CorpusBrowserFocus::SearchBar => self.handle_search_bar_input(action, nav),
            CorpusBrowserFocus::TreeBrowser => self.handle_tree_browser_input(action, nav),
            CorpusBrowserFocus::ConfigPanel => unreachable!(),
        }
    }

    /// Handle input when tree browser is focused.
    fn handle_tree_browser_input(
        &mut self,
        action: &InputAction,
        nav: &mut TreeNavigator,
    ) -> TreeBrowserAction {
        // If we have search results visible, capture navigation keys
        if !self.search.matches.is_empty() {
            match action {
                InputAction::NavUp => {
                    self.enter_match_and_up();
                    return TreeBrowserAction::None;
                }
                InputAction::NavDown => {
                    self.enter_match_and_down();
                    return TreeBrowserAction::None;
                }
                // Consume Left/Right to prevent tree navigation when search results visible
                InputAction::NavLeft | InputAction::NavRight => {
                    return TreeBrowserAction::None;
                }
                _ => {}
            }
        }

        match action {
            InputAction::Confirm => {
                if let Some(entry) = nav.current_entry() {
                    if entry.is_directory() {
                        TreeBrowserAction::EditDirectory(entry.path.clone())
                    } else {
                        TreeBrowserAction::EditFile(entry.path.clone())
                    }
                } else {
                    TreeBrowserAction::None
                }
            }
            // C opens dir config panel on any corpus directory
            InputAction::Char('C') => {
                if let Some(entry) = nav.current_entry() {
                    if entry.is_directory() && entry.path.starts_with(&self.corpus_dir) {
                        return TreeBrowserAction::OpenDirConfig(entry.path.clone());
                    }
                }
                TreeBrowserAction::None
            }
            // R opens transaction review when pending dir config edits exist
            InputAction::Char('R') if self.has_pending_edits() => {
                TreeBrowserAction::ReviewTransaction
            }
            // Z opens/advances wizard popup/pane on entries with packing markers
            InputAction::Char('Z') => {
                if let Some(entry) = nav.current_entry() {
                    if entry.packing_marker != PackingMarker::None {
                        let offer = self
                            .wizard_offer
                            .get_or_insert_with(|| Self::build_wizard_offer(&entry.packing_marker, &entry.name));
                        self.wizard_state.advance(offer);
                        if self.wizard_state.is_idle() {
                            self.wizard_offer = None;
                            self.wizard_pane.reset();
                        }
                    }
                }
                TreeBrowserAction::None
            }
            // Tab cycles through [MB] directories
            InputAction::CycleNext => {
                if self.cycle_packing_dirs(nav, true) {
                    TreeBrowserAction::None
                } else {
                    TreeBrowserAction::CycleNext
                }
            }
            InputAction::CyclePrev => {
                if self.cycle_packing_dirs(nav, false) {
                    TreeBrowserAction::None
                } else {
                    TreeBrowserAction::CyclePrev
                }
            }
            // Ctrl+/ activates inline filter
            InputAction::OpenFilter => {
                self.activate_filter();
                TreeBrowserAction::None
            }
            _ => TreeBrowserAction::None,
        }
    }

    /// Handle input when config panel is focused.
    fn handle_config_panel_input(&mut self, action: &InputAction) -> TreeBrowserAction {
        let panel = match self.config_panel {
            Some(ref mut p) => p,
            None => return TreeBrowserAction::None,
        };

        // Text input mode intercepts all keys
        if let Some(ref mut input) = panel.text_input {
            match action {
                InputAction::Confirm => {
                    let value = input.value().to_string();
                    if panel.field_cursor == 3 {
                        // Path schema text input
                        panel.path_schema = if value.is_empty() { None } else { Some(value) };
                    } else if panel.field_cursor == 5 {
                        // Pinned release text input
                        panel.pinned_release = if value.is_empty() { None } else { Some(value) };
                    } else if !value.is_empty() {
                        if let Some(cursor) = panel.lib_cursor {
                            if cursor < panel.libraries.len() {
                                // Editing existing item
                                panel.libraries[cursor] = value;
                            }
                        } else {
                            // Adding new item
                            panel.libraries.push(value);
                            panel.lib_cursor = Some(panel.libraries.len() - 1);
                        }
                    }
                    panel.text_input = None;
                    return TreeBrowserAction::None;
                }
                InputAction::Cancel => {
                    panel.text_input = None;
                    return TreeBrowserAction::None;
                }
                _ => {
                    input.handle_input(action);
                    return TreeBrowserAction::None;
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
                    return TreeBrowserAction::None;
                }
                InputAction::NavRight => {
                    if panel.button_cursor < 1 {
                        panel.button_cursor += 1;
                    }
                    return TreeBrowserAction::None;
                }
                InputAction::NavUp => {
                    panel.focus = PanelFocus::Fields;
                    return TreeBrowserAction::None;
                }
                InputAction::Confirm => {
                    if panel.button_cursor == 0 {
                        // Save
                        return TreeBrowserAction::SaveDirConfig;
                    } else {
                        // Discard
                        return TreeBrowserAction::CloseDirConfig;
                    }
                }
                InputAction::Cancel => {
                    panel.focus = PanelFocus::Fields;
                    return TreeBrowserAction::None;
                }
                _ => return TreeBrowserAction::None,
            }
        }

        // Field focus
        match action {
            InputAction::NavUp => {
                panel.wizard_state.dismiss();
                if panel.field_cursor == 0 {
                    // Within libraries field, navigate items
                    if let Some(ref mut cursor) = panel.lib_cursor {
                        if *cursor > 0 {
                            *cursor -= 1;
                        } else {
                            // Deselect library items, stay on libraries field
                            panel.lib_cursor = None;
                        }
                    }
                } else {
                    panel.field_cursor -= 1;
                    panel.lib_cursor = None;
                }
                TreeBrowserAction::None
            }
            InputAction::NavDown => {
                panel.wizard_state.dismiss();
                if panel.field_cursor == 0 {
                    // Navigate into library items first
                    if !panel.libraries.is_empty() {
                        if let Some(ref mut cursor) = panel.lib_cursor {
                            if *cursor + 1 < panel.libraries.len() {
                                *cursor += 1;
                            } else {
                                // Move to next field
                                panel.field_cursor = 1;
                                panel.lib_cursor = None;
                            }
                        } else {
                            // Enter library items
                            panel.lib_cursor = Some(0);
                        }
                    } else {
                        panel.field_cursor = 1;
                    }
                } else if panel.field_cursor < 5 {
                    panel.field_cursor += 1;
                }
                TreeBrowserAction::None
            }
            InputAction::CycleNext => {
                panel.focus = PanelFocus::Buttons;
                panel.button_cursor = 0;
                TreeBrowserAction::None
            }
            InputAction::Confirm | InputAction::Toggle => {
                match panel.field_cursor {
                    0 => {
                        // Libraries: edit selected item
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
                        // Path schema: edit as text
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
                        // Pinned release: edit as text
                        let mut input = TextInputState::new();
                        if let Some(ref release) = panel.pinned_release {
                            input.set_value(release.clone());
                        }
                        panel.text_input = Some(input);
                    }
                    _ => {}
                }
                TreeBrowserAction::None
            }
            // n: add new library
            InputAction::Char('n') => {
                if panel.field_cursor == 0 {
                    panel.lib_cursor = None; // New item, no cursor position
                    panel.text_input = Some(TextInputState::new());
                }
                TreeBrowserAction::None
            }
            // x: delete selected library
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
                TreeBrowserAction::None
            }
            InputAction::Char('z') | InputAction::Char('Z') => {
                let help = DIR_FIELD_HELP[panel.field_cursor];
                if !help.is_empty() {
                    let lines: Vec<Line<'static>> =
                        help.iter().map(|s| Line::raw(s.to_string())).collect();
                    let offer = WizardOffer::Popup(lines);
                    panel.wizard_state.advance(&offer);
                }
                TreeBrowserAction::None
            }
            InputAction::Cancel => TreeBrowserAction::CloseDirConfig,
            _ => TreeBrowserAction::None,
        }
    }

    /// Handle input when search bar is focused.
    fn handle_search_bar_input(
        &mut self,
        action: &InputAction,
        nav: &mut TreeNavigator,
    ) -> TreeBrowserAction {
        match action {
            InputAction::Confirm => {
                self.handle_search_enter(nav);
                TreeBrowserAction::None
            }
            InputAction::Cancel => {
                // Clear search and return to tree
                self.search_input.clear();
                self.search.clear();
                self.focus = CorpusBrowserFocus::TreeBrowser;
                self.search_input.focused = false;
                TreeBrowserAction::None
            }
            InputAction::CycleNext => {
                // Apply suggestion if available
                if let Some(suggestion) = self.get_suggestion() {
                    self.search_input.set_value(suggestion);
                    self.update_search_matches(nav);
                }
                TreeBrowserAction::None
            }
            // Up/Down navigate search results (if any), or do nothing
            InputAction::NavUp => {
                if !self.search.matches.is_empty() {
                    self.enter_match_and_up();
                }
                TreeBrowserAction::None
            }
            InputAction::NavDown => {
                if !self.search.matches.is_empty() {
                    self.enter_match_and_down();
                }
                TreeBrowserAction::None
            }
            // Arrow keys and Home/End for cursor navigation in text input
            InputAction::NavLeft | InputAction::NavRight | InputAction::Home | InputAction::End => {
                self.search_input.handle_input(action);
                TreeBrowserAction::None
            }
            // Character input
            InputAction::Char(c) => {
                self.search_input.insert_char(*c);
                self.update_search_matches(nav);
                self.reset_match_selection();
                TreeBrowserAction::None
            }
            InputAction::Backspace => {
                self.search_input.backspace();
                if self.search_input.is_empty() {
                    self.search.clear();
                } else {
                    self.update_search_matches(nav);
                }
                self.reset_match_selection();
                TreeBrowserAction::None
            }
            InputAction::Delete => {
                self.search_input.delete();
                if self.search_input.is_empty() {
                    self.search.clear();
                } else {
                    self.update_search_matches(nav);
                }
                self.reset_match_selection();
                TreeBrowserAction::None
            }
            // Ctrl+U clears input
            InputAction::KillToStart => {
                self.search_input.clear();
                self.search.clear();
                self.reset_match_selection();
                TreeBrowserAction::None
            }
            _ => TreeBrowserAction::None,
        }
    }

    /// Handle Enter in search bar
    fn handle_search_enter(&mut self, nav: &mut TreeNavigator) {
        let match_count = self.search.matches.len();

        if match_count == 0 {
            // No results - do nothing, stay in search
            return;
        }

        if match_count == 1 {
            // Single result - focus it and return to tree
            let target = self.search.matches[0].clone();
            nav.navigate_to_path(&target);
            self.search_input.clear();
            self.search.clear();
            self.focus = CorpusBrowserFocus::TreeBrowser;
            self.search_input.focused = false;
        } else {
            // Multiple results - enter match selection modal
            self.match_selection_mode = true;
            self.match_selection_idx = 0;
        }
    }

    /// Handle input during match selection mode.
    fn handle_match_selection_input(
        &mut self,
        action: &InputAction,
        nav: &mut TreeNavigator,
    ) -> TreeBrowserAction {
        match action {
            InputAction::NavUp => {
                self.enter_match_and_up();
                TreeBrowserAction::None
            }
            InputAction::NavDown => {
                self.enter_match_and_down();
                TreeBrowserAction::None
            }
            InputAction::Confirm => {
                self.confirm_match_selection(nav);
                TreeBrowserAction::None
            }
            InputAction::Cancel => {
                self.reset_match_selection();
                TreeBrowserAction::None
            }
            _ => TreeBrowserAction::None,
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
                // File-level: simple popup
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
                // Directory-level: popup + pane
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
    fn cycle_packing_dirs(&mut self, nav: &mut TreeNavigator, forward: bool) -> bool {
        let entries = nav.entries();
        let packing_indices: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e.packing_marker, PackingMarker::Directory(_)))
            .map(|(i, _)| i)
            .collect();

        if packing_indices.is_empty() {
            return false;
        }

        let cursor = nav.cursor_idx();
        let target = if forward {
            packing_indices
                .iter()
                .find(|&&idx| idx > cursor)
                .or(packing_indices.first())
                .copied()
        } else {
            packing_indices
                .iter()
                .rev()
                .find(|&&idx| idx < cursor)
                .or(packing_indices.last())
                .copied()
        };

        if let Some(target_idx) = target {
            nav.set_cursor(target_idx);
            self.on_cursor_move(nav);
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
    // Search Methods
    // =========================================================================

    /// Start search from navigator root.
    /// Update search matches based on current input.
    fn update_search_matches(&mut self, nav: &TreeNavigator) {
        let query = self.search_input.value().to_lowercase();
        if query.is_empty() {
            self.search.matches.clear();
            self.search.first_match_idx = None;
            return;
        }

        let search_root = match &self.search.search_root {
            Some(root) => root.clone(),
            None => return,
        };

        // Find all matching items recursively
        let mut matches = Vec::new();
        Self::search_items_recursive(&search_root, &query, &mut matches, self.config.show_files);

        // Sort alphabetically by name
        matches.sort_by(|a, b| {
            let name_a = a.file_name().map(|n| n.to_string_lossy().to_lowercase());
            let name_b = b.file_name().map(|n| n.to_string_lossy().to_lowercase());
            name_a.cmp(&name_b)
        });

        // Find index in entries list for first match
        self.search.first_match_idx = matches
            .first()
            .and_then(|path| nav.entries().iter().position(|e| &e.path == path));

        self.search.matches = matches;
    }

    /// Recursively search for items matching query.
    fn search_items_recursive(
        dir: &Path,
        query: &str,
        matches: &mut Vec<PathBuf>,
        include_files: bool,
    ) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // Skip hidden items
                if name.starts_with('.') {
                    continue;
                }

                // Check if name contains query (case-insensitive)
                if name.to_lowercase().contains(query) && (path.is_dir() || include_files) {
                    matches.push(path.clone());
                }

                // Recurse into subdirectories
                if path.is_dir() {
                    Self::search_items_recursive(&path, query, matches, include_files);
                }
            }
        }
    }

    /// Get suggestion for tab completion (first match name).
    fn get_suggestion(&self) -> Option<String> {
        self.search
            .matches
            .first()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
    }

    fn reset_match_selection(&mut self) {
        self.match_selection_mode = false;
        self.match_selection_idx = 0;
    }

    /// Enter match selection mode (if not already) and move up.
    fn enter_match_and_up(&mut self) {
        if !self.match_selection_mode {
            self.match_selection_mode = true;
            self.match_selection_idx = 0;
        }
        if self.match_selection_idx > 0 {
            self.match_selection_idx -= 1;
        }
    }

    /// Enter match selection mode (if not already) and move down.
    fn enter_match_and_down(&mut self) {
        if !self.match_selection_mode {
            self.match_selection_mode = true;
            self.match_selection_idx = 0;
        }
        if self.match_selection_idx + 1 < self.search.matches.len() {
            self.match_selection_idx += 1;
        }
    }

    fn confirm_match_selection(&mut self, nav: &mut TreeNavigator) {
        if let Some(path) = self.search.matches.get(self.match_selection_idx).cloned() {
            nav.navigate_to_path(&path);
        }
        self.reset_match_selection();
        // Clear search and return to tree
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

        // Calculate modal size
        let max_width = matches
            .iter()
            .map(|p| p.to_string_lossy().len())
            .max()
            .unwrap_or(30)
            .min(60) as u16
            + 6;
        let height = (matches.len() as u16 + 4).min(area.height - 4);

        // Center modal
        let x = area.x + (area.width.saturating_sub(max_width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let modal_area = Rect::new(x, y, max_width, height);

        // Build lines
        let visible_height = (height - 4) as usize;
        let scroll_offset = self.match_selection_idx.saturating_sub(visible_height / 2);

        let lines: Vec<Line> = matches
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(visible_height)
            .map(|(idx, path)| {
                let path_str = truncate_left(&path.to_string_lossy(), (max_width - 6) as usize);
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
