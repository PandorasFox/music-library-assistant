//! Insights View Module
//!
//! A full-screen view displaying computed insights over health signals.
//! Part of the lateral view ring - can cycle to adjacent views with Tab/Shift-Tab.
//!
//! ## Four-Bucket Structure
//!
//! Insights are organized into four buckets with distinct purposes:
//! 1. **Corpus Files** - OOB changes (top priority), indexed/unindexed/missing counts
//! 2. **Placeholder** - Reserved for future use (displays `:)`)
//! 3. **Library/Deploy** - Stale, leftover, ready-to-deploy, deployed healthy
//! 4. **Other Signals** - Remaining signals sorted by count
//!
//! ## Navigation
//!
//! - Up/Down: Navigate within and between buckets
//! - Enter: Launch flow for selected insight (blocked when Witch is busy)
//! - Tab/Shift-Tab: Cycle to adjacent view
//! - Esc: Return to main menu
//!
//! ## Modal State
//!
//! The view tracks whether the Witch is busy. When busy, actionable
//! insights are dimmed and the Enter key is blocked.

mod render;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::corpus::db::types::{InsightsData, CorpusFilesBucket, LibraryDeployBucket, TagSquashBucket, OtherSignalsBucket};
use crate::ui::widgets::ListClickTargets;
use crate::witch::DaemonStatus;

pub use render::render_insights_view;

/// Action returned from input handling
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsightsAction {
    /// No action needed
    None,
    /// Request to quit the application (show confirmation)
    RequestQuit,
    /// Cycle to next view in ring
    CycleNext,
    /// Cycle to previous view in ring
    CyclePrev,
    /// Launch flow for selected insight
    LaunchFlow,
}

/// State for the insights view modal/status
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum InsightsModal {
    /// Ready for user interaction
    Ready,
    /// The Witch has operations in-flight - actions blocked
    NotReady_WitchBusy,
}

impl Default for InsightsModal {
    fn default() -> Self {
        Self::Ready
    }
}

/// Which bucket currently has focus for navigation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusedBucket {
    #[default]
    Corpus,
    Placeholder,
    Library,
    Other,
}

impl FocusedBucket {
    /// Get the index of this bucket (0-3)
    pub fn index(self) -> usize {
        match self {
            FocusedBucket::Corpus => 0,
            FocusedBucket::Placeholder => 1,
            FocusedBucket::Library => 2,
            FocusedBucket::Other => 3,
        }
    }

    /// Get the next bucket in order
    fn next(self) -> Self {
        match self {
            FocusedBucket::Corpus => FocusedBucket::Placeholder,
            FocusedBucket::Placeholder => FocusedBucket::Library,
            FocusedBucket::Library => FocusedBucket::Other,
            FocusedBucket::Other => FocusedBucket::Other, // Stay at end
        }
    }

    /// Get the previous bucket in order
    fn prev(self) -> Self {
        match self {
            FocusedBucket::Corpus => FocusedBucket::Corpus, // Stay at start
            FocusedBucket::Placeholder => FocusedBucket::Corpus,
            FocusedBucket::Library => FocusedBucket::Placeholder,
            FocusedBucket::Other => FocusedBucket::Library,
        }
    }
}

/// Selection state within a single bucket
#[derive(Debug, Clone, Default)]
pub struct BucketSelection {
    /// Index of selected item within this bucket
    pub selected: usize,
    /// Scroll offset for rendering
    pub scroll: usize,
}

// ============================================================================
// Unified Bucket Entry System
// ============================================================================

/// Unique identifier for each insight type across all buckets.
/// Enables type-safe action dispatch and detail rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsightType {
    // Corpus bucket entries
    CorpusMtimeOnly,
    CorpusOobTagSync,
    CorpusOobTagConflict,
    CorpusInodeChanged,
    CorpusFilesInCorpus,
    CorpusFilesIndexed,
    CorpusFilesUnindexed,
    CorpusFilesMissing,
    CorpusDirectoriesMissing,
    CorpusFilesRelocated,
    CorpusCorruptFiles,
    CorpusShitFormatFiles,
    // Tag resolution bucket entries (duplicates at top for easy resolution)
    CrossSourceOverlaps,
    SubparDuplicates,
    InconsistentAlbumArtist,
    TagCanonicity { tag_name: String },
    CompoundTagValueSafe,   // All split parts exist in corpus
    CompoundTagValueReview, // Some/all parts are new to corpus
    // Library bucket entries
    LibraryStale,
    LibraryLeftover,
    LibraryDeployReady,
    LibraryDeployedHealthy,
    // Other bucket - dynamic entries identified by index
    OtherSignal { index: usize },
}

/// Actions that can be launched from specific insight types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightAction {
    /// Launch deployment preview flow
    LaunchDeploymentPreview,
    /// Launch missing file resolution flow
    LaunchMissingFileResolution,
    /// Launch missing directory acknowledgment flow
    LaunchMissingDirectoryResolution,
    /// Launch tag canonicity resolution flow
    LaunchTagCanonicityResolution,
    /// Launch compound tag split flow (safe - all parts exist)
    LaunchCompoundTagSplitSafe,
    /// Launch compound tag split flow (review - some parts new)
    LaunchCompoundTagSplitReview,
    /// Launch OOB tag sync resolution flow
    LaunchOobTagSync,
    /// Launch OOB tag conflict inspection
    LaunchOobTagConflict,
    /// Launch inode changed acknowledgement flow
    LaunchInodeChangedAcknowledge,
    /// Launch moved file acknowledgement flow
    LaunchMovedFileAcknowledge,
    /// Launch corrupt file resolution flow (stash + drop)
    LaunchCorruptFileResolution,
    /// Launch shit format transcode flow
    LaunchShitFormatTranscode,
    /// Launch intake confirmation flow for unindexed files
    LaunchIntakeConfirmation,
    /// Launch fingerprint duplicate resolution flow
    LaunchDirectoryOverlapResolution,
    /// Launch subpar duplicate stash flow
    LaunchSubparDuplicateResolution,
    /// Flow not yet implemented
    NotImplemented,
    /// Informational only - no action available
    Informational,
}

/// A single rendered entry in an insights bucket.
/// Contains all information needed for rendering, selection, and action dispatch.
#[derive(Debug, Clone)]
pub struct BucketEntry {
    /// Unique type identifier
    pub insight_type: InsightType,
    /// Display label
    pub label: String,
    /// Count value (None for placeholder)
    pub count: Option<usize>,
    /// Pre-computed color based on count and entry type
    pub color: Color,
    /// Sorting rank within bucket (lower = higher priority)
    pub rank: u8,
    /// Action available for this entry
    pub action: InsightAction,
}

impl BucketEntry {
    /// Create a corpus entry
    fn corpus(
        insight_type: InsightType,
        label: &str,
        count: usize,
        rank: u8,
        color: Color,
        action: InsightAction,
    ) -> Self {
        Self {
            insight_type,
            label: label.to_string(),
            count: Some(count),
            color,
            rank,
            action,
        }
    }

    /// Create a library entry
    fn library(insight_type: InsightType, label: &str, count: usize, color: Color) -> Self {
        Self {
            insight_type,
            label: label.to_string(),
            count: Some(count),
            color,
            rank: 0, // Library uses count for sorting, not rank
            action: InsightAction::LaunchDeploymentPreview,
        }
    }

    /// Create cross-source overlaps entry
    fn cross_source_overlaps(count: usize) -> Self {
        Self {
            insight_type: InsightType::CrossSourceOverlaps,
            label: "Cross-source overlaps".to_string(),
            count: Some(count),
            color: if count > 0 { Color::Cyan } else { Color::Green },
            rank: 0,
            action: InsightAction::LaunchDirectoryOverlapResolution,
        }
    }

    /// Create subpar duplicates entry
    fn subpar_duplicates(count: usize) -> Self {
        Self {
            insight_type: InsightType::SubparDuplicates,
            label: "Subpar duplicates".to_string(),
            count: Some(count),
            color: if count > 0 { Color::Cyan } else { Color::Green },
            rank: 0,
            action: InsightAction::LaunchSubparDuplicateResolution,
        }
    }

    /// Create inconsistent album_artist entry
    fn inconsistent_album_artist(count: usize) -> Self {
        Self {
            insight_type: InsightType::InconsistentAlbumArtist,
            label: "Inconsistent album_artist".to_string(),
            count: Some(count),
            color: if count > 0 { Color::Yellow } else { Color::Green },
            rank: 0,
            action: InsightAction::LaunchTagCanonicityResolution,
        }
    }

    /// Create tag canonicity entry (for a specific tag name)
    fn tag_canonicity(tag_name: &str, cluster_count: usize) -> Self {
        Self {
            insight_type: InsightType::TagCanonicity { tag_name: tag_name.to_string() },
            label: format!("{} canonicity", tag_name),
            count: Some(cluster_count),
            color: if cluster_count > 0 { Color::Yellow } else { Color::Green },
            rank: 0,
            action: InsightAction::LaunchTagCanonicityResolution,
        }
    }

    /// Create compound tag value safe entry (all parts exist in corpus)
    fn compound_tag_value_safe(count: usize) -> Self {
        Self {
            insight_type: InsightType::CompoundTagValueSafe,
            label: "Compound splits (safe)".to_string(),
            count: Some(count),
            color: if count > 0 { Color::Green } else { Color::DarkGray },
            rank: 0,
            action: InsightAction::LaunchCompoundTagSplitSafe,
        }
    }

    /// Create compound tag value review entry (some parts are new)
    fn compound_tag_value_review(count: usize) -> Self {
        Self {
            insight_type: InsightType::CompoundTagValueReview,
            label: "Compound splits (review)".to_string(),
            count: Some(count),
            color: if count > 0 { Color::Yellow } else { Color::DarkGray },
            rank: 0,
            action: InsightAction::LaunchCompoundTagSplitReview,
        }
    }

    /// Create "other signal" entry
    fn other(index: usize, label: &str, count: usize, signal_type: &str) -> Self {
        // Determine action based on signal type
        let action = match signal_type {
            "TagCanonicity" | "InconsistentAlbumArtist" => InsightAction::LaunchTagCanonicityResolution,
            "CompoundTagValue" => InsightAction::LaunchCompoundTagSplitReview, // Default to review
            _ => InsightAction::NotImplemented,
        };

        Self {
            insight_type: InsightType::OtherSignal { index },
            label: label.to_string(),
            count: Some(count),
            color: if count > 0 { Color::Yellow } else { Color::Green },
            rank: 0, // Pre-sorted from database
            action,
        }
    }
}

/// Pre-sorted entries for all buckets.
/// Built once when InsightsData changes, used everywhere.
#[derive(Debug, Clone, Default)]
pub struct CachedBucketEntries {
    pub corpus: Vec<BucketEntry>,
    pub placeholder: Vec<BucketEntry>,
    pub library: Vec<BucketEntry>,
    pub other: Vec<BucketEntry>,
}

impl CachedBucketEntries {
    /// Build from InsightsData, applying all sorting logic once
    pub fn from_insights_data(data: &InsightsData) -> Self {
        Self {
            corpus: Self::build_corpus_entries(&data.bucket_corpus),
            placeholder: Self::build_placeholder_entries(&data.bucket_placeholder),
            library: Self::build_library_entries(&data.bucket_library),
            other: Self::build_other_entries(&data.bucket_other),
        }
    }

    fn build_corpus_entries(corpus: &CorpusFilesBucket) -> Vec<BucketEntry> {
        let mut entries = vec![
            BucketEntry::corpus(
                InsightType::CorpusMtimeOnly,
                "Mtime changes (ack needed)",
                corpus.mtime_only_mismatch,
                if corpus.mtime_only_mismatch > 0 { 0 } else { 2 },
                if corpus.mtime_only_mismatch > 0 { Color::Yellow } else { Color::DarkGray },
                InsightAction::LaunchOobTagConflict, // Same flow as conflict, handles MtimeOnly bucket
            ),
            BucketEntry::corpus(
                InsightType::CorpusOobTagSync,
                "Tags syncable (out-of-band)",
                corpus.oob_tag_sync,
                if corpus.oob_tag_sync > 0 { 0 } else { 2 },
                if corpus.oob_tag_sync > 0 { Color::Yellow } else { Color::DarkGray },
                InsightAction::LaunchOobTagSync,
            ),
            BucketEntry::corpus(
                InsightType::CorpusOobTagConflict,
                "Tag conflicts (out-of-band)",
                corpus.oob_tag_conflict,
                if corpus.oob_tag_conflict > 0 { 0 } else { 2 },
                if corpus.oob_tag_conflict > 0 { Color::Red } else { Color::DarkGray },
                InsightAction::LaunchOobTagConflict,
            ),
            BucketEntry::corpus(
                InsightType::CorpusInodeChanged,
                "Files replaced (inode changed)",
                corpus.inode_changed,
                if corpus.inode_changed > 0 { 0 } else { 2 },
                if corpus.inode_changed > 0 { Color::Red } else { Color::DarkGray },
                InsightAction::LaunchInodeChangedAcknowledge,
            ),
            BucketEntry::corpus(
                InsightType::CorpusFilesInCorpus,
                "Files in corpus",
                corpus.files_in_corpus,
                1,
                Color::Yellow,
                InsightAction::Informational,
            ),
            BucketEntry::corpus(
                InsightType::CorpusFilesIndexed,
                "Files indexed",
                corpus.files_indexed,
                1,
                Color::Green,
                InsightAction::Informational,
            ),
            BucketEntry::corpus(
                InsightType::CorpusFilesUnindexed,
                "Files unindexed",
                corpus.files_unindexed,
                1,
                if corpus.files_unindexed > 0 { Color::Yellow } else { Color::Green },
                InsightAction::LaunchIntakeConfirmation,
            ),
            BucketEntry::corpus(
                InsightType::CorpusFilesMissing,
                "Files missing",
                corpus.files_missing,
                1,
                if corpus.files_missing > 0 { Color::Red } else { Color::Green },
                InsightAction::LaunchMissingFileResolution,
            ),
            BucketEntry::corpus(
                InsightType::CorpusDirectoriesMissing,
                "Directories missing",
                corpus.directories_missing,
                if corpus.directories_missing > 0 { 0 } else { 2 },
                if corpus.directories_missing > 0 { Color::Red } else { Color::DarkGray },
                InsightAction::LaunchMissingDirectoryResolution,
            ),
            BucketEntry::corpus(
                InsightType::CorpusFilesRelocated,
                "Files relocated (moved)",
                corpus.files_relocated,
                if corpus.files_relocated > 0 { 0 } else { 2 },
                if corpus.files_relocated > 0 { Color::Yellow } else { Color::DarkGray },
                InsightAction::LaunchMovedFileAcknowledge,
            ),
            BucketEntry::corpus(
                InsightType::CorpusCorruptFiles,
                "Corrupt files",
                corpus.corrupt_files,
                if corpus.corrupt_files > 0 { 0 } else { 2 },
                if corpus.corrupt_files > 0 { Color::Red } else { Color::DarkGray },
                InsightAction::LaunchCorruptFileResolution,
            ),
            BucketEntry::corpus(
                InsightType::CorpusShitFormatFiles,
                "Shit format files",
                corpus.shit_format_files,
                if corpus.shit_format_files > 0 { 0 } else { 2 },
                if corpus.shit_format_files > 0 { Color::Yellow } else { Color::DarkGray },
                InsightAction::LaunchShitFormatTranscode,
            ),
        ];

        // Sort by rank (0=top, 1=middle, 2=bottom), preserving relative order
        entries.sort_by_key(|e| e.rank);
        entries
    }

    fn build_library_entries(library: &LibraryDeployBucket) -> Vec<BucketEntry> {
        let mut entries = vec![
            BucketEntry::library(
                InsightType::LibraryStale,
                "Library stale",
                library.library_stale,
                if library.library_stale > 0 { Color::Yellow } else { Color::Green },
            ),
            BucketEntry::library(
                InsightType::LibraryLeftover,
                "Library leftover",
                library.library_leftover,
                if library.library_leftover > 0 { Color::Yellow } else { Color::Green },
            ),
            BucketEntry::library(
                InsightType::LibraryDeployReady,
                "Ready to deploy",
                library.deploy_ready,
                if library.deploy_ready > 0 { Color::Cyan } else { Color::Green },
            ),
            BucketEntry::library(
                InsightType::LibraryDeployedHealthy,
                "Deployed healthy",
                library.deployed_healthy,
                Color::Green,
            ),
        ];

        // Sort by count descending
        entries.sort_by(|a, b| b.count.unwrap_or(0).cmp(&a.count.unwrap_or(0)));
        entries
    }

    fn build_placeholder_entries(bucket: &TagSquashBucket) -> Vec<BucketEntry> {
        let mut entries = Vec::new();

        // Cross-source overlaps at top - easy resolutions
        if bucket.directory_overlap_cluster_count > 0 {
            entries.push(BucketEntry::cross_source_overlaps(bucket.directory_overlap_cluster_count));
        }

        // Subpar duplicates - identified low-quality copies ready to stash
        if bucket.subpar_duplicate_count > 0 {
            entries.push(BucketEntry::subpar_duplicates(bucket.subpar_duplicate_count));
        }

        // Add inconsistent album_artist if present
        if bucket.inconsistent_album_artist_count > 0 {
            entries.push(BucketEntry::inconsistent_album_artist(bucket.inconsistent_album_artist_count));
        }

        // Add tag canonicity entries for each tag type
        for entry in &bucket.tag_canonicity {
            entries.push(BucketEntry::tag_canonicity(&entry.tag_name, entry.cluster_count));
        }

        // Add compound tag values - safe first (easy bulk action), then review
        if bucket.compound_safe_count > 0 {
            entries.push(BucketEntry::compound_tag_value_safe(bucket.compound_safe_count));
        }
        if bucket.compound_review_count > 0 {
            entries.push(BucketEntry::compound_tag_value_review(bucket.compound_review_count));
        }

        entries
    }

    fn build_other_entries(other: &OtherSignalsBucket) -> Vec<BucketEntry> {
        // Entries come pre-sorted from database
        other.entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| BucketEntry::other(idx, &entry.display_label, entry.count, &entry.signal_type))
            .collect()
    }

    /// Get entries for a specific bucket
    pub fn entries_for(&self, bucket: FocusedBucket) -> &[BucketEntry] {
        match bucket {
            FocusedBucket::Corpus => &self.corpus,
            FocusedBucket::Placeholder => &self.placeholder,
            FocusedBucket::Library => &self.library,
            FocusedBucket::Other => &self.other,
        }
    }
}

// ============================================================================
// Click Target System
// ============================================================================

/// Click target result for insights list
#[derive(Debug, Clone, Copy)]
pub struct InsightClickTarget {
    pub bucket: FocusedBucket,
    pub item_index: usize,
}

/// Maps row IDs to bucket+index for click detection.
#[derive(Debug, Clone, Default)]
pub struct InsightsClickTargets {
    inner: ListClickTargets,
}

impl InsightsClickTargets {
    /// Create new empty click targets.
    pub fn new() -> Self {
        Self {
            inner: ListClickTargets::new(),
        }
    }

    /// Clear all stored targets (call at start of each render).
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Set the list area for bounds checking.
    pub fn set_list_area(&mut self, area: Rect) {
        self.inner.set_list_area(area);
    }

    /// Add a header row (not clickable for selection).
    /// We don't add headers to targets - they're not selectable.
    pub fn add_header(&mut self, _bucket: FocusedBucket, _y: u16) {
        // Headers are not clickable - intentionally empty
    }

    /// Add an item row target.
    pub fn add_item(&mut self, bucket: FocusedBucket, index: usize, y: u16) {
        // Encode bucket + index as "bucket_index" string
        let id = format!("{}_{}", bucket.index(), index);
        self.inner.add_row(id, y);
    }

    /// Check if a click hits an item, returning the bucket and index if so.
    pub fn hit_test(&self, x: u16, y: u16) -> Option<InsightClickTarget> {
        let id = self.inner.hit_test(x, y)?;

        // Parse "bucket_index" format
        let parts: Vec<&str> = id.split('_').collect();
        if parts.len() != 2 {
            return None;
        }

        let bucket_idx: usize = parts[0].parse().ok()?;
        let item_index: usize = parts[1].parse().ok()?;

        let bucket = match bucket_idx {
            0 => FocusedBucket::Corpus,
            1 => FocusedBucket::Placeholder,
            2 => FocusedBucket::Library,
            3 => FocusedBucket::Other,
            _ => return None,
        };

        Some(InsightClickTarget { bucket, item_index })
    }
}

/// State for the insights view
pub struct InsightsViewState {
    /// Modal state tracking Witch busy status
    pub modal: InsightsModal,
    /// Which bucket currently has navigation focus
    pub focused_bucket: FocusedBucket,
    /// Selection state for each bucket (indexed by FocusedBucket::index())
    pub bucket_selections: [BucketSelection; 4],
    /// Cached insights data from UiReadCache
    pub cached_data: Option<InsightsData>,
    /// Pre-computed sorted entries - rebuilt when cached_data changes
    pub cached_entries: CachedBucketEntries,
    /// Click targets for mouse selection (populated during render)
    pub click_targets: InsightsClickTargets,
}

impl Default for InsightsViewState {
    fn default() -> Self {
        Self {
            modal: InsightsModal::Ready,
            focused_bucket: FocusedBucket::default(),
            bucket_selections: Default::default(),
            cached_data: None,
            cached_entries: CachedBucketEntries::default(),
            click_targets: InsightsClickTargets::new(),
        }
    }
}

impl InsightsViewState {
    /// Create a new insights view state
    pub fn new() -> Self {
        Self::default()
    }

    /// Update state every tick - checks Witch status and caches insights data
    pub fn update(&mut self, witch_status: Option<&DaemonStatus>, insights_data: Option<InsightsData>) {
        let busy = witch_status
            .map(|s| s.pending > 0)
            .unwrap_or(false);

        self.modal = if busy {
            InsightsModal::NotReady_WitchBusy
        } else {
            InsightsModal::Ready
        };

        // Update cached data and rebuild sorted entries when new data available
        if let Some(data) = insights_data {
            self.cached_entries = CachedBucketEntries::from_insights_data(&data);
            self.cached_data = Some(data);
        }
    }

    /// Get the currently selected entry (unified across all buckets)
    pub fn selected_entry(&self) -> Option<&BucketEntry> {
        let entries = self.cached_entries.entries_for(self.focused_bucket);
        let selected_idx = self.bucket_selections[self.focused_bucket.index()].selected;
        entries.get(selected_idx)
    }

    /// Get the action for the currently selected entry
    pub fn selected_action(&self) -> Option<InsightAction> {
        self.selected_entry().map(|e| e.action)
    }

    /// Get the insight type for the currently selected entry
    pub fn selected_insight_type(&self) -> Option<InsightType> {
        self.selected_entry().map(|e| e.insight_type.clone())
    }

    /// Check if the Witch is busy (actions should be blocked)
    pub fn is_witch_busy(&self) -> bool {
        matches!(self.modal, InsightsModal::NotReady_WitchBusy)
    }

    /// Get the entry count for a specific bucket
    pub fn get_bucket_entry_count(&self, bucket: FocusedBucket) -> usize {
        self.cached_entries.entries_for(bucket).len()
    }

    /// Get current bucket's selection state (test-only)
    #[cfg(test)]
    fn current_selection(&self) -> &BucketSelection {
        &self.bucket_selections[self.focused_bucket.index()]
    }

    /// Get current bucket's selection state mutably
    fn current_selection_mut(&mut self) -> &mut BucketSelection {
        &mut self.bucket_selections[self.focused_bucket.index()]
    }

    /// Navigate up within the current bucket, or move to previous bucket
    fn navigate_up(&mut self) {
        let selection = self.current_selection_mut();

        if selection.selected > 0 {
            // Move up within current bucket
            selection.selected -= 1;
        } else {
            // At top of bucket - try to move to previous bucket
            let prev_bucket = self.focused_bucket.prev();
            if prev_bucket != self.focused_bucket {
                self.focused_bucket = prev_bucket;
                // Position at end of previous bucket
                let prev_count = self.get_bucket_entry_count(prev_bucket);
                self.current_selection_mut().selected = prev_count.saturating_sub(1);
            }
        }

        // Ensure selection is within bounds (in case entry count changed)
        let current_count = self.get_bucket_entry_count(self.focused_bucket);
        let selection = self.current_selection_mut();
        if selection.selected >= current_count && current_count > 0 {
            selection.selected = current_count - 1;
        }
    }

    /// Navigate down within the current bucket, or move to next bucket
    fn navigate_down(&mut self) {
        let entry_count = self.get_bucket_entry_count(self.focused_bucket);
        let selection = self.current_selection_mut();

        if selection.selected + 1 < entry_count {
            // Move down within current bucket
            selection.selected += 1;
        } else {
            // At bottom of bucket - try to move to next bucket
            let next_bucket = self.focused_bucket.next();
            if next_bucket != self.focused_bucket {
                self.focused_bucket = next_bucket;
                // Position at start of next bucket
                self.current_selection_mut().selected = 0;
            }
        }
    }

    /// Navigate to the very first entry (bucket 1, item 0)
    fn navigate_to_start(&mut self) {
        self.focused_bucket = FocusedBucket::Corpus;
        for selection in &mut self.bucket_selections {
            selection.selected = 0;
            selection.scroll = 0;
        }
    }

    /// Navigate to the very last entry (last bucket, last item)
    fn navigate_to_end(&mut self) {
        // Find last non-empty bucket
        for bucket in [FocusedBucket::Other, FocusedBucket::Library, FocusedBucket::Placeholder, FocusedBucket::Corpus] {
            let count = self.get_bucket_entry_count(bucket);
            if count > 0 {
                self.focused_bucket = bucket;
                self.bucket_selections[bucket.index()].selected = count - 1;
                return;
            }
        }
    }

    /// Handle mouse click, updating selection if hit.
    /// Returns true if selection changed.
    pub fn handle_click(&mut self, x: u16, y: u16) -> bool {
        if let Some(target) = self.click_targets.hit_test(x, y) {
            // Check if the bucket has items at this index
            let bucket_entry_count = self.get_bucket_entry_count(target.bucket);
            if target.item_index < bucket_entry_count {
                self.focused_bucket = target.bucket;
                self.bucket_selections[target.bucket.index()].selected = target.item_index;
                return true;
            }
        }
        false
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> InsightsAction {
        match key.code {
            KeyCode::Esc => InsightsAction::RequestQuit,

            KeyCode::Up | KeyCode::Char('k') => {
                self.navigate_up();
                InsightsAction::None
            }

            KeyCode::Down | KeyCode::Char('j') => {
                self.navigate_down();
                InsightsAction::None
            }

            KeyCode::Home => {
                self.navigate_to_start();
                InsightsAction::None
            }

            KeyCode::End => {
                self.navigate_to_end();
                InsightsAction::None
            }

            KeyCode::Enter => {
                // Block launch if the Witch is busy
                if self.is_witch_busy() {
                    return InsightsAction::None;
                }
                // TODO: Launch flow when implemented
                InsightsAction::LaunchFlow
            }

            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    InsightsAction::CyclePrev
                } else {
                    InsightsAction::CycleNext
                }
            }

            KeyCode::BackTab => InsightsAction::CyclePrev,

            _ => InsightsAction::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test InsightsData with standard bucket sizes
    fn mock_insights_data() -> InsightsData {
        InsightsData {
            bucket_corpus: CorpusFilesBucket {
                oob_tag_sync: 0,
                oob_tag_conflict: 0,
                mtime_only_mismatch: 0,
                inode_changed: 0,
                files_in_corpus: 100,
                files_indexed: 90,
                files_unindexed: 5,
                files_missing: 3,
                directories_missing: 0,
                files_relocated: 2,
                corrupt_files: 0,
                shit_format_files: 0,
                file_type_breakdown: vec![],
                _directory_breakdown: Default::default(),
            },
            bucket_placeholder: TagSquashBucket {
                directory_overlap_cluster_count: 0,
                subpar_duplicate_count: 0,
                tag_canonicity: vec![],
                inconsistent_album_artist_count: 0,
                compound_safe_count: 0,
                compound_review_count: 0,
            },
            bucket_library: LibraryDeployBucket {
                library_stale: 1,
                library_leftover: 2,
                deploy_ready: 10,
                deployed_healthy: 80,
            },
            bucket_other: OtherSignalsBucket {
                entries: vec![],
            },
        }
    }

    /// Create a state with populated cached_entries for navigation tests
    fn state_with_data() -> InsightsViewState {
        let mut state = InsightsViewState::new();
        let data = mock_insights_data();
        state.cached_entries = CachedBucketEntries::from_insights_data(&data);
        state.cached_data = Some(data);
        state
    }

    #[test]
    fn test_insights_action_exit() {
        let mut state = InsightsViewState::new();
        let action = state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(action, InsightsAction::RequestQuit);
    }

    #[test]
    fn test_witch_busy_blocks_enter() {
        let mut state = InsightsViewState::new();

        // Not busy - Enter should launch flow
        state.modal = InsightsModal::Ready;
        let action = state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(action, InsightsAction::LaunchFlow);

        // Busy - Enter should be blocked
        state.modal = InsightsModal::NotReady_WitchBusy;
        let action = state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(action, InsightsAction::None);
    }

    #[test]
    fn test_update_witch_status() {
        let mut state = InsightsViewState::new();

        // No status - should be Ready
        state.update(None, None);
        assert_eq!(state.modal, InsightsModal::Ready);

        // Pending > 0 - should be busy
        let busy_status = DaemonStatus {
            pending: 5,
            ..Default::default()
        };
        state.update(Some(&busy_status), None);
        assert_eq!(state.modal, InsightsModal::NotReady_WitchBusy);

        // Pending = 0 - should be ready again
        let idle_status = DaemonStatus {
            pending: 0,
            ..Default::default()
        };
        state.update(Some(&idle_status), None);
        assert_eq!(state.modal, InsightsModal::Ready);
    }

    #[test]
    fn test_tab_navigation_not_blocked() {
        let mut state = InsightsViewState::new();
        state.modal = InsightsModal::NotReady_WitchBusy;

        // Tab should still work even when the Witch is busy
        let action = state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(action, InsightsAction::CycleNext);

        let action = state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(action, InsightsAction::CyclePrev);
    }

    #[test]
    fn test_bucket_navigation() {
        let mut state = state_with_data();

        // Start at Corpus bucket, item 0
        assert_eq!(state.focused_bucket, FocusedBucket::Corpus);
        assert_eq!(state.current_selection().selected, 0);

        // Navigate down within corpus bucket
        state.navigate_down();
        assert_eq!(state.focused_bucket, FocusedBucket::Corpus);
        assert_eq!(state.current_selection().selected, 1);

        // Navigate to end of corpus bucket (12 items: 0-11)
        for _ in 0..10 {
            state.navigate_down();
        }
        assert_eq!(state.focused_bucket, FocusedBucket::Corpus);
        assert_eq!(state.current_selection().selected, 11);

        // Navigate down should move to Placeholder bucket
        state.navigate_down();
        assert_eq!(state.focused_bucket, FocusedBucket::Placeholder);
        assert_eq!(state.current_selection().selected, 0);

        // Navigate up should return to Corpus bucket at last item
        state.navigate_up();
        assert_eq!(state.focused_bucket, FocusedBucket::Corpus);
        assert_eq!(state.current_selection().selected, 11);
    }

    #[test]
    fn test_navigate_to_start_and_end() {
        let mut state = state_with_data();

        // Move around a bit
        state.focused_bucket = FocusedBucket::Library;
        state.bucket_selections[FocusedBucket::Library.index()].selected = 2;

        // Navigate to start
        state.navigate_to_start();
        assert_eq!(state.focused_bucket, FocusedBucket::Corpus);
        assert_eq!(state.current_selection().selected, 0);

        // Navigate to end - with no Other entries, Library is last bucket (4 items, so last is index 3)
        state.navigate_to_end();
        assert_eq!(state.focused_bucket, FocusedBucket::Library);
        assert_eq!(state.current_selection().selected, 3);
    }

    #[test]
    fn test_insights_click_targets() {
        use ratatui::layout::Rect;

        let mut targets = InsightsClickTargets::new();
        targets.set_list_area(Rect::new(0, 0, 100, 50));

        // Simulate Y positions like render would produce:
        // Y=0: Corpus header (not clickable)
        // Y=1: Corpus item 0
        // Y=2: Corpus item 1
        // Y=3: Library header (not clickable)
        // Y=4: Library item 0
        targets.add_header(FocusedBucket::Corpus, 0);
        targets.add_item(FocusedBucket::Corpus, 0, 1);
        targets.add_item(FocusedBucket::Corpus, 1, 2);
        targets.add_header(FocusedBucket::Library, 3);
        targets.add_item(FocusedBucket::Library, 0, 4);

        // Click on header - should not hit
        assert!(targets.hit_test(10, 0).is_none());

        // Click on Corpus item 0
        let hit = targets.hit_test(10, 1).unwrap();
        assert_eq!(hit.bucket, FocusedBucket::Corpus);
        assert_eq!(hit.item_index, 0);

        // Click on Corpus item 1
        let hit = targets.hit_test(10, 2).unwrap();
        assert_eq!(hit.bucket, FocusedBucket::Corpus);
        assert_eq!(hit.item_index, 1);

        // Click on Library header - should not hit
        assert!(targets.hit_test(10, 3).is_none());

        // Click on Library item 0
        let hit = targets.hit_test(10, 4).unwrap();
        assert_eq!(hit.bucket, FocusedBucket::Library);
        assert_eq!(hit.item_index, 0);

        // Click outside the list area
        assert!(targets.hit_test(150, 1).is_none());
    }

    #[test]
    fn test_handle_click() {
        use ratatui::layout::Rect;

        let mut state = state_with_data();

        // Set up click targets manually (normally done by render)
        state.click_targets.clear();
        state.click_targets.set_list_area(Rect::new(0, 0, 100, 50));
        state.click_targets.add_item(FocusedBucket::Corpus, 0, 1);
        state.click_targets.add_item(FocusedBucket::Corpus, 1, 2);
        state.click_targets.add_item(FocusedBucket::Library, 0, 5);

        // Start at default (Corpus bucket, item 0)
        assert_eq!(state.focused_bucket, FocusedBucket::Corpus);
        assert_eq!(state.bucket_selections[FocusedBucket::Corpus.index()].selected, 0);

        // Click on Corpus item 1
        assert!(state.handle_click(10, 2));
        assert_eq!(state.focused_bucket, FocusedBucket::Corpus);
        assert_eq!(state.bucket_selections[FocusedBucket::Corpus.index()].selected, 1);

        // Click on Library item 0 - should change bucket
        assert!(state.handle_click(10, 5));
        assert_eq!(state.focused_bucket, FocusedBucket::Library);
        assert_eq!(state.bucket_selections[FocusedBucket::Library.index()].selected, 0);

        // Click outside - should return false
        assert!(!state.handle_click(200, 200));
        // Selection should not change
        assert_eq!(state.focused_bucket, FocusedBucket::Library);
    }
}
