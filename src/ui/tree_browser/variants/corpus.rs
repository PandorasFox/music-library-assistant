//! Corpus Browser Variant
//!
//! Browse files and directories with persistent search bar and tag editor launch.

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::ui::helpers::truncate_left;
use crate::ui::widgets::TextInputState;
use crate::ui::widgets::detail_panel::{DetailField, DetailWidget, PanelButton, render_detail_panel};

use crate::ui::tree_browser::actions::TreeBrowserAction;
use crate::ui::tree_browser::config::CorpusBrowserConfig;
use crate::ui::tree_browser::navigator::TreeNavigator;

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

/// State for the directory config panel.
#[derive(Debug)]
pub struct DirConfigPanelState {
    /// Which source directory we're editing (relative path).
    pub source_path: PathBuf,
    // Editable copies:
    pub libraries: Vec<String>,
    pub can_stash_dupes: bool,
    pub interior_dupes: bool,
    // Originals for dirty checking:
    pub orig_libraries: Vec<String>,
    pub orig_can_stash_dupes: bool,
    pub orig_interior_dupes: bool,
    // UI state:
    /// 0=libraries, 1=can_stash_dupes, 2=interior_dupes
    pub field_cursor: usize,
    pub focus: PanelFocus,
    /// 0=Save, 1=Discard
    pub button_cursor: usize,
    /// Selected library item within the libraries list (None = field label focused).
    pub lib_cursor: Option<usize>,
    /// Active text input for library editing/adding.
    pub text_input: Option<TextInputState>,
}

impl DirConfigPanelState {
    /// Whether any edits have been made.
    pub fn has_edits(&self) -> bool {
        self.libraries != self.orig_libraries
            || self.can_stash_dupes != self.orig_can_stash_dupes
            || self.interior_dupes != self.orig_interior_dupes
    }

    /// Render the config panel using the detail_panel widget.
    pub fn render_config_panel(&self, f: &mut Frame, area: Rect) {
        let path_str = self.source_path.display().to_string();
        let title = format!(" Dir: {} ", path_str);
        let border_color = if self.has_edits() { Color::Yellow } else { Color::Cyan };

        let libs_edited = self.libraries != self.orig_libraries;
        let stash_edited = self.can_stash_dupes != self.orig_can_stash_dupes;
        let interior_edited = self.interior_dupes != self.orig_interior_dupes;

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
                widget: DetailWidget::Bool { value: self.can_stash_dupes, edited: stash_edited },
            },
            DetailField {
                label: "Interior dupes",
                widget: DetailWidget::Bool { value: self.interior_dupes, edited: interior_edited },
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

        let hint = if self.text_input.is_some() {
            Some("Enter confirm  Esc cancel")
        } else if self.focus == PanelFocus::Buttons {
            Some("Enter select  Up fields")
        } else if self.field_cursor == 0 {
            Some("n add  x del  Enter edit  Tab buttons")
        } else {
            Some("Enter/Space toggle  Tab buttons")
        };

        render_detail_panel(
            f,
            area,
            &title,
            border_color,
            &fields,
            self.field_cursor,
            &buttons,
            self.focus == PanelFocus::Buttons,
            hint,
        );
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

    /// Called when cursor moves - no-op now that preview pane is removed.
    pub fn on_cursor_move(&mut self, _nav: &TreeNavigator) {
        // Preview pane removed - nothing to update
    }

    /// Handle Escape - clear filter/search and return focus to tree.
    pub fn handle_escape(&mut self, nav: &mut TreeNavigator) -> bool {
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

        if self.match_selection_mode {
            self.cancel_match_selection();
            true
        } else if self.focus == CorpusBrowserFocus::SearchBar || !self.search_input.is_empty() {
            // Clear search and return to tree
            self.search_input.clear();
            self.search.clear();
            self.focus = CorpusBrowserFocus::TreeBrowser;
            true
        } else if nav.has_path_filter() {
            // Clear active filter
            nav.clear_path_filter();
            true
        } else {
            false
        }
    }

    /// Check if variant wants to capture navigation keys.
    /// Returns true when search is active (search bar focused or has results)
    /// or when config panel is focused.
    pub fn wants_navigation_keys(&self) -> bool {
        self.focus == CorpusBrowserFocus::ConfigPanel
            || self.match_selection_mode
            || self.focus == CorpusBrowserFocus::SearchBar
            || !self.search.matches.is_empty()
    }

    /// Handle variant-specific keys.
    pub fn handle_key(&mut self, key: KeyEvent, nav: &mut TreeNavigator) -> TreeBrowserAction {
        // Config panel has priority when focused
        if self.focus == CorpusBrowserFocus::ConfigPanel {
            return self.handle_config_panel_key(key);
        }

        // Match selection mode has priority (modal overlay)
        if self.match_selection_mode {
            return self.handle_match_selection_key(key, nav);
        }

        // Route based on focus
        match self.focus {
            CorpusBrowserFocus::SearchBar => self.handle_search_bar_key(key, nav),
            CorpusBrowserFocus::TreeBrowser => self.handle_tree_browser_key(key, nav),
            CorpusBrowserFocus::ConfigPanel => unreachable!(),
        }
    }

    /// Handle keys when tree browser is focused.
    fn handle_tree_browser_key(&mut self, key: KeyEvent, nav: &mut TreeNavigator) -> TreeBrowserAction {
        // If we have search results visible, capture navigation keys
        if !self.search.matches.is_empty() {
            match key.code {
                KeyCode::Up => {
                    if !self.match_selection_mode {
                        self.match_selection_mode = true;
                        self.match_selection_idx = 0;
                    }
                    self.match_selection_up();
                    return TreeBrowserAction::None;
                }
                KeyCode::Down => {
                    if !self.match_selection_mode {
                        self.match_selection_mode = true;
                        self.match_selection_idx = 0;
                    }
                    self.match_selection_down();
                    return TreeBrowserAction::None;
                }
                // Consume Left/Right to prevent tree navigation when search results visible
                KeyCode::Left | KeyCode::Right => {
                    return TreeBrowserAction::None;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Enter => {
                if let Some(entry) = nav.current_entry() {
                    if entry.is_directory {
                        TreeBrowserAction::EditDirectory(entry.path.clone())
                    } else {
                        TreeBrowserAction::EditFile(entry.path.clone())
                    }
                } else {
                    TreeBrowserAction::None
                }
            }
            // C opens dir config panel on any corpus directory
            KeyCode::Char('C') => {
                if let Some(entry) = nav.current_entry() {
                    if entry.is_directory && entry.path.starts_with(&self.corpus_dir) {
                        return TreeBrowserAction::OpenDirConfig(entry.path.clone());
                    }
                }
                TreeBrowserAction::None
            }
            // Ctrl+F opens filter popup
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                TreeBrowserAction::OpenFilter
            }
            _ => TreeBrowserAction::None,
        }
    }

    /// Handle keys when config panel is focused.
    fn handle_config_panel_key(&mut self, key: KeyEvent) -> TreeBrowserAction {
        let panel = match self.config_panel {
            Some(ref mut p) => p,
            None => return TreeBrowserAction::None,
        };

        // Text input mode intercepts all keys
        if let Some(ref mut input) = panel.text_input {
            match key.code {
                KeyCode::Enter => {
                    let value = input.value().to_string();
                    if !value.is_empty() {
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
                KeyCode::Esc => {
                    panel.text_input = None;
                    return TreeBrowserAction::None;
                }
                _ => {
                    input.handle_key(key);
                    return TreeBrowserAction::None;
                }
            }
        }

        // Button focus
        if panel.focus == PanelFocus::Buttons {
            match key.code {
                KeyCode::Left => {
                    if panel.button_cursor > 0 {
                        panel.button_cursor -= 1;
                    }
                    return TreeBrowserAction::None;
                }
                KeyCode::Right => {
                    if panel.button_cursor < 1 {
                        panel.button_cursor += 1;
                    }
                    return TreeBrowserAction::None;
                }
                KeyCode::Up => {
                    panel.focus = PanelFocus::Fields;
                    return TreeBrowserAction::None;
                }
                KeyCode::Enter => {
                    if panel.button_cursor == 0 {
                        // Save
                        return TreeBrowserAction::SaveDirConfig;
                    } else {
                        // Discard
                        return TreeBrowserAction::CloseDirConfig;
                    }
                }
                KeyCode::Esc => {
                    panel.focus = PanelFocus::Fields;
                    return TreeBrowserAction::None;
                }
                _ => return TreeBrowserAction::None,
            }
        }

        // Field focus
        match key.code {
            KeyCode::Up => {
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
            KeyCode::Down => {
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
                } else if panel.field_cursor < 2 {
                    panel.field_cursor += 1;
                }
                TreeBrowserAction::None
            }
            KeyCode::Tab => {
                panel.focus = PanelFocus::Buttons;
                panel.button_cursor = 0;
                TreeBrowserAction::None
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
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
                        panel.can_stash_dupes = !panel.can_stash_dupes;
                    }
                    2 => {
                        panel.interior_dupes = !panel.interior_dupes;
                    }
                    _ => {}
                }
                TreeBrowserAction::None
            }
            // n: add new library
            KeyCode::Char('n') => {
                if panel.field_cursor == 0 {
                    panel.lib_cursor = None; // New item, no cursor position
                    panel.text_input = Some(TextInputState::new());
                }
                TreeBrowserAction::None
            }
            // x: delete selected library
            KeyCode::Char('x') => {
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
            KeyCode::Esc => {
                TreeBrowserAction::CloseDirConfig
            }
            _ => TreeBrowserAction::None,
        }
    }

    /// Handle keys when search bar is focused.
    fn handle_search_bar_key(&mut self, key: KeyEvent, nav: &mut TreeNavigator) -> TreeBrowserAction {
        match key.code {
            KeyCode::Enter => {
                self.handle_search_enter(nav);
                TreeBrowserAction::None
            }
            KeyCode::Esc => {
                // Clear search and return to tree
                self.search_input.clear();
                self.search.clear();
                self.focus = CorpusBrowserFocus::TreeBrowser;
                self.search_input.focused = false;
                TreeBrowserAction::None
            }
            KeyCode::Tab => {
                // Apply suggestion if available
                if let Some(suggestion) = self.get_suggestion() {
                    self.search_input.set_value(suggestion);
                    self.update_search_matches(nav);
                }
                TreeBrowserAction::None
            }
            // Up/Down navigate search results (if any), or do nothing
            KeyCode::Up => {
                if !self.search.matches.is_empty() {
                    // Enter match selection mode and navigate
                    if !self.match_selection_mode {
                        self.match_selection_mode = true;
                        self.match_selection_idx = 0;
                    }
                    self.match_selection_up();
                }
                TreeBrowserAction::None
            }
            KeyCode::Down => {
                if !self.search.matches.is_empty() {
                    // Enter match selection mode and navigate
                    if !self.match_selection_mode {
                        self.match_selection_mode = true;
                        self.match_selection_idx = 0;
                    }
                    self.match_selection_down();
                }
                TreeBrowserAction::None
            }
            // Arrow keys for cursor navigation in text input
            KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End => {
                self.search_input.handle_key(key);
                TreeBrowserAction::None
            }
            // Character input
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_input.insert_char(c);
                self.update_search_matches(nav);
                // Reset match selection when typing
                self.match_selection_mode = false;
                self.match_selection_idx = 0;
                TreeBrowserAction::None
            }
            KeyCode::Backspace => {
                self.search_input.backspace();
                if self.search_input.is_empty() {
                    self.search.clear();
                } else {
                    self.update_search_matches(nav);
                }
                // Reset match selection when typing
                self.match_selection_mode = false;
                self.match_selection_idx = 0;
                TreeBrowserAction::None
            }
            KeyCode::Delete => {
                self.search_input.delete();
                if self.search_input.is_empty() {
                    self.search.clear();
                } else {
                    self.update_search_matches(nav);
                }
                // Reset match selection when typing
                self.match_selection_mode = false;
                self.match_selection_idx = 0;
                TreeBrowserAction::None
            }
            // Ctrl+U clears input
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_input.clear();
                self.search.clear();
                self.match_selection_mode = false;
                self.match_selection_idx = 0;
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

    /// Handle keys during match selection mode.
    fn handle_match_selection_key(
        &mut self,
        key: KeyEvent,
        nav: &mut TreeNavigator,
    ) -> TreeBrowserAction {
        match key.code {
            KeyCode::Up => {
                self.match_selection_up();
                TreeBrowserAction::None
            }
            KeyCode::Down => {
                self.match_selection_down();
                TreeBrowserAction::None
            }
            KeyCode::Enter => {
                self.confirm_match_selection(nav);
                TreeBrowserAction::None
            }
            KeyCode::Esc => {
                self.cancel_match_selection();
                TreeBrowserAction::None
            }
            _ => TreeBrowserAction::None,
        }
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
    fn search_items_recursive(dir: &Path, query: &str, matches: &mut Vec<PathBuf>, include_files: bool) {
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
        self.search.matches
            .first()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
    }

    fn match_selection_up(&mut self) {
        if self.match_selection_idx > 0 {
            self.match_selection_idx -= 1;
        }
    }

    fn match_selection_down(&mut self) {
        if self.match_selection_idx + 1 < self.search.matches.len() {
            self.match_selection_idx += 1;
        }
    }

    fn confirm_match_selection(&mut self, nav: &mut TreeNavigator) {
        if let Some(path) = self.search.matches.get(self.match_selection_idx).cloned() {
            nav.navigate_to_path(&path);
        }
        self.cancel_match_selection();
        // Clear search and return to tree
        self.search_input.clear();
        self.search.clear();
        self.focus = CorpusBrowserFocus::TreeBrowser;
        self.search_input.focused = false;
    }

    fn cancel_match_selection(&mut self) {
        self.match_selection_mode = false;
        self.match_selection_idx = 0;
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
                    "▶ "
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
            .title(format!("Select Match ({}/{})", self.match_selection_idx + 1, matches.len()));

        f.render_widget(Clear, modal_area);
        f.render_widget(Paragraph::new(all_lines).block(block), modal_area);
    }
}
