//! Types for OOB tag bucketed resolution flow.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::corpus::db::types::{BucketedOobFile, ConflictBucket, TagMismatchEntry};
use crate::ui::bulk_selection::BulkSelectionState;
use crate::ui::filter_popup::FilterCondition;
use crate::ui::widgets::{ButtonRects, FocusPane};

// ============================================================================
// Resolution Button
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionButton {
    /// Make disk match DB (write db_value to files)
    ApplyDb,
    /// Make DB match disk (assimilate disk_value into index)
    AssimilateDisk,
}

impl ResolutionButton {
    pub fn toggle(self) -> Self {
        match self {
            Self::ApplyDb => Self::AssimilateDisk,
            Self::AssimilateDisk => Self::ApplyDb,
        }
    }
}

// ============================================================================
// Per-Bucket File State
// ============================================================================

pub struct BucketFileState {
    pub files: Vec<BucketedOobFile>,
    pub cursor: usize,
    pub scroll: usize,
    /// Per-bucket selection state
    pub selection: BulkSelectionState,
    /// Per-bucket filter condition
    pub filter: Option<FilterCondition>,
    /// Per-bucket filtered indices (cached)
    pub filtered_indices: Option<Vec<usize>>,
}

impl BucketFileState {
    pub fn new(files: Vec<BucketedOobFile>) -> Self {
        Self {
            files,
            cursor: 0,
            scroll: 0,
            selection: BulkSelectionState::new(),
            filter: None,
            filtered_indices: None,
        }
    }

    pub fn current_file(&self) -> Option<&BucketedOobFile> {
        self.files.get(self.cursor)
    }

    /// Get indices of files that match the current filter.
    pub fn get_filtered_indices(&self) -> Vec<usize> {
        if let Some(ref indices) = self.filtered_indices {
            indices.clone()
        } else {
            (0..self.files.len()).collect()
        }
    }

    /// Apply a filter condition and compute filtered indices.
    pub fn apply_filter(&mut self, condition: FilterCondition) {
        if condition.is_active() {
            // For now, accept all (bucket files don't have full metadata)
            // TODO: Add full metadata filtering when track info is available
            let indices: Vec<usize> = (0..self.files.len()).collect();
            self.filtered_indices = Some(indices);
            self.filter = Some(condition);
        } else {
            self.filter = None;
            self.filtered_indices = None;
        }
    }

    /// Clear the current filter.
    pub fn clear_filter(&mut self) {
        self.filter = None;
        self.filtered_indices = None;
    }

    fn navigate_up(&mut self) -> bool {
        if self.cursor > 0 {
            self.cursor -= 1;
            true
        } else {
            false
        }
    }

    fn navigate_down(&mut self) -> bool {
        if self.cursor + 1 < self.files.len() {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn page_up(&mut self) -> bool {
        let old = self.cursor;
        self.cursor = self.cursor.saturating_sub(20);
        self.cursor != old
    }

    fn page_down(&mut self) -> bool {
        let old = self.cursor;
        self.cursor = (self.cursor + 20).min(self.files.len().saturating_sub(1));
        self.cursor != old
    }
}

// ============================================================================
// Action Enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OobConflictAction {
    None,
    /// File or bucket selection changed — action handler should reload diff
    Navigate,
    /// Resolve the active bucket (bulk, buckets 1/2 only - tag sync/conflict)
    Resolve,
    /// Acknowledge mtime-only changes (bucket 0 only)
    Acknowledge,
    /// Close inspector and return to Insights
    Cancel,
    /// Open filter popup (Ctrl+F)
    OpenFilter,
}

// ============================================================================
// State
// ============================================================================

pub struct OobConflictState {
    /// Active bucket tab
    pub active_bucket: ConflictBucket,
    /// Per-bucket file lists (indexed by ConflictBucket::index())
    /// Each bucket has its own selection and filter state.
    pub buckets: [BucketFileState; 4],
    /// Counts per bucket (for tab labels)
    pub bucket_counts: [usize; 4],
    /// Resolution button selection (for resolvable buckets)
    pub selected_button: ResolutionButton,
    /// On-demand tag diff for the currently selected file
    pub current_diff: Vec<TagMismatchEntry>,
    /// Current focus pane (List or Buttons)
    pub focus_pane: FocusPane,
    /// Button rectangles for click detection (set during render)
    pub button_rects: ButtonRects,
}

impl OobConflictState {
    pub fn new(files: Vec<BucketedOobFile>) -> Self {
        // Partition files into buckets
        let mut b0 = Vec::new();
        let mut b1 = Vec::new();
        let mut b2 = Vec::new();
        let mut b3 = Vec::new();

        for file in files {
            match file.bucket {
                ConflictBucket::MtimeOnly => b0.push(file),
                ConflictBucket::DbOnly => b1.push(file),
                ConflictBucket::DiskOnly => b2.push(file),
                ConflictBucket::Conflict => b3.push(file),
            }
        }

        let bucket_counts = [b0.len(), b1.len(), b2.len(), b3.len()];

        // Start on first non-empty bucket
        let initial_bucket = ConflictBucket::ALL
            .iter()
            .find(|b| bucket_counts[b.index()] > 0)
            .copied()
            .unwrap_or(ConflictBucket::MtimeOnly);

        Self {
            active_bucket: initial_bucket,
            buckets: [
                BucketFileState::new(b0),
                BucketFileState::new(b1),
                BucketFileState::new(b2),
                BucketFileState::new(b3),
            ],
            bucket_counts,
            selected_button: ResolutionButton::ApplyDb,
            current_diff: Vec::new(),
            focus_pane: FocusPane::List,
            button_rects: ButtonRects::new(),
        }
    }

    pub fn active_bucket_state(&self) -> &BucketFileState {
        &self.buckets[self.active_bucket.index()]
    }

    fn active_bucket_state_mut(&mut self) -> &mut BucketFileState {
        &mut self.buckets[self.active_bucket.index()]
    }

    pub fn total_files(&self) -> usize {
        self.bucket_counts.iter().sum()
    }

    /// Apply a filter condition to the active bucket.
    pub fn apply_filter(&mut self, condition: FilterCondition) {
        self.active_bucket_state_mut().apply_filter(condition);
    }

    /// Clear the filter from the active bucket.
    pub fn clear_filter(&mut self) {
        self.active_bucket_state_mut().clear_filter();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> OobConflictAction {
        // Shift+Up / Shift+Down: cycle focus pane
        if key.modifiers.contains(KeyModifiers::SHIFT) {
            match key.code {
                KeyCode::Up => {
                    self.focus_pane = self.focus_pane.prev();
                    return OobConflictAction::None;
                }
                KeyCode::Down => {
                    self.focus_pane = self.focus_pane.next();
                    return OobConflictAction::None;
                }
                _ => {}
            }
        }

        // Ctrl+A: toggle all selection in active bucket (respects filter)
        if key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::CONTROL) {
            let bucket = self.active_bucket_state_mut();
            let indices = bucket.get_filtered_indices();
            bucket.selection.toggle_all_filtered(&indices);
            return OobConflictAction::None;
        }

        // Ctrl+F: open filter popup
        if key.code == KeyCode::Char('f') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return OobConflictAction::OpenFilter;
        }

        match key.code {
            // Space: toggle selection on current file in active bucket
            KeyCode::Char(' ') => {
                let bucket = self.active_bucket_state_mut();
                if !bucket.files.is_empty() {
                    let cursor = bucket.cursor;
                    bucket.selection.toggle(cursor);
                }
                OobConflictAction::None
            }

            // Bucket tab navigation
            KeyCode::Tab => {
                self.active_bucket = self.active_bucket.next();
                OobConflictAction::Navigate
            }
            KeyCode::BackTab => {
                self.active_bucket = self.active_bucket.prev();
                OobConflictAction::Navigate
            }

            // File navigation within active bucket
            KeyCode::Up | KeyCode::Char('k') => {
                if self.active_bucket_state_mut().navigate_up() {
                    OobConflictAction::Navigate
                } else {
                    OobConflictAction::None
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.active_bucket_state_mut().navigate_down() {
                    OobConflictAction::Navigate
                } else {
                    OobConflictAction::None
                }
            }
            KeyCode::PageUp => {
                if self.active_bucket_state_mut().page_up() {
                    OobConflictAction::Navigate
                } else {
                    OobConflictAction::None
                }
            }
            KeyCode::PageDown => {
                if self.active_bucket_state_mut().page_down() {
                    OobConflictAction::Navigate
                } else {
                    OobConflictAction::None
                }
            }

            // Resolution button toggle (resolvable buckets only, when focused on buttons)
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l') => {
                if self.focus_pane == FocusPane::Buttons && self.active_bucket.is_resolvable() {
                    self.selected_button = self.selected_button.toggle();
                }
                OobConflictAction::None
            }

            // Confirm resolution (when focused on buttons)
            KeyCode::Enter => {
                if self.focus_pane == FocusPane::Buttons
                    && !self.active_bucket_state().files.is_empty()
                {
                    if self.active_bucket.is_acknowledgeable() {
                        OobConflictAction::Acknowledge
                    } else if self.active_bucket.is_resolvable() {
                        OobConflictAction::Resolve
                    } else {
                        OobConflictAction::None
                    }
                } else {
                    OobConflictAction::None
                }
            }

            KeyCode::Esc => OobConflictAction::Cancel,

            _ => OobConflictAction::None,
        }
    }
}

// ============================================================================
// On-demand diff computation
// ============================================================================

/// Compute the tag diff between DB and disk for a single track.
///
/// Reads DB tags from the database and disk tags from the filesystem,
/// then returns entries for all fields where the values differ.
/// Properly handles multi-value tags by comparing value sets per key.
pub fn compute_tag_diff(
    read_db: &crate::corpus::db::ReadOnlyDb<'_>,
    track_id: i64,
    abs_path: &std::path::Path,
) -> Vec<TagMismatchEntry> {
    use crate::corpus::tags::TagSet;
    use std::collections::HashSet;

    // Get DB tags as TagSet
    let db_tags = match read_db.get_track_tags(track_id) {
        Ok(tags) => tags,
        Err(_) => return Vec::new(),
    };
    let db_tagset = TagSet::new(
        db_tags.into_iter().map(|t| (t.tag_name, t.tag_value))
    );

    // Read disk tags as TagSet
    let disk_tagset = match TagSet::from_file(abs_path) {
        Ok(tags) => tags,
        Err(_) => return Vec::new(),
    };

    // Get all unique tag names from both sources
    let mut all_tag_names: HashSet<String> = HashSet::new();
    for (k, _) in db_tagset.iter() {
        all_tag_names.insert(k.to_string());
    }
    for (k, _) in disk_tagset.iter() {
        all_tag_names.insert(k.to_string());
    }

    let mut mismatches = Vec::new();
    for tag_name in all_tag_names {
        // Collect all values for this tag from each source
        let db_values: Vec<&str> = db_tagset.values_for(&tag_name).collect();
        let disk_values: Vec<&str> = disk_tagset.values_for(&tag_name).collect();

        // Convert to sets for proper comparison (order doesn't matter)
        let db_set: HashSet<&str> = db_values.iter().copied().collect();
        let disk_set: HashSet<&str> = disk_values.iter().copied().collect();

        if db_set != disk_set {
            // Aggregate multi-values into semicolon-separated string for display
            let db_display = if db_values.is_empty() {
                None
            } else {
                Some(db_values.join("; "))
            };
            let disk_display = if disk_values.is_empty() {
                None
            } else {
                Some(disk_values.join("; "))
            };

            mismatches.push(TagMismatchEntry {
                field: tag_name,
                db_value: db_display,
                disk_value: disk_display,
                // Store individual values for proper mutation generation
                _db_values: db_values.iter().map(|s| s.to_string()).collect(),
                _disk_values: disk_values.iter().map(|s| s.to_string()).collect(),
            });
        }
    }

    mismatches.sort_by(|a, b| a.field.cmp(&b.field));
    mismatches
}
