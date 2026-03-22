//! DirectoryBrowser — backend-agnostic tree browser widget.
//!
//! Manages a lazily-loaded, flattened tree of directory/file entries with
//! typed actions for expand, collapse, and selection. The app layer dispatches
//! protocol queries in response to actions and feeds results back.

use std::collections::HashSet;

use crate::input::InputAction;
use crate::text_input::TextInputState;
use mm_meta::domain_query_types::DirectoryListingEntry;

// ============================================================================
// BrowserEntry
// ============================================================================

/// A single entry in the flattened tree (directory or file).
#[derive(Debug, Clone)]
pub struct BrowserEntry {
    /// Relative path within zone.
    pub path: String,
    /// Display name (last path component).
    pub name: String,
    /// Indentation depth.
    pub depth: usize,
    /// True for directories, false for files.
    pub is_dir: bool,
    /// Whether this directory is expanded (directories only).
    pub expanded: bool,
    /// Direct child audio file count (directories only).
    pub file_count: usize,
    /// Inode (files only).
    pub inode: Option<i64>,
    /// Duration in milliseconds (files only).
    pub duration_ms: Option<i64>,
    /// Bitrate in kbps (files only).
    pub bitrate_kbps: Option<i32>,
}

// ============================================================================
// BrowserAction
// ============================================================================

/// Actions produced by the DirectoryBrowser widget.
///
/// The app layer dispatches protocol queries for data-fetching actions
/// and feeds results back via `populate_*` methods.
#[derive(Debug, Clone)]
pub enum BrowserAction {
    /// Directory needs children loaded. Carries relative path.
    RequestExpand(String),
    /// Collapse expanded directory at cursor.
    Collapse,
    /// File selected (Enter on file). Carries inode.
    SelectFile(i64),
    /// Directory selected (Enter on dir). Carries relative path.
    SelectDirectory(String),
    /// Text search requested. Carries query string.
    RequestSearch(String),
}

// ============================================================================
// DirectoryBrowser
// ============================================================================

/// Backend-agnostic tree browser interaction state.
///
/// Entries are populated lazily via `populate_root` and `populate_children`.
/// Input handling produces `BrowserAction`s that the app layer dispatches.
#[derive(Debug)]
pub struct DirectoryBrowser {
    /// Flattened visible entries.
    pub entries: Vec<BrowserEntry>,
    /// Current cursor position.
    pub cursor: usize,
    /// Scroll offset for viewport.
    pub scroll: usize,
    /// Viewport height (set by render layer).
    pub visible_height: usize,
    /// Inline search state.
    pub search: TextInputState,
    /// Whether the search bar is active.
    pub search_active: bool,
    /// Current zone label (e.g. "corpus", "library").
    pub current_zone: String,
    /// Active path filter (if any) — set of matching relative paths.
    path_filter: Option<HashSet<String>>,
}

impl DirectoryBrowser {
    /// Create a new browser for a given zone.
    pub fn new(zone: &str) -> Self {
        Self {
            entries: Vec::new(),
            cursor: 0,
            scroll: 0,
            visible_height: 20,
            search: TextInputState::new(),
            search_active: false,
            current_zone: zone.to_string(),
            path_filter: None,
        }
    }

    /// Handle a semantic input action.
    ///
    /// Returns `Some(action)` when the app layer needs to do something
    /// (fetch data, navigate, etc). Returns `None` when input is consumed
    /// internally (cursor movement, scroll).
    pub fn handle_input(&mut self, action: &InputAction) -> Option<BrowserAction> {
        // Search bar captures all input when active
        if self.search_active {
            return self.handle_search_input(action);
        }

        match action {
            InputAction::NavUp => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.ensure_visible();
                }
                None
            }
            InputAction::NavDown => {
                if self.cursor + 1 < self.entries.len() {
                    self.cursor += 1;
                    self.ensure_visible();
                }
                None
            }
            InputAction::NavRight => {
                // Expand collapsed directory, or no-op on file/expanded dir
                if let Some(entry) = self.entries.get(self.cursor) {
                    if entry.is_dir && !entry.expanded {
                        return Some(BrowserAction::RequestExpand(entry.path.clone()));
                    }
                }
                None
            }
            InputAction::NavLeft => {
                // Collapse expanded directory, or jump to parent
                if let Some(entry) = self.entries.get(self.cursor) {
                    if entry.is_dir && entry.expanded {
                        return Some(BrowserAction::Collapse);
                    } else if entry.depth > 0 {
                        self.jump_to_parent();
                    }
                }
                None
            }
            InputAction::Confirm => {
                if let Some(entry) = self.entries.get(self.cursor) {
                    if entry.is_dir {
                        return Some(BrowserAction::SelectDirectory(entry.path.clone()));
                    } else if let Some(inode) = entry.inode {
                        return Some(BrowserAction::SelectFile(inode));
                    }
                }
                None
            }
            InputAction::Home => {
                self.cursor = 0;
                self.ensure_visible();
                None
            }
            InputAction::End => {
                if !self.entries.is_empty() {
                    self.cursor = self.entries.len() - 1;
                    self.ensure_visible();
                }
                None
            }
            InputAction::PageUp => {
                let jump = self.visible_height.saturating_sub(1).max(1);
                self.cursor = self.cursor.saturating_sub(jump);
                self.ensure_visible();
                None
            }
            InputAction::PageDown => {
                let jump = self.visible_height.saturating_sub(1).max(1);
                self.cursor = (self.cursor + jump).min(self.entries.len().saturating_sub(1));
                self.ensure_visible();
                None
            }
            InputAction::OpenFilter => {
                // Ctrl+/ activates search bar
                self.search_active = true;
                self.search.focused = true;
                None
            }
            _ => None,
        }
    }

    fn handle_search_input(&mut self, action: &InputAction) -> Option<BrowserAction> {
        match action {
            InputAction::Cancel => {
                self.search_active = false;
                self.search.focused = false;
                None
            }
            InputAction::Confirm => {
                let query = self.search.value().to_string();
                self.search_active = false;
                self.search.focused = false;
                if !query.is_empty() {
                    Some(BrowserAction::RequestSearch(query))
                } else {
                    None
                }
            }
            _ => {
                self.search.handle_input(action);
                None
            }
        }
    }

    /// Jump cursor to parent directory (entry with depth - 1 above cursor).
    fn jump_to_parent(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor) {
            if entry.depth > 0 {
                let target_depth = entry.depth - 1;
                for i in (0..self.cursor).rev() {
                    if self.entries[i].depth == target_depth && self.entries[i].is_dir {
                        self.cursor = i;
                        self.ensure_visible();
                        break;
                    }
                }
            }
        }
    }

    /// Ensure cursor is visible in viewport.
    fn ensure_visible(&mut self) {
        if self.visible_height == 0 {
            return;
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + self.visible_height {
            self.scroll = self.cursor - self.visible_height + 1;
        }
    }

    // =========================================================================
    // Data population
    // =========================================================================

    /// Set root-level entries (response to initial load).
    pub fn populate_root(&mut self, entries: Vec<DirectoryListingEntry>) {
        self.entries = entries
            .into_iter()
            .map(|e| listing_to_browser_entry(e, 0))
            .collect();
        self.cursor = 0;
        self.scroll = 0;
    }

    /// Insert children under a parent directory (response to RequestExpand).
    ///
    /// Finds the parent by path, marks it expanded, inserts children at depth+1.
    pub fn populate_children(&mut self, parent_path: &str, children: Vec<DirectoryListingEntry>) {
        let Some(parent_idx) = self.entries.iter().position(|e| e.path == parent_path && e.is_dir) else {
            return;
        };

        // Mark parent as expanded
        self.entries[parent_idx].expanded = true;
        let child_depth = self.entries[parent_idx].depth + 1;

        // Insert children after parent
        let new_entries: Vec<BrowserEntry> = children
            .into_iter()
            .map(|e| listing_to_browser_entry(e, child_depth))
            .collect();

        let insert_pos = parent_idx + 1;
        for (i, entry) in new_entries.into_iter().enumerate() {
            self.entries.insert(insert_pos + i, entry);
        }
    }

    /// Collapse the directory at the cursor, removing its descendants.
    pub fn collapse_at_cursor(&mut self) {
        if self.cursor >= self.entries.len() {
            return;
        }
        let entry = &self.entries[self.cursor];
        if !entry.is_dir || !entry.expanded {
            return;
        }
        let depth = entry.depth;

        // Count descendants to remove
        let mut remove_count = 0;
        for entry in self.entries.iter().skip(self.cursor + 1) {
            if entry.depth > depth {
                remove_count += 1;
            } else {
                break;
            }
        }

        // Remove descendants and mark collapsed
        for _ in 0..remove_count {
            self.entries.remove(self.cursor + 1);
        }
        self.entries[self.cursor].expanded = false;
    }

    /// Get the entry at the cursor (if any).
    pub fn current_entry(&self) -> Option<&BrowserEntry> {
        self.entries.get(self.cursor)
    }

    /// Set cursor to a specific index (clamped).
    pub fn set_cursor(&mut self, idx: usize) {
        if idx < self.entries.len() {
            self.cursor = idx;
            self.ensure_visible();
        }
    }

    // =========================================================================
    // Path navigation
    // =========================================================================

    /// Navigate to a specific path: expand ancestors, set cursor.
    ///
    /// The path must already be loaded in the entries list (ancestors expanded).
    /// If found, moves cursor to it and ensures visible.
    pub fn navigate_to_path(&mut self, path: &str) -> bool {
        if let Some(idx) = self.entries.iter().position(|e| e.path == path) {
            self.cursor = idx;
            self.ensure_visible();
            true
        } else {
            false
        }
    }

    // =========================================================================
    // Path filtering
    // =========================================================================

    /// Set a path filter — only entries whose paths are in the set (plus their
    /// ancestor directories) will be visible. Does NOT modify the entries list;
    /// the caller is responsible for rebuilding entries from a filtered query.
    ///
    /// The filter set is stored for `has_path_filter` / `clear_path_filter`.
    pub fn set_path_filter(&mut self, matching_paths: HashSet<String>) {
        self.path_filter = Some(matching_paths);
    }

    /// Clear the active path filter.
    pub fn clear_path_filter(&mut self) {
        self.path_filter = None;
    }

    /// Whether a path filter is currently active.
    pub fn has_path_filter(&self) -> bool {
        self.path_filter.is_some()
    }

    /// Number of matching files in the current path filter, if any.
    pub fn filtered_file_count(&self) -> Option<usize> {
        self.path_filter.as_ref().map(|f| f.len())
    }

    // =========================================================================
    // Predicate search
    // =========================================================================

    /// Find the next entry (wrapping) matching a predicate, starting after `from`.
    pub fn find_next_matching(
        &self,
        from: usize,
        predicate: impl Fn(&BrowserEntry) -> bool,
    ) -> Option<usize> {
        let len = self.entries.len();
        if len == 0 {
            return None;
        }
        // Search from (from+1) to end, then wrap around from 0 to from
        for offset in 1..=len {
            let idx = (from + offset) % len;
            if predicate(&self.entries[idx]) {
                return Some(idx);
            }
        }
        None
    }

    /// Find the previous entry (wrapping) matching a predicate, starting before `from`.
    pub fn find_prev_matching(
        &self,
        from: usize,
        predicate: impl Fn(&BrowserEntry) -> bool,
    ) -> Option<usize> {
        let len = self.entries.len();
        if len == 0 {
            return None;
        }
        for offset in 1..=len {
            let idx = (from + len - offset) % len;
            if predicate(&self.entries[idx]) {
                return Some(idx);
            }
        }
        None
    }
}

/// Convert a `DirectoryListingEntry` to a `BrowserEntry` at a given depth.
fn listing_to_browser_entry(entry: DirectoryListingEntry, depth: usize) -> BrowserEntry {
    BrowserEntry {
        path: entry.path,
        name: entry.name,
        depth,
        is_dir: entry.is_dir,
        expanded: false,
        file_count: entry.file_count,
        inode: entry.inode,
        duration_ms: entry.duration_ms,
        bitrate_kbps: entry.bitrate_kbps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_dir(name: &str, path: &str, file_count: usize) -> DirectoryListingEntry {
        DirectoryListingEntry {
            name: name.to_string(),
            path: path.to_string(),
            is_dir: true,
            file_count,
            inode: None,
            duration_ms: None,
            bitrate_kbps: None,
        }
    }

    fn make_file(name: &str, path: &str, inode: i64) -> DirectoryListingEntry {
        DirectoryListingEntry {
            name: name.to_string(),
            path: path.to_string(),
            is_dir: false,
            file_count: 0,
            inode: Some(inode),
            duration_ms: Some(180_000),
            bitrate_kbps: Some(320),
        }
    }

    #[test]
    fn populate_root() {
        let mut browser = DirectoryBrowser::new("corpus");
        browser.populate_root(vec![
            make_dir("rock", "rock", 10),
            make_dir("jazz", "jazz", 5),
        ]);
        assert_eq!(browser.entries.len(), 2);
        assert_eq!(browser.entries[0].name, "rock");
        assert_eq!(browser.entries[1].name, "jazz");
        assert!(browser.entries[0].is_dir);
    }

    #[test]
    fn populate_children_and_collapse() {
        let mut browser = DirectoryBrowser::new("corpus");
        browser.populate_root(vec![
            make_dir("rock", "rock", 10),
            make_dir("jazz", "jazz", 5),
        ]);

        // Expand "rock"
        browser.populate_children("rock", vec![
            make_dir("prog", "rock/prog", 3),
            make_file("track.flac", "rock/track.flac", 1),
        ]);

        assert_eq!(browser.entries.len(), 4);
        assert!(browser.entries[0].expanded);
        assert_eq!(browser.entries[1].name, "prog");
        assert_eq!(browser.entries[1].depth, 1);
        assert_eq!(browser.entries[2].name, "track.flac");
        assert_eq!(browser.entries[3].name, "jazz");

        // Collapse "rock"
        browser.cursor = 0;
        browser.collapse_at_cursor();
        assert_eq!(browser.entries.len(), 2);
        assert!(!browser.entries[0].expanded);
    }

    #[test]
    fn handle_input_nav() {
        let mut browser = DirectoryBrowser::new("corpus");
        browser.populate_root(vec![
            make_dir("a", "a", 1),
            make_dir("b", "b", 2),
            make_dir("c", "c", 3),
        ]);

        assert_eq!(browser.cursor, 0);

        // Down
        assert!(browser.handle_input(&InputAction::NavDown).is_none());
        assert_eq!(browser.cursor, 1);

        // Down again
        browser.handle_input(&InputAction::NavDown);
        assert_eq!(browser.cursor, 2);

        // Up
        browser.handle_input(&InputAction::NavUp);
        assert_eq!(browser.cursor, 1);

        // Home
        browser.handle_input(&InputAction::Home);
        assert_eq!(browser.cursor, 0);

        // End
        browser.handle_input(&InputAction::End);
        assert_eq!(browser.cursor, 2);
    }

    #[test]
    fn handle_input_expand_collapse() {
        let mut browser = DirectoryBrowser::new("corpus");
        browser.populate_root(vec![
            make_dir("rock", "rock", 10),
        ]);

        // Right on collapsed dir → RequestExpand
        let action = browser.handle_input(&InputAction::NavRight);
        assert!(matches!(action, Some(BrowserAction::RequestExpand(ref p)) if p == "rock"));

        // Simulate populate
        browser.populate_children("rock", vec![
            make_file("track.flac", "rock/track.flac", 1),
        ]);
        assert!(browser.entries[0].expanded);

        // Left on expanded dir → Collapse
        browser.cursor = 0;
        let action = browser.handle_input(&InputAction::NavLeft);
        assert!(matches!(action, Some(BrowserAction::Collapse)));
    }

    #[test]
    fn handle_input_confirm() {
        let mut browser = DirectoryBrowser::new("corpus");
        browser.populate_root(vec![
            make_dir("rock", "rock", 10),
        ]);
        browser.populate_children("rock", vec![
            make_file("track.flac", "rock/track.flac", 42),
        ]);

        // Confirm on directory
        browser.cursor = 0;
        let action = browser.handle_input(&InputAction::Confirm);
        assert!(matches!(action, Some(BrowserAction::SelectDirectory(ref p)) if p == "rock"));

        // Confirm on file
        browser.cursor = 1;
        let action = browser.handle_input(&InputAction::Confirm);
        assert!(matches!(action, Some(BrowserAction::SelectFile(42))));
    }
}
