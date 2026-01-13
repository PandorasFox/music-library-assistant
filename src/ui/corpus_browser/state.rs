//! Corpus Browser State Management

use std::fs;
use std::path::{Path, PathBuf};

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;

use crate::config::AUDIO_EXTENSIONS;

use super::types::{BrowserEntry, CorpusBrowserConfig, FileMetadata};

/// State for the corpus browser.
#[derive(Debug)]
pub struct CorpusBrowserState {
    /// Configuration
    config: CorpusBrowserConfig,
    /// Flattened tree of entries
    entries: Vec<BrowserEntry>,
    /// Current cursor position
    cursor_idx: usize,
    /// Root path of the corpus
    root_path: PathBuf,
    /// Scroll offset for view
    scroll_offset: usize,
    /// Visible height (set during render)
    visible_height: usize,
    /// Cached metadata for currently focused file
    cached_metadata: Option<FileMetadata>,
    /// Path of cached metadata
    cached_path: Option<PathBuf>,
}

impl CorpusBrowserState {
    /// Create a new corpus browser at the given root path.
    pub fn new(root_path: PathBuf, config: CorpusBrowserConfig) -> Self {
        let mut state = Self {
            config,
            entries: Vec::new(),
            cursor_idx: 0,
            root_path: root_path.clone(),
            scroll_offset: 0,
            visible_height: 20,
            cached_metadata: None,
            cached_path: None,
        };

        // Initialize with root expanded
        state.load_root();
        state
    }

    /// Load the root directory entries.
    fn load_root(&mut self) {
        self.entries.clear();

        // Add root directory as first entry
        let root_name = self.root_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.root_path.to_string_lossy().to_string());

        let has_children = self.path_has_children(&self.root_path);
        let item_count = self.count_audio_files(&self.root_path);

        self.entries.push(BrowserEntry {
            path: self.root_path.clone(),
            name: root_name,
            depth: 0,
            is_directory: true,
            is_expanded: true, // Start expanded
            has_children,
            item_count,
        });

        // Load children of root
        self.load_children(0);
    }

    /// Check if a path has children (subdirs or audio files).
    fn path_has_children(&self, path: &Path) -> bool {
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    return true;
                }
                if self.is_audio_file(&path) {
                    return true;
                }
            }
        }
        false
    }

    /// Count audio files in a directory (non-recursive).
    fn count_audio_files(&self, path: &Path) -> usize {
        fs::read_dir(path)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| self.is_audio_file(&e.path()))
                    .count()
            })
            .unwrap_or(0)
    }

    /// Check if a path is an audio file.
    fn is_audio_file(&self, path: &Path) -> bool {
        path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| AUDIO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
                .unwrap_or(false)
    }

    /// Load children of the entry at the given index.
    fn load_children(&mut self, parent_idx: usize) {
        let parent = &self.entries[parent_idx];
        if !parent.is_directory || !parent.is_expanded {
            return;
        }

        let parent_path = parent.path.clone();
        let child_depth = parent.depth + 1;

        // Collect children
        let mut dirs: Vec<BrowserEntry> = Vec::new();
        let mut files: Vec<BrowserEntry> = Vec::new();

        if let Ok(read_dir) = fs::read_dir(&parent_path) {
            let mut entries: Vec<_> = read_dir.flatten().collect();
            entries.sort_by_key(|e| e.path());

            for entry in entries {
                let path = entry.path();
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // Skip hidden files/directories
                if name.starts_with('.') {
                    continue;
                }

                if path.is_dir() {
                    let has_children = self.path_has_children(&path);
                    let item_count = self.count_audio_files(&path);
                    dirs.push(BrowserEntry {
                        path,
                        name,
                        depth: child_depth,
                        is_directory: true,
                        is_expanded: false,
                        has_children,
                        item_count,
                    });
                } else if self.config.show_files && self.is_audio_file(&path) {
                    files.push(BrowserEntry {
                        path,
                        name,
                        depth: child_depth,
                        is_directory: false,
                        is_expanded: false,
                        has_children: false,
                        item_count: 0,
                    });
                }
            }
        }

        // Insert children after parent (directories first, then files)
        let insert_pos = parent_idx + 1;
        let children: Vec<_> = dirs.into_iter().chain(files).collect();
        for (i, child) in children.into_iter().enumerate() {
            self.entries.insert(insert_pos + i, child);
        }
    }

    /// Remove children of the entry at the given index.
    fn remove_children(&mut self, parent_idx: usize) {
        let parent_depth = self.entries[parent_idx].depth;
        let mut remove_count = 0;

        // Count entries that are children (higher depth, until we hit same/lower depth)
        for entry in self.entries.iter().skip(parent_idx + 1) {
            if entry.depth > parent_depth {
                remove_count += 1;
            } else {
                break;
            }
        }

        // Remove children
        for _ in 0..remove_count {
            self.entries.remove(parent_idx + 1);
        }
    }

    // =========================================================================
    // Navigation
    // =========================================================================

    /// Move cursor up.
    pub fn move_up(&mut self) {
        if self.cursor_idx > 0 {
            self.cursor_idx -= 1;
            self.ensure_visible();
            self.update_cached_metadata();
        }
    }

    /// Move cursor down.
    pub fn move_down(&mut self) {
        if self.cursor_idx + 1 < self.entries.len() {
            self.cursor_idx += 1;
            self.ensure_visible();
            self.update_cached_metadata();
        }
    }

    /// Expand current directory.
    pub fn expand_current(&mut self) {
        if let Some(entry) = self.entries.get_mut(self.cursor_idx) {
            if entry.is_directory && entry.has_children && !entry.is_expanded {
                entry.is_expanded = true;
                self.load_children(self.cursor_idx);
            }
        }
    }

    /// Collapse current directory or move to parent.
    pub fn collapse_or_parent(&mut self) {
        if let Some(entry) = self.entries.get(self.cursor_idx) {
            if entry.is_directory && entry.is_expanded {
                // Collapse
                self.entries[self.cursor_idx].is_expanded = false;
                self.remove_children(self.cursor_idx);
            } else {
                // Move to parent
                let current_depth = entry.depth;
                if current_depth > 0 {
                    for i in (0..self.cursor_idx).rev() {
                        if self.entries[i].depth < current_depth && self.entries[i].is_directory {
                            self.cursor_idx = i;
                            self.ensure_visible();
                            self.update_cached_metadata();
                            break;
                        }
                    }
                }
            }
        }
    }

    /// Ensure cursor is visible in viewport.
    fn ensure_visible(&mut self) {
        if self.cursor_idx < self.scroll_offset {
            self.scroll_offset = self.cursor_idx;
        } else if self.cursor_idx >= self.scroll_offset + self.visible_height {
            self.scroll_offset = self.cursor_idx - self.visible_height + 1;
        }
    }

    // =========================================================================
    // Metadata
    // =========================================================================

    /// Update cached metadata for current entry.
    fn update_cached_metadata(&mut self) {
        let current_path = self.entries.get(self.cursor_idx).map(|e| e.path.clone());

        // Only update if path changed
        if current_path != self.cached_path {
            self.cached_path = current_path.clone();
            self.cached_metadata = current_path.and_then(|p| {
                if p.is_file() {
                    Some(self.load_file_metadata(&p))
                } else {
                    None
                }
            });
        }
    }

    /// Load metadata for a file.
    fn load_file_metadata(&self, path: &Path) -> FileMetadata {
        let file_size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let file_type = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();

        // Try to load tags using lofty
        let (bitrate_kbps, duration_ms, sample_rate, tags) =
            if let Ok(tagged_file) = lofty::read_from_path(path) {
                let props = tagged_file.properties();
                let bitrate = props.audio_bitrate();
                let duration = props.duration().as_millis() as u64;
                let sample = props.sample_rate();

                // Extract tags
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

    /// Get current entry.
    pub fn current_entry(&self) -> Option<&BrowserEntry> {
        self.entries.get(self.cursor_idx)
    }

    /// Get current entry's path.
    pub fn current_path(&self) -> Option<&PathBuf> {
        self.current_entry().map(|e| &e.path)
    }

    /// Get entries for rendering.
    pub fn entries(&self) -> &[BrowserEntry] {
        &self.entries
    }

    /// Get cursor index.
    pub fn cursor_idx(&self) -> usize {
        self.cursor_idx
    }

    /// Get scroll offset.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// Set visible height.
    pub fn set_visible_height(&mut self, height: usize) {
        self.visible_height = height;
    }

    /// Get cached metadata.
    pub fn cached_metadata(&self) -> Option<&FileMetadata> {
        self.cached_metadata.as_ref()
    }

    /// Get config.
    pub fn config(&self) -> &CorpusBrowserConfig {
        &self.config
    }
}
