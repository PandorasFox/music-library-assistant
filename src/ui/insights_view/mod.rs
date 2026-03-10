//! Insights View Module
//!
//! A full-screen view displaying computed insights over health signals.
//! Part of the lateral view ring - can cycle to adjacent views with Tab/Shift-Tab.
//!
//! ## Three-Bucket Structure
//!
//! Insights are organized into three buckets with distinct purposes:
//! 1. **Corpus Files** - OOB changes (top priority), indexed/unindexed/missing counts
//! 2. **Tag & Duplicate Issues** - Tag canonicity, compound splits, duplicates
//! 3. **Other Signals** - Remaining signals sorted by count
//!
//! Library/Deploy was removed — deploy is now a lateral view tab.
//!
//! ## Navigation
//!
//! - Up/Down: Navigate within flat list (headers skipped automatically)
//! - Enter: Launch modal for selected insight (blocked when Witch is busy)
//! - Z: Show detail pane for selected insight (wizard system)
//! - Tab/Shift-Tab: Cycle to adjacent view
//! - Esc: Return to main menu
//!
//! ## Modal State
//!
//! The view tracks whether the Witch is busy. When busy, actionable
//! insights are dimmed and the Enter key is blocked.

mod render;

use std::collections::BTreeSet;
use std::collections::HashSet;

use crate::ui::input::InputAction;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::meta::decisions::DecisionKeyKind;
use crate::meta::views::{CorpusFilesBucket, InsightsData, OtherSignalsBucket, TagSquashBucket};
use crate::ui::widgets::standard_list::{ListEntry, ListInputResult, StandardListConfig, StandardListState};
use crate::ui::widgets::wizard::{WizardItem, WizardOffer};
use crate::witch::WorkStatus;

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
    /// Launch modal for selected insight
    Launch,
}

/// State for the insights view modal/status
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[allow(non_camel_case_types)]
pub enum InsightsModal {
    /// Ready for user interaction
    #[default]
    Ready,
    /// The Witch has operations in-flight - actions blocked
    NotReady_WitchBusy,
}

/// Which bucket an entry or header belongs to (preserved for rendering)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusedBucket {
    #[default]
    Corpus,
    Placeholder,
    Other,
}

impl FocusedBucket {
    /// Get the index of this bucket (0-2)
    pub fn index(self) -> usize {
        match self {
            FocusedBucket::Corpus => 0,
            FocusedBucket::Placeholder => 1,
            FocusedBucket::Other => 2,
        }
    }
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
    CorpusFilesInCorpus,
    CorpusFilesIndexed,
    CorpusImagesInCorpus,
    CorpusFilesUnindexed,
    CorpusFilesMissing,
    CorpusDirectoriesMissing,
    CorpusFilesRelocated,
    CorpusCorruptFiles,
    CorpusShitFormatFiles,
    // Tag resolution bucket entries (duplicates at top for easy resolution)
    CrossSourceOverlaps,
    ReleaseOverlaps,
    SubparDuplicates,
    RedundantDuplicates,
    InconsistentAlbumArtist,
    TagCanonicity { tag_name: String },
    CompoundTagValueSafe { tag_name: String }, // All split parts exist in corpus
    CompoundTagValueReview { tag_name: String }, // Some/all parts are new to corpus
    MissingAlbumSingle,
    DiscExtraction,
    PathTagMismatch,
    // Other bucket - dynamic entries identified by index
    OtherSignal { index: usize },
}

impl InsightType {
    /// For single-decision kinds, returns the `DecisionKeyKind` that fully
    /// handles all items of this insight type. Returns `None` for informational
    /// entries and per-item sources whose modals already back-fill state.
    pub fn single_decision_kind(&self) -> Option<DecisionKeyKind> {
        match self {
            InsightType::CorpusMtimeOnly => Some(DecisionKeyKind::MtimeAck),
            InsightType::CorpusOobTagSync => Some(DecisionKeyKind::OobSync),
            InsightType::CorpusOobTagConflict => Some(DecisionKeyKind::OobConflict),
            InsightType::CorpusFilesUnindexed => Some(DecisionKeyKind::IntakeIndex),
            InsightType::CorpusFilesMissing => Some(DecisionKeyKind::MissingFile),
            InsightType::CorpusDirectoriesMissing => Some(DecisionKeyKind::MissingDirectory),
            InsightType::CorpusFilesRelocated => Some(DecisionKeyKind::MovedFile),
            InsightType::CorpusCorruptFiles => Some(DecisionKeyKind::CorruptFile),
            InsightType::CorpusShitFormatFiles => Some(DecisionKeyKind::ShitFormat),
            InsightType::SubparDuplicates => Some(DecisionKeyKind::SubparDuplicate),
            // Informational entries
            InsightType::CorpusFilesInCorpus
            | InsightType::CorpusFilesIndexed
            | InsightType::CorpusImagesInCorpus => None,
            // Per-item sources — modals already back-fill from staged decisions
            InsightType::TagCanonicity { .. }
            | InsightType::CompoundTagValueSafe { .. }
            | InsightType::CompoundTagValueReview { .. }
            | InsightType::CrossSourceOverlaps
            | InsightType::ReleaseOverlaps
            | InsightType::RedundantDuplicates
            | InsightType::InconsistentAlbumArtist
            | InsightType::MissingAlbumSingle
            | InsightType::DiscExtraction
            | InsightType::PathTagMismatch
            | InsightType::OtherSignal { .. } => None,
        }
    }
}

/// Actions that can be launched from specific insight types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightAction {
    /// Launch missing file resolution modal
    LaunchMissingFileResolution,
    /// Launch missing directory acknowledgment modal
    LaunchMissingDirectoryResolution,
    /// Launch tag canonicity resolution modal
    LaunchTagCanonicityResolution,
    /// Launch compound tag split modal (safe - all parts exist)
    LaunchCompoundTagSplitSafe,
    /// Launch compound tag split modal (review - some parts new)
    LaunchCompoundTagSplitReview,
    /// Launch OOB tag sync resolution modal
    LaunchOobTagSync,
    /// Launch OOB tag conflict inspection
    LaunchOobTagConflict,
    /// Launch moved file acknowledgement modal
    LaunchMovedFileAcknowledge,
    /// Launch corrupt file resolution modal (stash + drop)
    LaunchCorruptFileResolution,
    /// Launch shit format transcode modal
    LaunchShitFormatTranscode,
    /// Launch intake confirmation for unindexed files
    LaunchIntakeConfirmation,
    /// Launch fingerprint duplicate resolution modal
    LaunchDirectoryOverlapResolution,
    /// Launch release overlap resolution modal
    LaunchReleaseOverlapResolution,
    /// Launch subpar duplicate stash
    LaunchSubparDuplicateResolution,
    /// Launch manual review modal (redundant dups, deploy conflicts, metadata dups)
    LaunchManualReview,
    /// Launch missing tag resolution (opens tag editor with all affected files)
    LaunchMissingTagResolution,
    /// Launch missing album single resolution modal
    LaunchMissingAlbumSingleResolution,
    /// Launch embedded disc number resolution
    LaunchDiscExtractionResolution,
    /// Launch path-tag schema mismatch resolution
    LaunchPathTagMismatchResolution,
    /// Not yet implemented
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

    /// Create release overlaps entry
    fn release_overlaps(count: usize) -> Self {
        Self {
            insight_type: InsightType::ReleaseOverlaps,
            label: "Release overlaps".to_string(),
            count: Some(count),
            color: if count > 0 { Color::Cyan } else { Color::Green },
            rank: 0,
            action: InsightAction::LaunchReleaseOverlapResolution,
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

    /// Create redundant duplicates entry
    fn redundant_duplicates(count: usize) -> Self {
        Self {
            insight_type: InsightType::RedundantDuplicates,
            label: "Redundant duplicates".to_string(),
            count: Some(count),
            color: if count > 0 {
                Color::Yellow
            } else {
                Color::Green
            },
            rank: 0,
            action: InsightAction::LaunchManualReview,
        }
    }

    /// Create inconsistent album_artist entry
    fn inconsistent_album_artist(count: usize) -> Self {
        Self {
            insight_type: InsightType::InconsistentAlbumArtist,
            label: "Inconsistent album_artist".to_string(),
            count: Some(count),
            color: if count > 0 {
                Color::Yellow
            } else {
                Color::Green
            },
            rank: 0,
            action: InsightAction::LaunchTagCanonicityResolution,
        }
    }

    /// Create tag canonicity entry (for a specific tag name)
    fn tag_canonicity(tag_name: &str, cluster_count: usize) -> Self {
        Self {
            insight_type: InsightType::TagCanonicity {
                tag_name: tag_name.to_string(),
            },
            label: format!("{} canonicity", tag_name),
            count: Some(cluster_count),
            color: if cluster_count > 0 {
                Color::Yellow
            } else {
                Color::Green
            },
            rank: 0,
            action: InsightAction::LaunchTagCanonicityResolution,
        }
    }

    /// Create compound tag value safe entry (all parts exist in corpus)
    fn compound_tag_value_safe(tag_name: &str, count: usize) -> Self {
        Self {
            insight_type: InsightType::CompoundTagValueSafe {
                tag_name: tag_name.to_string(),
            },
            label: format!("{} compound splits (safe)", tag_name),
            count: Some(count),
            color: if count > 0 {
                Color::Green
            } else {
                Color::DarkGray
            },
            rank: 0,
            action: InsightAction::LaunchCompoundTagSplitSafe,
        }
    }

    /// Create compound tag value review entry (some parts are new)
    fn compound_tag_value_review(tag_name: &str, count: usize) -> Self {
        Self {
            insight_type: InsightType::CompoundTagValueReview {
                tag_name: tag_name.to_string(),
            },
            label: format!("{} compound splits (review)", tag_name),
            count: Some(count),
            color: if count > 0 {
                Color::Yellow
            } else {
                Color::DarkGray
            },
            rank: 0,
            action: InsightAction::LaunchCompoundTagSplitReview,
        }
    }

    /// Create missing album single entry
    fn missing_album_single(count: usize) -> Self {
        Self {
            insight_type: InsightType::MissingAlbumSingle,
            label: "Missing album singles".to_string(),
            count: Some(count),
            color: if count > 0 {
                Color::Yellow
            } else {
                Color::DarkGray
            },
            rank: 0,
            action: InsightAction::LaunchMissingAlbumSingleResolution,
        }
    }

    /// Create embedded disc number entry
    fn disc_extraction(count: usize) -> Self {
        Self {
            insight_type: InsightType::DiscExtraction,
            label: "Disc extractions".to_string(),
            count: Some(count),
            color: if count > 0 {
                Color::Yellow
            } else {
                Color::DarkGray
            },
            rank: 0,
            action: InsightAction::LaunchDiscExtractionResolution,
        }
    }

    /// Create path-tag mismatch entry
    fn path_tag_mismatch(count: usize) -> Self {
        Self {
            insight_type: InsightType::PathTagMismatch,
            label: "Filename tag schema issues".to_string(),
            count: Some(count),
            color: if count > 0 {
                Color::Yellow
            } else {
                Color::DarkGray
            },
            rank: 0,
            action: InsightAction::LaunchPathTagMismatchResolution,
        }
    }

    /// Create "other signal" entry
    fn other(index: usize, label: &str, count: usize, signal_type: &str) -> Self {
        // Determine action based on signal type
        let action = match signal_type {
            "TagCanonicity" | "InconsistentAlbumArtist" => {
                InsightAction::LaunchTagCanonicityResolution
            }
            "CompoundTagValue" => InsightAction::LaunchCompoundTagSplitReview, // Default to review
            "missing_tag" => InsightAction::LaunchMissingTagResolution,
            "metadata_dup" | "deploy_conflict" => InsightAction::LaunchManualReview,
            _ => InsightAction::NotImplemented,
        };

        Self {
            insight_type: InsightType::OtherSignal { index },
            label: label.to_string(),
            count: Some(count),
            color: if count > 0 {
                Color::Yellow
            } else {
                Color::Green
            },
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
    pub other: Vec<BucketEntry>,
}

impl CachedBucketEntries {
    /// Build from InsightsData, applying all sorting logic once
    pub fn from_insights_data(data: &InsightsData) -> Self {
        Self {
            corpus: Self::build_corpus_entries(&data.bucket_corpus),
            placeholder: Self::build_placeholder_entries(&data.bucket_placeholder),
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
                if corpus.mtime_only_mismatch > 0 {
                    Color::Yellow
                } else {
                    Color::DarkGray
                },
                InsightAction::LaunchOobTagConflict, // Same modal as conflict, handles MtimeOnly bucket
            ),
            BucketEntry::corpus(
                InsightType::CorpusOobTagSync,
                "Tags syncable (out-of-band)",
                corpus.oob_tag_sync,
                if corpus.oob_tag_sync > 0 { 0 } else { 2 },
                if corpus.oob_tag_sync > 0 {
                    Color::Yellow
                } else {
                    Color::DarkGray
                },
                InsightAction::LaunchOobTagSync,
            ),
            BucketEntry::corpus(
                InsightType::CorpusOobTagConflict,
                "Tag conflicts (out-of-band)",
                corpus.oob_tag_conflict,
                if corpus.oob_tag_conflict > 0 { 0 } else { 2 },
                if corpus.oob_tag_conflict > 0 {
                    Color::Red
                } else {
                    Color::DarkGray
                },
                InsightAction::LaunchOobTagConflict,
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
                InsightType::CorpusImagesInCorpus,
                "Images in corpus",
                corpus.images_in_corpus,
                1,
                Color::Green,
                InsightAction::Informational,
            ),
            BucketEntry::corpus(
                InsightType::CorpusFilesUnindexed,
                "Files unindexed",
                corpus.files_unindexed,
                1,
                if corpus.files_unindexed > 0 {
                    Color::Yellow
                } else {
                    Color::Green
                },
                InsightAction::LaunchIntakeConfirmation,
            ),
            BucketEntry::corpus(
                InsightType::CorpusFilesMissing,
                "Files missing",
                corpus.files_missing,
                1,
                if corpus.files_missing > 0 {
                    Color::Red
                } else {
                    Color::Green
                },
                InsightAction::LaunchMissingFileResolution,
            ),
            BucketEntry::corpus(
                InsightType::CorpusDirectoriesMissing,
                "Directories missing",
                corpus.directories_missing,
                if corpus.directories_missing > 0 { 0 } else { 2 },
                if corpus.directories_missing > 0 {
                    Color::Red
                } else {
                    Color::DarkGray
                },
                InsightAction::LaunchMissingDirectoryResolution,
            ),
            BucketEntry::corpus(
                InsightType::CorpusFilesRelocated,
                "Files relocated (moved)",
                corpus.files_relocated,
                if corpus.files_relocated > 0 { 0 } else { 2 },
                if corpus.files_relocated > 0 {
                    Color::Yellow
                } else {
                    Color::DarkGray
                },
                InsightAction::LaunchMovedFileAcknowledge,
            ),
            BucketEntry::corpus(
                InsightType::CorpusCorruptFiles,
                "Corrupt files",
                corpus.corrupt_files,
                if corpus.corrupt_files > 0 { 0 } else { 2 },
                if corpus.corrupt_files > 0 {
                    Color::Red
                } else {
                    Color::DarkGray
                },
                InsightAction::LaunchCorruptFileResolution,
            ),
            BucketEntry::corpus(
                InsightType::CorpusShitFormatFiles,
                "Shit format files",
                corpus.shit_format_files,
                if corpus.shit_format_files > 0 { 0 } else { 2 },
                if corpus.shit_format_files > 0 {
                    Color::Yellow
                } else {
                    Color::DarkGray
                },
                InsightAction::LaunchShitFormatTranscode,
            ),
        ];

        // Sort by rank (0=top, 1=middle, 2=bottom), preserving relative order
        entries.sort_by_key(|e| e.rank);
        entries
    }

    fn build_placeholder_entries(bucket: &TagSquashBucket) -> Vec<BucketEntry> {
        let mut entries = Vec::new();

        // Cross-source overlaps at top - easy resolutions
        if bucket.directory_overlap_cluster_count > 0 {
            entries.push(BucketEntry::cross_source_overlaps(
                bucket.directory_overlap_cluster_count,
            ));
        }

        // Release overlaps - multiple releases → same album directory
        if bucket.release_overlap_count > 0 {
            entries.push(BucketEntry::release_overlaps(bucket.release_overlap_count));
        }

        // Subpar duplicates - identified low-quality copies ready to stash
        if bucket.subpar_duplicate_count > 0 {
            entries.push(BucketEntry::subpar_duplicates(
                bucket.subpar_duplicate_count,
            ));
        }

        // Redundant duplicates - equal-quality copies needing operator choice
        if bucket.redundant_duplicate_count > 0 {
            entries.push(BucketEntry::redundant_duplicates(
                bucket.redundant_duplicate_count,
            ));
        }

        // Add inconsistent album_artist if present
        if bucket.inconsistent_album_artist_count > 0 {
            entries.push(BucketEntry::inconsistent_album_artist(
                bucket.inconsistent_album_artist_count,
            ));
        }

        // Add tag canonicity entries for each tag type
        for entry in &bucket.tag_canonicity {
            entries.push(BucketEntry::tag_canonicity(
                &entry.tag_name,
                entry.cluster_count,
            ));
        }

        // Add compound tag values per tag - safe first (easy bulk action), then review
        for entry in &bucket.compound_tags {
            if entry.safe_count > 0 {
                entries.push(BucketEntry::compound_tag_value_safe(
                    &entry.tag_name,
                    entry.safe_count,
                ));
            }
            if entry.review_count > 0 {
                entries.push(BucketEntry::compound_tag_value_review(
                    &entry.tag_name,
                    entry.review_count,
                ));
            }
        }

        // Missing album singles
        if bucket.missing_album_single_count > 0 {
            entries.push(BucketEntry::missing_album_single(
                bucket.missing_album_single_count,
            ));
        }

        // Embedded disc numbers
        if bucket.disc_extraction_count > 0 {
            entries.push(BucketEntry::disc_extraction(bucket.disc_extraction_count));
        }

        // Path-tag schema mismatches
        if bucket.path_tag_mismatch_count > 0 {
            entries.push(BucketEntry::path_tag_mismatch(
                bucket.path_tag_mismatch_count,
            ));
        }

        entries
    }

    fn build_other_entries(other: &OtherSignalsBucket) -> Vec<BucketEntry> {
        // Entries come pre-sorted from database
        other
            .entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| {
                BucketEntry::other(idx, &entry.display_label, entry.count, &entry.signal_type)
            })
            .collect()
    }

    /// Remove entries whose single-decision kind is in the handled set.
    fn filter_handled(&mut self, handled: &HashSet<DecisionKeyKind>) {
        if handled.is_empty() {
            return;
        }
        let dominated = |e: &BucketEntry| {
            e.insight_type
                .single_decision_kind()
                .is_some_and(|k| handled.contains(&k))
        };
        self.corpus.retain(|e| !dominated(e));
        self.placeholder.retain(|e| !dominated(e));
        // 'other' bucket entries don't have single_decision_source mappings,
        // but retain for completeness in case the pattern extends.
        self.other.retain(|e| !dominated(e));
    }

    /// Get entries for a specific bucket
    pub fn entries_for(&self, bucket: FocusedBucket) -> &[BucketEntry] {
        match bucket {
            FocusedBucket::Corpus => &self.corpus,
            FocusedBucket::Placeholder => &self.placeholder,
            FocusedBucket::Other => &self.other,
        }
    }
}

// ============================================================================
// Flat List Item (StandardList integration)
// ============================================================================

/// A single item in the flattened insights list.
/// Headers are non-selectable separators; entries carry wizard detail panes.
pub enum InsightListItem {
    Header {
        title: String,
        bucket: FocusedBucket,
    },
    Entry {
        entry: BucketEntry,
        detail_lines: Vec<Line<'static>>,
    },
}

impl WizardItem for InsightListItem {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        match self {
            Self::Header { .. } => None,
            Self::Entry {
                detail_lines,
                ..
            } => {
                if detail_lines.is_empty() {
                    None
                } else {
                    Some(WizardOffer::Popup(detail_lines.clone()))
                }
            }
        }
    }
}

impl ListEntry for InsightListItem {
    type Action = InsightType;

    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<InsightType> {
        match self {
            Self::Header { .. } => None,
            Self::Entry { entry, .. } => Some(entry.insight_type.clone()),
        }
    }

    fn is_selectable(&self) -> bool {
        matches!(self, Self::Entry { .. })
    }
}

// ============================================================================
// Detail Line Generation (baked at construction time)
// ============================================================================

/// Generate detail lines for a bucket entry's popup. Called once during list construction.
/// Returns popup content lines with a styled title header.
fn detail_lines_for_entry(
    entry: &BucketEntry,
    data: Option<&InsightsData>,
    busy: bool,
) -> Vec<Line<'static>> {
    let text_color = if busy { Color::DarkGray } else { Color::White };
    let title_color = if busy { Color::DarkGray } else { Color::Yellow };

    let mut lines = Vec::new();
    let title;

    match entry.insight_type {
        InsightType::CorpusMtimeOnly => {
            title = "Mtime-Only Changes".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Files touched but tags unchanged.", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("Acknowledge to update scan state", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("without modifying files.", Style::default().fg(text_color))));
        }
        InsightType::CorpusOobTagSync => {
            title = "Tags Syncable (Out-of-Band)".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Files have extra tags in one direction", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("only: either on disk or in the index.", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("Can be synced to bring both in line.", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to resolve.", Style::default().fg(Color::Cyan))));
        }
        InsightType::CorpusOobTagConflict => {
            title = "Tag Conflicts (Out-of-Band)".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Files have tag values that differ", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("between disk and database, or have", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("extras in both directions.", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to inspect.", Style::default().fg(Color::Cyan))));
        }
        InsightType::CorpusCorruptFiles => {
            title = "Corrupt Files".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Files that failed to read during", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("tag verification or waveform decoding.", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to stash and drop.", Style::default().fg(Color::Cyan))));
        }
        InsightType::CorpusShitFormatFiles => {
            title = "Shit Format Files".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Non-Vorbis container files (MP3, M4A,", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("WAV, etc.) with poor metadata support.", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to transcode to Opus.", Style::default().fg(Color::Cyan))));
        }
        InsightType::CorpusFilesInCorpus => {
            title = "Files in Corpus".to_string();
            lines.push(Line::from(""));
            if let Some(data) = data {
                if !data.bucket_corpus.file_type_breakdown.is_empty() {
                    lines.push(Line::from(Span::styled("By file type:", Style::default().fg(text_color))));
                    for (ext, count) in &data.bucket_corpus.file_type_breakdown {
                        lines.push(Line::from(Span::styled(format!("  .{}: {}", ext, count), Style::default().fg(text_color))));
                    }
                } else {
                    lines.push(Line::from(Span::styled("No files found.", Style::default().fg(text_color))));
                }
            } else {
                lines.push(Line::from(Span::styled("Loading...", Style::default().fg(Color::DarkGray))));
            }
        }
        InsightType::CorpusFilesIndexed => {
            title = "Files Indexed".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Audio files with complete metadata", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("in the database.", Style::default().fg(text_color))));
        }
        InsightType::CorpusImagesInCorpus => {
            title = "Images in Corpus".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Image files (sidecar album art, etc.)", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("indexed in the corpus.", Style::default().fg(text_color))));
        }
        InsightType::CorpusFilesUnindexed => {
            title = "Files Unindexed".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Audio files in corpus not yet", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("indexed. Run indexing to process.", Style::default().fg(text_color))));
        }
        InsightType::CorpusFilesMissing => {
            title = "Files Missing".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Indexed files no longer found", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("at expected path. May have been", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("moved or deleted.", Style::default().fg(text_color))));
        }
        InsightType::CorpusDirectoriesMissing => {
            title = "Directories Missing".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Indexed directories no longer found", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("on disk. May have been moved or", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("deleted externally.", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to drop from index.", Style::default().fg(Color::Cyan))));
        }
        InsightType::CorpusFilesRelocated => {
            title = "Files Relocated (Moved)".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Files moved within corpus (same inode,", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("different path). Database paths need", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("updating to match new locations.", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to acknowledge and update paths.", Style::default().fg(Color::Cyan))));
        }
        InsightType::CrossSourceOverlaps => {
            title = "Cross-Source Overlaps".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Same tracks exist in different source", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("directories (e.g., bandcamp vs indie).", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to resolve by source.", Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }))));
        }
        InsightType::ReleaseOverlaps => {
            title = "Release Overlaps".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Multiple releases deploy into the", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("same album directory. Stash the", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("inferior release or fix tags.", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to resolve.", Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }))));
        }
        InsightType::SubparDuplicates => {
            title = "Subpar Duplicates".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Lower quality versions of tracks", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("identified by fingerprint analysis.", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to stash subpar copies.", Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }))));
        }
        InsightType::RedundantDuplicates => {
            title = "Redundant Duplicates".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Same fingerprint, identical quality.", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("Neither file is subpar — requires", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("operator choice.", Style::default().fg(text_color))));
        }
        InsightType::InconsistentAlbumArtist => {
            title = "Inconsistent Album Artist".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Albums with multiple artists but", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("missing or inconsistent album_artist.", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to resolve.", Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }))));
        }
        InsightType::TagCanonicity { ref tag_name } => {
            title = format!("{} Canonicity", tag_name);
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(format!("Variants of {} tags that should", tag_name), Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("be unified (e.g., spelling differences).", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to resolve.", Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }))));
        }
        InsightType::CompoundTagValueSafe { ref tag_name } => {
            title = format!("{} Compound Splits (Safe)", tag_name);
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(format!("All split parts for {} tags already", tag_name), Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("exist in corpus. Safe to split in bulk.", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to bulk split.", Style::default().fg(if busy { Color::DarkGray } else { Color::Green }))));
        }
        InsightType::CompoundTagValueReview { ref tag_name } => {
            title = format!("{} Compound Splits (Review)", tag_name);
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(format!("Some split parts for {} tags are", tag_name), Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("new to corpus. Review each to verify.", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to review.", Style::default().fg(if busy { Color::DarkGray } else { Color::Yellow }))));
        }
        InsightType::MissingAlbumSingle => {
            title = "Missing Album Singles".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Tracks with ARTIST and TITLE but no", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("ALBUM tag, grouped by artist.", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Press Enter to assign album values.", Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }))));
        }
        InsightType::DiscExtraction => {
            title = "Disc Extractions".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Disc values embedded in ALBUM or", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("TRACKNUMBER tags (e.g., \"Album, Disc 2\"", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("or track number \"A01\").", Style::default().fg(text_color))));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Extract to DISCNUMBER + clean source tag.", Style::default().fg(if busy { Color::DarkGray } else { Color::Cyan }))));
        }
        InsightType::PathTagMismatch => {
            title = "Filename Tag Schema Issues".to_string();
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled("Files where tags derived from the", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("filename path don't match embedded", Style::default().fg(text_color))));
            lines.push(Line::from(Span::styled("tag values.", Style::default().fg(text_color))));
        }
        InsightType::OtherSignal { index } => {
            // Get extended info from cached_data if available
            if let Some(data) = data {
                if let Some(signal_entry) = data.bucket_other.entries.get(index) {
                    title = signal_entry.display_label.clone();
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(format!("Count: {}", signal_entry.count), Style::default().fg(text_color))));
                    if let Some(affected) = signal_entry.affected_count {
                        lines.push(Line::from(Span::styled(format!("Affected tracks: {}", affected), Style::default().fg(text_color))));
                    }
                    lines.insert(0, Line::from(Span::styled(title, Style::default().fg(title_color).add_modifier(Modifier::BOLD))));
                    return lines;
                }
            }
            // Fallback
            title = entry.label.clone();
            lines.push(Line::from(""));
            if let Some(count) = entry.count {
                lines.push(Line::from(Span::styled(format!("Count: {}", count), Style::default().fg(text_color))));
            }
        }
    }

    lines.insert(0, Line::from(Span::styled(title, Style::default().fg(title_color).add_modifier(Modifier::BOLD))));
    lines
}

// ============================================================================
// Flat List Building
// ============================================================================

/// Build the flat list of items from cached bucket entries.
fn build_flat_items(
    entries: &CachedBucketEntries,
    data: Option<&InsightsData>,
    busy: bool,
) -> Vec<InsightListItem> {
    let mut items = Vec::new();

    // Corpus Files bucket
    items.push(InsightListItem::Header {
        title: "Corpus Files".to_string(),
        bucket: FocusedBucket::Corpus,
    });
    for entry in &entries.corpus {
        let detail_lines = detail_lines_for_entry(entry, data, busy);
        items.push(InsightListItem::Entry {
            entry: entry.clone(),
            detail_lines,
        });
    }

    // Tag health bucket
    items.push(InsightListItem::Header {
        title: "Tag health".to_string(),
        bucket: FocusedBucket::Placeholder,
    });
    for entry in &entries.placeholder {
        let detail_lines = detail_lines_for_entry(entry, data, busy);
        items.push(InsightListItem::Entry {
            entry: entry.clone(),
            detail_lines,
        });
    }

    // Other Signals bucket
    items.push(InsightListItem::Header {
        title: "Other Signals".to_string(),
        bucket: FocusedBucket::Other,
    });
    if entries.other.is_empty() {
        // Empty placeholder — show as a dimmed non-actionable entry
        items.push(InsightListItem::Entry {
            entry: BucketEntry {
                insight_type: InsightType::OtherSignal { index: 0 },
                label: "(no other signals)".to_string(),
                count: None,
                color: Color::DarkGray,
                rank: 0,
                action: InsightAction::Informational,
            },
            detail_lines: vec![],
        });
    } else {
        for entry in &entries.other {
            let detail_lines = detail_lines_for_entry(entry, data, busy);
            items.push(InsightListItem::Entry {
                entry: entry.clone(),
                detail_lines,
            });
        }
    }

    items
}

// ============================================================================
// Insights View State
// ============================================================================

/// State for the insights view
pub struct InsightsViewState {
    /// Modal state tracking Witch busy status
    pub modal: InsightsModal,
    /// Cached insights data from UiReadCache
    pub cached_data: Option<InsightsData>,
    /// Pre-computed sorted entries - rebuilt when cached_data changes
    pub cached_entries: CachedBucketEntries,
    /// Flattened list items (headers + entries)
    pub flat_items: Vec<InsightListItem>,
    /// StandardList state machine
    pub list: StandardListState,
    /// Last handled-kinds set, for change detection
    last_handled_sources: HashSet<DecisionKeyKind>,
}

impl Default for InsightsViewState {
    fn default() -> Self {
        Self {
            modal: InsightsModal::Ready,
            cached_data: None,
            cached_entries: CachedBucketEntries::default(),
            flat_items: Vec::new(),
            list: StandardListState::new(StandardListConfig::default()),
            last_handled_sources: HashSet::new(),
        }
    }
}

impl InsightsViewState {
    /// Create a new insights view state
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild the flat item list from cached entries.
    fn rebuild_flat_items(&mut self) {
        let busy = self.is_witch_busy();
        self.flat_items = build_flat_items(
            &self.cached_entries,
            self.cached_data.as_ref(),
            busy,
        );
        self.list.clamp_cursor(&self.flat_items);
    }

    /// Update state every tick - checks Witch status and caches insights data.
    pub fn update(
        &mut self,
        witch_status: Option<&WorkStatus>,
        insights_data: Option<InsightsData>,
        handled_sources: &HashSet<DecisionKeyKind>,
        cache_stale: bool,
    ) {
        let busy = cache_stale || witch_status.map(|s| s.pending > 0).unwrap_or(false);

        self.modal = if busy {
            InsightsModal::NotReady_WitchBusy
        } else {
            InsightsModal::Ready
        };

        let handled_changed = *handled_sources != self.last_handled_sources;

        // Rebuild when new insights data arrives OR when the handled set changes
        if insights_data.is_some() || handled_changed {
            // Use new data if available, otherwise rebuild from cached data
            if let Some(data) = insights_data.as_ref().or(self.cached_data.as_ref()) {
                let mut entries = CachedBucketEntries::from_insights_data(data);
                entries.filter_handled(handled_sources);
                self.cached_entries = entries;
            }

            if let Some(data) = insights_data {
                self.cached_data = Some(data);
            }

            if handled_changed {
                self.last_handled_sources = handled_sources.clone();
            }

            self.rebuild_flat_items();
        }
    }

    /// Get the currently selected entry
    pub fn selected_entry(&self) -> Option<&BucketEntry> {
        match self.flat_items.get(self.list.cursor) {
            Some(InsightListItem::Entry { entry, .. }) => Some(entry),
            _ => None,
        }
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

    /// Handle mouse click, updating selection if hit.
    /// Returns true if selection changed.
    pub fn handle_click(&mut self, x: u16, y: u16) -> bool {
        self.list.handle_click(x, y, &self.flat_items).is_some()
    }

    /// Handle semantic input action
    pub fn handle_input(&mut self, action: &InputAction) -> InsightsAction {
        let result = self.list.handle_input(action, &self.flat_items);

        match result {
            ListInputResult::Consumed | ListInputResult::CursorMoved | ListInputResult::Toggled => {
                InsightsAction::None
            }
            ListInputResult::Confirm(_insight_type) => {
                // Block launch if the Witch is busy
                if self.is_witch_busy() {
                    return InsightsAction::None;
                }
                InsightsAction::Launch
            }
            ListInputResult::Unhandled => {
                // Handle actions that StandardList doesn't know about
                match action {
                    InputAction::Cancel => InsightsAction::RequestQuit,
                    InputAction::CycleNext => InsightsAction::CycleNext,
                    InputAction::CyclePrev => InsightsAction::CyclePrev,
                    _ => InsightsAction::None,
                }
            }
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
                files_in_corpus: 100,
                files_indexed: 90,
                files_unindexed: 5,
                files_missing: 3,
                directories_missing: 0,
                files_relocated: 2,
                corrupt_files: 0,
                shit_format_files: 0,
                images_in_corpus: 10,
                file_type_breakdown: vec![],
                _directory_breakdown: Default::default(),
            },
            bucket_placeholder: TagSquashBucket {
                directory_overlap_cluster_count: 0,
                release_overlap_count: 0,
                subpar_duplicate_count: 0,
                redundant_duplicate_count: 0,
                tag_canonicity: vec![],
                inconsistent_album_artist_count: 0,
                compound_tags: vec![],
                missing_album_single_count: 0,
                disc_extraction_count: 0,
                path_tag_mismatch_count: 0,
            },
            bucket_other: OtherSignalsBucket { entries: vec![] },
        }
    }

    /// Create a state with populated cached_entries for navigation tests
    fn state_with_data() -> InsightsViewState {
        let mut state = InsightsViewState::new();
        let data = mock_insights_data();
        state.cached_entries = CachedBucketEntries::from_insights_data(&data);
        state.cached_data = Some(data);
        state.rebuild_flat_items();
        state
    }

    #[test]
    fn test_insights_action_exit() {
        let mut state = InsightsViewState::new();
        let action = state.handle_input(&InputAction::Cancel);
        assert_eq!(action, InsightsAction::RequestQuit);
    }

    #[test]
    fn test_witch_busy_blocks_enter() {
        let mut state = state_with_data();

        // Not busy - Enter should launch modal
        state.modal = InsightsModal::Ready;
        let action = state.handle_input(&InputAction::Confirm);
        assert_eq!(action, InsightsAction::Launch);

        // Busy - Enter should be blocked
        state.modal = InsightsModal::NotReady_WitchBusy;
        let action = state.handle_input(&InputAction::Confirm);
        assert_eq!(action, InsightsAction::None);
    }

    #[test]
    fn test_update_witch_status() {
        let mut state = InsightsViewState::new();
        let no_handled = HashSet::new();

        // No status - should be Ready
        state.update(None, None, &no_handled, false);
        assert_eq!(state.modal, InsightsModal::Ready);

        // Pending > 0 - should be busy
        let busy_status = WorkStatus {
            pending: 5,
            ..Default::default()
        };
        state.update(Some(&busy_status), None, &no_handled, false);
        assert_eq!(state.modal, InsightsModal::NotReady_WitchBusy);

        // Pending = 0 - should be ready again
        let idle_status = WorkStatus {
            pending: 0,
            ..Default::default()
        };
        state.update(Some(&idle_status), None, &no_handled, false);
        assert_eq!(state.modal, InsightsModal::Ready);
    }

    #[test]
    fn test_filter_hides_handled_entries() {
        let mut state = InsightsViewState::new();
        let data = mock_insights_data();
        let no_handled = HashSet::new();

        // Populate with data, no filtering
        state.update(None, Some(data.clone()), &no_handled, false);
        let corpus_count_before = state.cached_entries.corpus.len();
        assert!(corpus_count_before > 0);

        // Now mark MtimeAck as handled — CorpusMtimeOnly should disappear
        let mut handled = HashSet::new();
        handled.insert(DecisionKeyKind::MtimeAck);
        state.update(None, None, &handled, false);

        // Should have one fewer entry
        assert_eq!(state.cached_entries.corpus.len(), corpus_count_before - 1);
        // And it shouldn't contain CorpusMtimeOnly
        assert!(!state
            .cached_entries
            .corpus
            .iter()
            .any(|e| e.insight_type == InsightType::CorpusMtimeOnly));
    }

    #[test]
    fn test_informational_entries_survive_filtering() {
        let mut state = InsightsViewState::new();
        let data = mock_insights_data();

        // Handle several sources
        let mut handled = HashSet::new();
        handled.insert(DecisionKeyKind::MtimeAck);
        handled.insert(DecisionKeyKind::OobSync);
        handled.insert(DecisionKeyKind::MissingFile);

        state.update(None, Some(data), &handled, false);

        // Informational entries (FilesInCorpus, FilesIndexed) should survive
        assert!(state
            .cached_entries
            .corpus
            .iter()
            .any(|e| e.insight_type == InsightType::CorpusFilesInCorpus));
        assert!(state
            .cached_entries
            .corpus
            .iter()
            .any(|e| e.insight_type == InsightType::CorpusFilesIndexed));
    }

    #[test]
    fn test_selection_clamped_after_filter() {
        let mut state = InsightsViewState::new();
        let data = mock_insights_data();
        let no_handled = HashSet::new();

        // Populate and select last item
        state.update(None, Some(data.clone()), &no_handled, false);
        // Move to end
        state.list.cursor = state.flat_items.len().saturating_sub(1);

        // Handle multiple sources to shrink the list
        let mut handled = HashSet::new();
        handled.insert(DecisionKeyKind::MtimeAck);
        handled.insert(DecisionKeyKind::OobSync);
        handled.insert(DecisionKeyKind::OobConflict);
        handled.insert(DecisionKeyKind::MissingFile);
        handled.insert(DecisionKeyKind::MissingDirectory);
        handled.insert(DecisionKeyKind::MovedFile);
        handled.insert(DecisionKeyKind::CorruptFile);
        handled.insert(DecisionKeyKind::ShitFormat);
        handled.insert(DecisionKeyKind::IntakeIndex);

        state.update(None, None, &handled, false);

        // Cursor should be within bounds
        assert!(state.list.cursor < state.flat_items.len());
    }

    #[test]
    fn test_handled_set_change_triggers_rebuild() {
        let mut state = InsightsViewState::new();
        let data = mock_insights_data();
        let no_handled = HashSet::new();

        // Initial populate
        state.update(None, Some(data), &no_handled, false);
        let count_before = state.cached_entries.corpus.len();

        // Change handled set without new InsightsData — should still rebuild
        let mut handled = HashSet::new();
        handled.insert(DecisionKeyKind::MtimeAck);
        state.update(None, None, &handled, false);
        assert_eq!(state.cached_entries.corpus.len(), count_before - 1);

        // Discard (empty handled) — should restore
        state.update(None, None, &no_handled, false);
        assert_eq!(state.cached_entries.corpus.len(), count_before);
    }

    #[test]
    fn test_tab_navigation_not_blocked() {
        let mut state = InsightsViewState::new();
        state.modal = InsightsModal::NotReady_WitchBusy;

        // Tab should still work even when the Witch is busy
        let action = state.handle_input(&InputAction::CycleNext);
        assert_eq!(action, InsightsAction::CycleNext);

        let action = state.handle_input(&InputAction::CyclePrev);
        assert_eq!(action, InsightsAction::CyclePrev);
    }

    #[test]
    fn test_flat_items_have_headers() {
        let state = state_with_data();
        // Should have 3 headers + entries
        let header_count = state
            .flat_items
            .iter()
            .filter(|i| matches!(i, InsightListItem::Header { .. }))
            .count();
        assert_eq!(header_count, 3);
    }

    #[test]
    fn test_cursor_skips_headers() {
        let mut state = state_with_data();
        // Cursor should start on first selectable item (index 1, after first header)
        state.list.clamp_cursor(&state.flat_items);
        assert!(matches!(
            state.flat_items[state.list.cursor],
            InsightListItem::Entry { .. }
        ));
    }

    #[test]
    fn test_navigation_down_and_up() {
        let mut state = state_with_data();
        state.list.set_visible_height(30);
        state.list.clamp_cursor(&state.flat_items);
        let start = state.list.cursor;

        // Navigate down
        state.handle_input(&InputAction::NavDown);
        assert!(state.list.cursor > start);
        // Should still be on an entry, not a header
        assert!(matches!(
            state.flat_items[state.list.cursor],
            InsightListItem::Entry { .. }
        ));

        // Navigate back up
        state.handle_input(&InputAction::NavUp);
        assert_eq!(state.list.cursor, start);
    }

    #[test]
    fn test_selected_entry_returns_bucket_entry() {
        let state = state_with_data();
        // Should be able to get the selected entry
        let entry = state.selected_entry();
        assert!(entry.is_some());
    }
}
