//! Corpus Browser Variant
//!
//! Browse files and directories with metadata preview, type-to-jump search,
//! and launch tag editor on Enter.

use std::fs;
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::ui::helpers::truncate_left;

use crate::ui::tree_browser::actions::TreeBrowserAction;
use crate::ui::tree_browser::config::CorpusBrowserConfig;
use crate::ui::tree_browser::navigator::{EntryFilter, TreeNavigator};

/// Metadata about an audio file for the preview pane.
#[derive(Debug, Clone, Default)]
pub struct FileMetadata {
    /// Bitrate in kbps
    pub bitrate_kbps: Option<u32>,
    /// Duration in milliseconds
    pub duration_ms: Option<u64>,
    /// Sample rate in Hz
    pub sample_rate: Option<u32>,
    /// File size in bytes
    pub file_size: u64,
    /// File format/extension
    pub file_type: String,
    /// Tag key-value pairs
    pub tags: Vec<(String, String)>,
}

/// Search state for type-to-jump functionality.
#[derive(Debug, Clone, Default)]
pub struct SearchState {
    /// Current search query
    pub query: String,
    /// Directory being searched (the one hovered when search started)
    pub search_root: Option<PathBuf>,
    /// Matching directory paths found
    pub matches: Vec<PathBuf>,
    /// First alphabetically matching directory name (for suggestion)
    pub suggestion: Option<String>,
    /// Index in entries list for first match
    pub first_match_idx: Option<usize>,
}

impl SearchState {
    /// Clear all search state.
    pub fn clear(&mut self) {
        self.query.clear();
        self.search_root = None;
        self.matches.clear();
        self.suggestion = None;
        self.first_match_idx = None;
    }

    /// Check if search is currently active.
    pub fn is_active(&self) -> bool {
        !self.query.is_empty() || self.search_root.is_some()
    }
}

/// Corpus browser variant state.
#[derive(Debug)]
pub struct CorpusBrowserVariant {
    /// Variant-specific configuration
    config: CorpusBrowserConfig,
    /// Cached metadata for focused file
    cached_metadata: Option<FileMetadata>,
    /// Path of cached metadata
    cached_path: Option<PathBuf>,
    /// Search state for type-to-jump
    search: SearchState,
    /// Whether we're in match selection mode (multiple matches)
    match_selection_mode: bool,
    /// Index in matches list when selecting
    match_selection_idx: usize,
}

impl CorpusBrowserVariant {
    /// Create a new corpus browser variant.
    pub fn new(config: CorpusBrowserConfig) -> Self {
        Self {
            config,
            cached_metadata: None,
            cached_path: None,
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

    /// Called when cursor moves - update cached metadata.
    pub fn on_cursor_move(&mut self, nav: &TreeNavigator) {
        self.update_cached_metadata(nav);
    }

    /// Handle Escape - cancel search if active.
    pub fn handle_escape(&mut self) -> bool {
        if self.match_selection_mode {
            self.cancel_match_selection();
            true
        } else if self.search.is_active() {
            self.cancel_search();
            true
        } else {
            false
        }
    }

    /// Handle variant-specific keys.
    pub fn handle_key(&mut self, key: KeyEvent, nav: &mut TreeNavigator) -> TreeBrowserAction {
        // Match selection mode has priority
        if self.match_selection_mode {
            return self.handle_match_selection_key(key, nav);
        }

        // Search mode handling
        if self.search.is_active() {
            return self.handle_search_key(key, nav);
        }

        // Normal mode
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
            KeyCode::Char(c) if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' => {
                self.search_push_char(c, nav);
                TreeBrowserAction::None
            }
            _ => TreeBrowserAction::None,
        }
    }

    /// Handle keys during search mode.
    fn handle_search_key(&mut self, key: KeyEvent, nav: &mut TreeNavigator) -> TreeBrowserAction {
        match key.code {
            KeyCode::Char(c) if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' => {
                self.search_push_char(c, nav);
                TreeBrowserAction::None
            }
            KeyCode::Backspace => {
                self.search_pop_char();
                TreeBrowserAction::None
            }
            KeyCode::Tab => {
                self.apply_suggestion();
                TreeBrowserAction::None
            }
            KeyCode::Enter => {
                self.jump_to_match(nav);
                TreeBrowserAction::None
            }
            KeyCode::Esc => {
                self.cancel_search();
                TreeBrowserAction::None
            }
            _ => TreeBrowserAction::None,
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

    /// Start search mode from current directory.
    fn start_search(&mut self, nav: &TreeNavigator) {
        let search_root = if let Some(entry) = nav.current_entry() {
            if entry.is_directory {
                Some(entry.path.clone())
            } else {
                entry.path.parent().map(|p| p.to_path_buf())
            }
        } else {
            Some(nav.root_path().clone())
        };

        self.search.search_root = search_root;
        self.search.query.clear();
        self.search.matches.clear();
        self.search.suggestion = None;
        self.search.first_match_idx = None;
    }

    /// Add a character to search query and update matches.
    fn search_push_char(&mut self, c: char, nav: &TreeNavigator) {
        if !self.search.is_active() {
            self.start_search(nav);
        }
        self.search.query.push(c);
        self.update_search_matches(nav);
    }

    /// Remove a character from search query.
    fn search_pop_char(&mut self) {
        if self.search.query.pop().is_some() {
            if self.search.query.is_empty() {
                self.search.clear();
            }
        }
    }

    /// Cancel search mode.
    fn cancel_search(&mut self) {
        self.search.clear();
        self.match_selection_mode = false;
        self.match_selection_idx = 0;
    }

    /// Update search matches based on current query.
    fn update_search_matches(&mut self, nav: &TreeNavigator) {
        let query_lower = self.search.query.to_lowercase();
        if query_lower.is_empty() {
            self.search.matches.clear();
            self.search.suggestion = None;
            self.search.first_match_idx = None;
            return;
        }

        let search_root = match &self.search.search_root {
            Some(root) => root.clone(),
            None => return,
        };

        // Find all matching directories recursively
        let mut matches = Vec::new();
        Self::search_directories_recursive(&search_root, &query_lower, &mut matches);

        // Sort alphabetically by directory name
        matches.sort_by(|a, b| {
            let name_a = a.file_name().map(|n| n.to_string_lossy().to_lowercase());
            let name_b = b.file_name().map(|n| n.to_string_lossy().to_lowercase());
            name_a.cmp(&name_b)
        });

        // Set suggestion to first match's name
        self.search.suggestion = matches
            .first()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));

        // Find index in entries list for first match
        self.search.first_match_idx = matches
            .first()
            .and_then(|path| nav.entries().iter().position(|e| &e.path == path));

        self.search.matches = matches;
    }

    /// Recursively search for directories matching query.
    fn search_directories_recursive(dir: &Path, query: &str, matches: &mut Vec<PathBuf>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // Skip hidden directories
                if name.starts_with('.') {
                    continue;
                }

                // Check if name contains query (case-insensitive)
                if name.to_lowercase().contains(query) {
                    matches.push(path.clone());
                }

                // Recurse into subdirectories
                Self::search_directories_recursive(&path, query, matches);
            }
        }
    }

    /// Apply current suggestion (Tab key).
    fn apply_suggestion(&mut self) {
        if let Some(suggestion) = &self.search.suggestion {
            self.search.query = suggestion.clone();
        }
    }

    /// Jump to match (Enter key).
    fn jump_to_match(&mut self, nav: &mut TreeNavigator) {
        if self.search.matches.is_empty() {
            return;
        }

        if self.search.matches.len() == 1 {
            // Single match - jump directly
            let target_path = self.search.matches[0].clone();
            nav.navigate_to_path(&target_path);
            self.cancel_search();
        } else {
            // Multiple matches - enter selection mode
            self.match_selection_mode = true;
            self.match_selection_idx = 0;
        }
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
        self.cancel_search();
    }

    fn cancel_match_selection(&mut self) {
        self.match_selection_mode = false;
        self.match_selection_idx = 0;
    }

    // =========================================================================
    // Metadata
    // =========================================================================

    /// Update cached metadata for current entry.
    fn update_cached_metadata(&mut self, nav: &TreeNavigator) {
        let current_path = nav.current_entry().map(|e| e.path.clone());

        if current_path != self.cached_path {
            self.cached_path = current_path.clone();
            self.cached_metadata = current_path.and_then(|p| {
                if p.is_file() {
                    Some(Self::load_file_metadata(&p))
                } else {
                    None
                }
            });
        }
    }

    /// Load metadata for a file.
    fn load_file_metadata(path: &Path) -> FileMetadata {
        let file_size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let file_type = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();

        let (bitrate_kbps, duration_ms, sample_rate, tags) =
            if let Ok(tagged_file) = lofty::read_from_path(path) {
                let props = tagged_file.properties();
                let bitrate = props.audio_bitrate();
                let duration = props.duration().as_millis() as u64;
                let sample = props.sample_rate();

                let mut tag_vec = Vec::new();
                if let Some(tag) = tagged_file.primary_tag() {
                    if let Some(v) = tag.artist() {
                        tag_vec.push(("artist".to_string(), v.to_string()));
                    }
                    if let Some(v) = tag.album() {
                        tag_vec.push(("album".to_string(), v.to_string()));
                    }
                    if let Some(v) = tag.title() {
                        tag_vec.push(("title".to_string(), v.to_string()));
                    }
                    if let Some(v) = tag.track() {
                        tag_vec.push(("track_number".to_string(), v.to_string()));
                    }
                    if let Some(v) = tag.genre() {
                        tag_vec.push(("genre".to_string(), v.to_string()));
                    }
                    if let Some(v) = tag.year() {
                        tag_vec.push(("date".to_string(), v.to_string()));
                    }
                }

                (bitrate, Some(duration), sample, tag_vec)
            } else {
                (None, None, None, Vec::new())
            };

        FileMetadata {
            bitrate_kbps,
            duration_ms,
            sample_rate,
            file_size,
            file_type,
            tags,
        }
    }

    // =========================================================================
    // Accessors
    // =========================================================================

    /// Get cached metadata.
    pub fn cached_metadata(&self) -> Option<&FileMetadata> {
        self.cached_metadata.as_ref()
    }

    /// Get search state.
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

    /// Render variant-specific overlays.
    pub fn render_overlays(&self, f: &mut Frame, area: Rect) {
        if self.match_selection_mode {
            self.render_match_modal(f, area);
        } else if self.search.is_active() {
            self.render_search_popup(f, area);
        }
    }

    /// Render search popup near top of screen.
    fn render_search_popup(&self, f: &mut Frame, area: Rect) {
        let popup_width = 50u16;
        let popup_height = 3u16;

        // Position near top, centered horizontally
        let x = area.x + (area.width.saturating_sub(popup_width)) / 2;
        let y = area.y + 2;

        let popup_area = Rect::new(x, y, popup_width, popup_height);

        // Build content
        let suggestion_suffix = if let Some(ref suggestion) = self.search.suggestion {
            if suggestion.to_lowercase().starts_with(&self.search.query.to_lowercase()) {
                suggestion[self.search.query.len()..].to_string()
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let match_count = self.search.matches.len();
        let count_style = if match_count > 0 {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::Red)
        };

        let line = Line::from(vec![
            Span::raw(&self.search.query),
            Span::styled(&suggestion_suffix, Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled(format!("({} matches)", match_count), count_style),
        ]);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan))
            .title("Search");

        f.render_widget(Clear, popup_area);
        f.render_widget(Paragraph::new(line).block(block), popup_area);
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

    /// Render the preview pane (right side).
    pub fn render_preview_pane(&self, f: &mut Frame, area: Rect, nav: &TreeNavigator) {
        let block = Block::default().borders(Borders::ALL).title("Preview");

        let content = if let Some(entry) = nav.current_entry() {
            if entry.is_directory {
                self.build_directory_preview(entry, nav)
            } else if let Some(ref metadata) = self.cached_metadata {
                self.build_file_preview(entry, metadata)
            } else {
                vec![Line::raw("Loading metadata...")]
            }
        } else {
            vec![Line::raw("No selection")]
        };

        let paragraph = Paragraph::new(content).block(block);
        f.render_widget(paragraph, area);
    }

    fn build_directory_preview(
        &self,
        entry: &super::super::entry::TreeEntry,
        _nav: &TreeNavigator,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        lines.push(Line::styled(
            "Directory",
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::raw(""));

        let path_str = entry.path.to_string_lossy().to_string();
        lines.push(Line::raw(format!("Path: {}", path_str)));

        if entry.item_count > 0 {
            lines.push(Line::raw(format!("Audio files: {}", entry.item_count)));
        }

        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Enter: Edit tracks in this directory",
            Style::default().fg(Color::DarkGray),
        ));

        lines
    }

    fn build_file_preview(
        &self,
        entry: &super::super::entry::TreeEntry,
        metadata: &FileMetadata,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        lines.push(Line::styled(
            entry.name.clone(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::raw(""));

        // Technical info
        lines.push(Line::raw(format!(
            "Type: {} | Size: {}",
            metadata.file_type,
            format_file_size(metadata.file_size)
        )));

        if let Some(bitrate) = metadata.bitrate_kbps {
            let duration_str = metadata
                .duration_ms
                .map(format_duration)
                .unwrap_or_else(|| "?".to_string());
            let sample_str = metadata
                .sample_rate
                .map(|s| format!("{}Hz", s))
                .unwrap_or_else(|| "?".to_string());
            lines.push(Line::raw(format!(
                "{}kbps | {} | {}",
                bitrate, duration_str, sample_str
            )));
        }

        lines.push(Line::raw(""));

        // Tags
        for (key, value) in &metadata.tags {
            lines.push(Line::raw(format!("{}: {}", key, value)));
        }

        if metadata.tags.is_empty() {
            lines.push(Line::styled(
                "(no tags found)",
                Style::default().fg(Color::DarkGray),
            ));
        }

        lines
    }
}

// =========================================================================
// Helpers
// =========================================================================

fn format_file_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1}GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.1}MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1}KB", bytes as f64 / 1_000.0)
    } else {
        format!("{}B", bytes)
    }
}

fn format_duration(ms: u64) -> String {
    let seconds = ms / 1000;
    let minutes = seconds / 60;
    let secs = seconds % 60;
    format!("{}:{:02}", minutes, secs)
}
