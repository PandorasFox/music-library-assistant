//! Corpus Browser Variant
//!
//! Browse files and directories with persistent search bar and tag editor launch.

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::ui::helpers::truncate_left;
use crate::ui::widgets::TextInputState;

use crate::ui::tree_browser::actions::TreeBrowserAction;
use crate::ui::tree_browser::config::CorpusBrowserConfig;
use crate::ui::tree_browser::navigator::{EntryFilter, TreeNavigator};

/// Focus state for corpus browser
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CorpusBrowserFocus {
    /// Focus is on the tree browser
    #[default]
    TreeBrowser,
    /// Focus is on the search bar
    SearchBar,
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

/// Corpus browser variant state.
#[derive(Debug)]
pub struct CorpusBrowserVariant {
    /// Variant-specific configuration
    config: CorpusBrowserConfig,
    /// Current focus (tree browser or search bar)
    focus: CorpusBrowserFocus,
    /// Text input state for search bar
    search_input: TextInputState,
    /// Search results
    search: SearchState,
    /// Whether we're in match selection mode (multiple matches on enter)
    match_selection_mode: bool,
    /// Index in matches list when selecting
    match_selection_idx: usize,
}

impl CorpusBrowserVariant {
    /// Create a new corpus browser variant.
    pub fn new(config: CorpusBrowserConfig) -> Self {
        Self {
            config,
            focus: CorpusBrowserFocus::TreeBrowser,
            search_input: TextInputState::new(),
            search: SearchState::default(),
            match_selection_mode: false,
            match_selection_idx: 0,
        }
    }

    /// Get the entry filter for corpus browser (includes files).
    pub fn entry_filter(&self) -> EntryFilter {
        if self.config.show_files {
            EntryFilter::with_files()
        } else {
            EntryFilter::directories_only()
        }
    }

    /// Called when cursor moves - no-op now that preview pane is removed.
    pub fn on_cursor_move(&mut self, _nav: &TreeNavigator) {
        // Preview pane removed - nothing to update
    }

    /// Handle Escape - clear search and return focus to tree.
    pub fn handle_escape(&mut self) -> bool {
        if self.match_selection_mode {
            self.cancel_match_selection();
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
    /// Returns true when search is active (search bar focused or has results).
    pub fn wants_navigation_keys(&self) -> bool {
        self.match_selection_mode
            || self.focus == CorpusBrowserFocus::SearchBar
            || !self.search.matches.is_empty()
    }

    /// Handle variant-specific keys.
    pub fn handle_key(&mut self, key: KeyEvent, nav: &mut TreeNavigator) -> TreeBrowserAction {
        // Match selection mode has priority (modal overlay)
        if self.match_selection_mode {
            return self.handle_match_selection_key(key, nav);
        }

        // Route based on focus
        match self.focus {
            CorpusBrowserFocus::SearchBar => self.handle_search_bar_key(key, nav),
            CorpusBrowserFocus::TreeBrowser => self.handle_tree_browser_key(key, nav),
        }
    }

    /// Handle keys when tree browser is focused.
    fn handle_tree_browser_key(&mut self, key: KeyEvent, nav: &mut TreeNavigator) -> TreeBrowserAction {
        // If we have search results visible, capture navigation keys
        if !self.search.matches.is_empty() {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    if !self.match_selection_mode {
                        self.match_selection_mode = true;
                        self.match_selection_idx = 0;
                    }
                    self.match_selection_up();
                    return TreeBrowserAction::None;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if !self.match_selection_mode {
                        self.match_selection_mode = true;
                        self.match_selection_idx = 0;
                    }
                    self.match_selection_down();
                    return TreeBrowserAction::None;
                }
                // Consume h/l to prevent tree navigation when search results visible
                KeyCode::Char('h') | KeyCode::Char('l') | KeyCode::Left | KeyCode::Right => {
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
            // Any printable character starts search
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Move focus to search bar and insert character
                self.focus = CorpusBrowserFocus::SearchBar;
                self.search_input.focused = true;
                self.start_search(nav);
                self.search_input.insert_char(c);
                self.update_search_matches(nav);
                TreeBrowserAction::None
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
            KeyCode::Up | KeyCode::Char('k') => {
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
            KeyCode::Down | KeyCode::Char('j') => {
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
            // hjkl for navigation should not pass through
            KeyCode::Char('h') | KeyCode::Char('l') => {
                // Consume these to prevent tree browser navigation
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
            KeyCode::Up | KeyCode::Char('k') => {
                self.match_selection_up();
                TreeBrowserAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
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
    fn start_search(&mut self, nav: &TreeNavigator) {
        self.search.search_root = Some(nav.root_path().clone());
        self.search.matches.clear();
        self.search.first_match_idx = None;
    }

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
                if name.to_lowercase().contains(query) {
                    if path.is_dir() || include_files {
                        matches.push(path.clone());
                    }
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
    // Accessors
    // =========================================================================

    /// Get current focus.
    pub fn focus(&self) -> CorpusBrowserFocus {
        self.focus
    }

    /// Get search input state.
    pub fn search_input(&self) -> &TextInputState {
        &self.search_input
    }

    /// Get search results.
    pub fn search(&self) -> &SearchState {
        &self.search
    }

    /// Check if in match selection mode.
    pub fn is_match_selection_mode(&self) -> bool {
        self.match_selection_mode
    }

    /// Get match selection index.
    pub fn match_selection_idx(&self) -> usize {
        self.match_selection_idx
    }

    // =========================================================================
    // Rendering
    // =========================================================================

    /// Render the search bar.
    pub fn render_search_bar(&self, f: &mut Frame, area: Rect) {
        let is_focused = self.focus == CorpusBrowserFocus::SearchBar;

        let placeholder = "type to start searching files and directories";

        // Build search bar content
        let (_content, _style) = if self.search_input.is_empty() && !is_focused {
            (placeholder.to_string(), Style::default().fg(Color::DarkGray))
        } else if self.search_input.is_empty() && is_focused {
            // Focused but empty - show cursor
            let mut state = self.search_input.clone();
            state.focused = true;
            // Render via widget in the else block instead
            (String::new(), Style::default())
        } else {
            // Show search value with match count
            let match_count = self.search.matches.len();
            let count_suffix = if match_count > 0 {
                format!("  ({} matches)", match_count)
            } else if !self.search_input.is_empty() {
                "  (no matches)".to_string()
            } else {
                String::new()
            };

            // Build with cursor if focused
            if is_focused {
                // Will render with TextInput widget
                (String::new(), Style::default())
            } else {
                (format!("{}{}", self.search_input.value(), count_suffix), Style::default().fg(Color::White))
            }
        };

        let border_color = if is_focused { Color::Cyan } else { Color::DarkGray };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title("Search");

        if is_focused || !self.search_input.is_empty() {
            // Use custom rendering for cursor support
            let mut state = self.search_input.clone();
            state.focused = is_focused;

            let match_count = self.search.matches.len();
            let count_suffix = if match_count > 0 {
                format!("  ({} matches)", match_count)
            } else if !self.search_input.is_empty() {
                "  (no matches)".to_string()
            } else {
                String::new()
            };

            let count_style = if match_count > 0 {
                Style::default().fg(Color::Green)
            } else if !self.search_input.is_empty() {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            // Build line with cursor
            let line = if state.value.is_empty() && is_focused {
                Line::from(vec![
                    Span::styled(" ", Style::default().bg(Color::White).fg(Color::Black)),
                    Span::styled(&count_suffix, count_style),
                ])
            } else if is_focused {
                let chars: Vec<char> = state.value.chars().collect();
                let mut spans = Vec::new();

                // Text before cursor
                if state.cursor > 0 {
                    let before: String = chars[..state.cursor].iter().collect();
                    spans.push(Span::styled(before, Style::default().fg(Color::White)));
                }

                // Cursor character (or space if at end)
                if state.cursor < chars.len() {
                    let cursor_char = chars[state.cursor].to_string();
                    spans.push(Span::styled(cursor_char, Style::default().bg(Color::White).fg(Color::Black)));

                    // Text after cursor
                    if state.cursor + 1 < chars.len() {
                        let after: String = chars[state.cursor + 1..].iter().collect();
                        spans.push(Span::styled(after, Style::default().fg(Color::White)));
                    }
                } else {
                    spans.push(Span::styled(" ", Style::default().bg(Color::White).fg(Color::Black)));
                }

                spans.push(Span::styled(count_suffix, count_style));
                Line::from(spans)
            } else {
                Line::from(vec![
                    Span::styled(state.value.as_str(), Style::default().fg(Color::White)),
                    Span::styled(&count_suffix, count_style),
                ])
            };

            let paragraph = Paragraph::new(line).block(block);
            f.render_widget(paragraph, area);
        } else {
            // Show placeholder
            let paragraph = Paragraph::new(Span::styled(placeholder, Style::default().fg(Color::DarkGray)))
                .block(block);
            f.render_widget(paragraph, area);
        }
    }

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
