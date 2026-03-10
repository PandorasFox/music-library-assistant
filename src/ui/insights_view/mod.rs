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
    /// Corpus problem entry: bubbles to top when count > 0, sinks when zero.
    fn problem(ty: InsightType, label: &str, count: usize, nonzero_color: Color, action: InsightAction) -> Self {
        Self {
            insight_type: ty, label: label.to_string(), count: Some(count),
            color: if count > 0 { nonzero_color } else { Color::DarkGray },
            rank: if count > 0 { 0 } else { 2 }, action,
        }
    }

    /// Corpus informational entry: always rank 1, fixed color, no action.
    fn info(ty: InsightType, label: &str, count: usize, color: Color) -> Self {
        Self {
            insight_type: ty, label: label.to_string(), count: Some(count),
            color, rank: 1, action: InsightAction::Informational,
        }
    }

    /// Corpus entry with action and count-dependent color, always rank 1.
    fn active(ty: InsightType, label: &str, count: usize, nonzero: Color, zero: Color, action: InsightAction) -> Self {
        Self {
            insight_type: ty, label: label.to_string(), count: Some(count),
            color: if count > 0 { nonzero } else { zero }, rank: 1, action,
        }
    }

    /// Tag health bucket entry: rank 0, count-dependent color.
    fn counted(ty: InsightType, label: impl Into<String>, count: usize, nonzero: Color, zero: Color, action: InsightAction) -> Self {
        Self {
            insight_type: ty, label: label.into(), count: Some(count),
            color: if count > 0 { nonzero } else { zero }, rank: 0, action,
        }
    }

    /// "Other signal" entry with action inferred from signal type.
    fn other(index: usize, label: &str, count: usize, signal_type: &str) -> Self {
        let action = match signal_type {
            "TagCanonicity" | "InconsistentAlbumArtist" => InsightAction::LaunchTagCanonicityResolution,
            "CompoundTagValue" => InsightAction::LaunchCompoundTagSplitReview,
            "missing_tag" => InsightAction::LaunchMissingTagResolution,
            "metadata_dup" | "deploy_conflict" => InsightAction::LaunchManualReview,
            _ => InsightAction::NotImplemented,
        };
        Self {
            insight_type: InsightType::OtherSignal { index }, label: label.to_string(),
            count: Some(count), color: if count > 0 { Color::Yellow } else { Color::Green },
            rank: 0, action,
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

    fn build_corpus_entries(c: &CorpusFilesBucket) -> Vec<BucketEntry> {
        let mut entries = vec![
            BucketEntry::problem(InsightType::CorpusMtimeOnly, "Mtime changes (ack needed)", c.mtime_only_mismatch, Color::Yellow, InsightAction::LaunchOobTagConflict),
            BucketEntry::problem(InsightType::CorpusOobTagSync, "Tags syncable (out-of-band)", c.oob_tag_sync, Color::Yellow, InsightAction::LaunchOobTagSync),
            BucketEntry::problem(InsightType::CorpusOobTagConflict, "Tag conflicts (out-of-band)", c.oob_tag_conflict, Color::Red, InsightAction::LaunchOobTagConflict),
            BucketEntry::info(InsightType::CorpusFilesInCorpus, "Files in corpus", c.files_in_corpus, Color::Yellow),
            BucketEntry::info(InsightType::CorpusFilesIndexed, "Files indexed", c.files_indexed, Color::Green),
            BucketEntry::info(InsightType::CorpusImagesInCorpus, "Images in corpus", c.images_in_corpus, Color::Green),
            BucketEntry::active(InsightType::CorpusFilesUnindexed, "Files unindexed", c.files_unindexed, Color::Yellow, Color::Green, InsightAction::LaunchIntakeConfirmation),
            BucketEntry::active(InsightType::CorpusFilesMissing, "Files missing", c.files_missing, Color::Red, Color::Green, InsightAction::LaunchMissingFileResolution),
            BucketEntry::problem(InsightType::CorpusDirectoriesMissing, "Directories missing", c.directories_missing, Color::Red, InsightAction::LaunchMissingDirectoryResolution),
            BucketEntry::problem(InsightType::CorpusFilesRelocated, "Files relocated (moved)", c.files_relocated, Color::Yellow, InsightAction::LaunchMovedFileAcknowledge),
            BucketEntry::problem(InsightType::CorpusCorruptFiles, "Corrupt files", c.corrupt_files, Color::Red, InsightAction::LaunchCorruptFileResolution),
            BucketEntry::problem(InsightType::CorpusShitFormatFiles, "Shit format files", c.shit_format_files, Color::Yellow, InsightAction::LaunchShitFormatTranscode),
        ];
        entries.sort_by_key(|e| e.rank);
        entries
    }

    fn build_placeholder_entries(bucket: &TagSquashBucket) -> Vec<BucketEntry> {
        let mut entries = Vec::new();

        if bucket.directory_overlap_cluster_count > 0 {
            entries.push(BucketEntry::counted(InsightType::CrossSourceOverlaps, "Cross-source overlaps", bucket.directory_overlap_cluster_count, Color::Cyan, Color::Green, InsightAction::LaunchDirectoryOverlapResolution));
        }
        if bucket.release_overlap_count > 0 {
            entries.push(BucketEntry::counted(InsightType::ReleaseOverlaps, "Release overlaps", bucket.release_overlap_count, Color::Cyan, Color::Green, InsightAction::LaunchReleaseOverlapResolution));
        }
        if bucket.subpar_duplicate_count > 0 {
            entries.push(BucketEntry::counted(InsightType::SubparDuplicates, "Subpar duplicates", bucket.subpar_duplicate_count, Color::Cyan, Color::Green, InsightAction::LaunchSubparDuplicateResolution));
        }
        if bucket.redundant_duplicate_count > 0 {
            entries.push(BucketEntry::counted(InsightType::RedundantDuplicates, "Redundant duplicates", bucket.redundant_duplicate_count, Color::Yellow, Color::Green, InsightAction::LaunchManualReview));
        }
        if bucket.inconsistent_album_artist_count > 0 {
            entries.push(BucketEntry::counted(InsightType::InconsistentAlbumArtist, "Inconsistent album_artist", bucket.inconsistent_album_artist_count, Color::Yellow, Color::Green, InsightAction::LaunchTagCanonicityResolution));
        }
        for entry in &bucket.tag_canonicity {
            entries.push(BucketEntry::counted(
                InsightType::TagCanonicity { tag_name: entry.tag_name.clone() },
                format!("{} canonicity", entry.tag_name), entry.cluster_count,
                Color::Yellow, Color::Green, InsightAction::LaunchTagCanonicityResolution,
            ));
        }
        for entry in &bucket.compound_tags {
            if entry.safe_count > 0 {
                entries.push(BucketEntry::counted(
                    InsightType::CompoundTagValueSafe { tag_name: entry.tag_name.clone() },
                    format!("{} compound splits (safe)", entry.tag_name), entry.safe_count,
                    Color::Green, Color::DarkGray, InsightAction::LaunchCompoundTagSplitSafe,
                ));
            }
            if entry.review_count > 0 {
                entries.push(BucketEntry::counted(
                    InsightType::CompoundTagValueReview { tag_name: entry.tag_name.clone() },
                    format!("{} compound splits (review)", entry.tag_name), entry.review_count,
                    Color::Yellow, Color::DarkGray, InsightAction::LaunchCompoundTagSplitReview,
                ));
            }
        }
        if bucket.missing_album_single_count > 0 {
            entries.push(BucketEntry::counted(InsightType::MissingAlbumSingle, "Missing album singles", bucket.missing_album_single_count, Color::Yellow, Color::DarkGray, InsightAction::LaunchMissingAlbumSingleResolution));
        }
        if bucket.disc_extraction_count > 0 {
            entries.push(BucketEntry::counted(InsightType::DiscExtraction, "Disc extractions", bucket.disc_extraction_count, Color::Yellow, Color::DarkGray, InsightAction::LaunchDiscExtractionResolution));
        }
        if bucket.path_tag_mismatch_count > 0 {
            entries.push(BucketEntry::counted(InsightType::PathTagMismatch, "Filename tag schema issues", bucket.path_tag_mismatch_count, Color::Yellow, Color::DarkGray, InsightAction::LaunchPathTagMismatchResolution));
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

}

// ============================================================================
// Flat List Item (StandardList integration)
// ============================================================================

/// A single item in the flattened insights list.
/// Headers are non-selectable separators; entries carry wizard detail panes.
pub enum InsightListItem {
    Header {
        title: String,
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

/// Build a detail popup with title, description lines, and optional CTA.
fn detail_popup(
    title: &str,
    desc: &[&str],
    cta: Option<(&str, Color)>,
    text_color: Color,
    title_color: Color,
) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(desc.len() + 4);
    lines.push(Line::from(Span::styled(title.to_string(), Style::default().fg(title_color).add_modifier(Modifier::BOLD))));
    lines.push(Line::from(""));
    for text in desc {
        lines.push(Line::from(Span::styled(text.to_string(), Style::default().fg(text_color))));
    }
    if let Some((cta_text, color)) = cta {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(cta_text.to_string(), Style::default().fg(color))));
    }
    lines
}

/// Generate detail lines for a bucket entry's popup. Called once during list construction.
fn detail_lines_for_entry(
    entry: &BucketEntry,
    data: Option<&InsightsData>,
    busy: bool,
) -> Vec<Line<'static>> {
    let tc = if busy { Color::DarkGray } else { Color::White };
    let hc = if busy { Color::DarkGray } else { Color::Yellow };
    let cta = |c: Color| if busy { Color::DarkGray } else { c };

    match entry.insight_type {
        InsightType::CorpusMtimeOnly => detail_popup("Mtime-Only Changes", &[
            "Files touched but tags unchanged.",
            "Acknowledge to update scan state",
            "without modifying files.",
        ], None, tc, hc),
        InsightType::CorpusOobTagSync => detail_popup("Tags Syncable (Out-of-Band)", &[
            "Files have extra tags in one direction",
            "only: either on disk or in the index.",
            "Can be synced to bring both in line.",
        ], Some(("Press Enter to resolve.", Color::Cyan)), tc, hc),
        InsightType::CorpusOobTagConflict => detail_popup("Tag Conflicts (Out-of-Band)", &[
            "Files have tag values that differ",
            "between disk and database, or have",
            "extras in both directions.",
        ], Some(("Press Enter to inspect.", Color::Cyan)), tc, hc),
        InsightType::CorpusCorruptFiles => detail_popup("Corrupt Files", &[
            "Files that failed to read during",
            "tag verification or waveform decoding.",
        ], Some(("Press Enter to stash and drop.", Color::Cyan)), tc, hc),
        InsightType::CorpusShitFormatFiles => detail_popup("Shit Format Files", &[
            "Non-Vorbis container files (MP3, M4A,",
            "WAV, etc.) with poor metadata support.",
        ], Some(("Press Enter to transcode to Opus.", Color::Cyan)), tc, hc),
        InsightType::CorpusFilesInCorpus => {
            let mut lines = vec![
                Line::from(Span::styled("Files in Corpus".to_string(), Style::default().fg(hc).add_modifier(Modifier::BOLD))),
                Line::from(""),
            ];
            if let Some(data) = data {
                if !data.bucket_corpus.file_type_breakdown.is_empty() {
                    lines.push(Line::from(Span::styled("By file type:".to_string(), Style::default().fg(tc))));
                    for (ext, count) in &data.bucket_corpus.file_type_breakdown {
                        lines.push(Line::from(Span::styled(format!("  .{}: {}", ext, count), Style::default().fg(tc))));
                    }
                } else {
                    lines.push(Line::from(Span::styled("No files found.".to_string(), Style::default().fg(tc))));
                }
            } else {
                lines.push(Line::from(Span::styled("Loading...".to_string(), Style::default().fg(Color::DarkGray))));
            }
            lines
        }
        InsightType::CorpusFilesIndexed => detail_popup("Files Indexed", &[
            "Audio files with complete metadata",
            "in the database.",
        ], None, tc, hc),
        InsightType::CorpusImagesInCorpus => detail_popup("Images in Corpus", &[
            "Image files (sidecar album art, etc.)",
            "indexed in the corpus.",
        ], None, tc, hc),
        InsightType::CorpusFilesUnindexed => detail_popup("Files Unindexed", &[
            "Audio files in corpus not yet",
            "indexed. Run indexing to process.",
        ], None, tc, hc),
        InsightType::CorpusFilesMissing => detail_popup("Files Missing", &[
            "Indexed files no longer found",
            "at expected path. May have been",
            "moved or deleted.",
        ], None, tc, hc),
        InsightType::CorpusDirectoriesMissing => detail_popup("Directories Missing", &[
            "Indexed directories no longer found",
            "on disk. May have been moved or",
            "deleted externally.",
        ], Some(("Press Enter to drop from index.", Color::Cyan)), tc, hc),
        InsightType::CorpusFilesRelocated => detail_popup("Files Relocated (Moved)", &[
            "Files moved within corpus (same inode,",
            "different path). Database paths need",
            "updating to match new locations.",
        ], Some(("Press Enter to acknowledge and update paths.", Color::Cyan)), tc, hc),
        InsightType::CrossSourceOverlaps => detail_popup("Cross-Source Overlaps", &[
            "Same tracks exist in different source",
            "directories (e.g., bandcamp vs indie).",
        ], Some(("Press Enter to resolve by source.", cta(Color::Cyan))), tc, hc),
        InsightType::ReleaseOverlaps => detail_popup("Release Overlaps", &[
            "Multiple releases deploy into the",
            "same album directory. Stash the",
            "inferior release or fix tags.",
        ], Some(("Press Enter to resolve.", cta(Color::Cyan))), tc, hc),
        InsightType::SubparDuplicates => detail_popup("Subpar Duplicates", &[
            "Lower quality versions of tracks",
            "identified by fingerprint analysis.",
        ], Some(("Press Enter to stash subpar copies.", cta(Color::Cyan))), tc, hc),
        InsightType::RedundantDuplicates => detail_popup("Redundant Duplicates", &[
            "Same fingerprint, identical quality.",
            "Neither file is subpar \u{2014} requires",
            "operator choice.",
        ], None, tc, hc),
        InsightType::InconsistentAlbumArtist => detail_popup("Inconsistent Album Artist", &[
            "Albums with multiple artists but",
            "missing or inconsistent album_artist.",
        ], Some(("Press Enter to resolve.", cta(Color::Cyan))), tc, hc),
        InsightType::TagCanonicity { ref tag_name } => {
            let desc1 = format!("Variants of {} tags that should", tag_name);
            detail_popup(&format!("{} Canonicity", tag_name), &[
                &desc1, "be unified (e.g., spelling differences).",
            ], Some(("Press Enter to resolve.", cta(Color::Cyan))), tc, hc)
        }
        InsightType::CompoundTagValueSafe { ref tag_name } => {
            let desc1 = format!("All split parts for {} tags already", tag_name);
            detail_popup(&format!("{} Compound Splits (Safe)", tag_name), &[
                &desc1, "exist in corpus. Safe to split in bulk.",
            ], Some(("Press Enter to bulk split.", cta(Color::Green))), tc, hc)
        }
        InsightType::CompoundTagValueReview { ref tag_name } => {
            let desc1 = format!("Some split parts for {} tags are", tag_name);
            detail_popup(&format!("{} Compound Splits (Review)", tag_name), &[
                &desc1, "new to corpus. Review each to verify.",
            ], Some(("Press Enter to review.", cta(Color::Yellow))), tc, hc)
        }
        InsightType::MissingAlbumSingle => detail_popup("Missing Album Singles", &[
            "Tracks with ARTIST and TITLE but no",
            "ALBUM tag, grouped by artist.",
        ], Some(("Press Enter to assign album values.", cta(Color::Cyan))), tc, hc),
        InsightType::DiscExtraction => detail_popup("Disc Extractions", &[
            "Disc values embedded in ALBUM or",
            "TRACKNUMBER tags (e.g., \"Album, Disc 2\"",
            "or track number \"A01\").",
        ], Some(("Extract to DISCNUMBER + clean source tag.", cta(Color::Cyan))), tc, hc),
        InsightType::PathTagMismatch => detail_popup("Filename Tag Schema Issues", &[
            "Files where tags derived from the",
            "filename path don't match embedded",
            "tag values.",
        ], None, tc, hc),
        InsightType::OtherSignal { index } => {
            if let Some(data) = data {
                if let Some(signal_entry) = data.bucket_other.entries.get(index) {
                    let mut lines = vec![
                        Line::from(Span::styled(signal_entry.display_label.clone(), Style::default().fg(hc).add_modifier(Modifier::BOLD))),
                        Line::from(""),
                        Line::from(Span::styled(format!("Count: {}", signal_entry.count), Style::default().fg(tc))),
                    ];
                    if let Some(affected) = signal_entry.affected_count {
                        lines.push(Line::from(Span::styled(format!("Affected tracks: {}", affected), Style::default().fg(tc))));
                    }
                    return lines;
                }
            }
            let mut lines = vec![
                Line::from(Span::styled(entry.label.clone(), Style::default().fg(hc).add_modifier(Modifier::BOLD))),
                Line::from(""),
            ];
            if let Some(count) = entry.count {
                lines.push(Line::from(Span::styled(format!("Count: {}", count), Style::default().fg(tc))));
            }
            lines
        }
    }
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
    /// Whether the Witch has operations in-flight (actions blocked).
    pub witch_busy: bool,
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
            witch_busy: false,
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

        self.witch_busy = busy;

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
        self.witch_busy
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
        state.witch_busy = false;
        let action = state.handle_input(&InputAction::Confirm);
        assert_eq!(action, InsightsAction::Launch);

        // Busy - Enter should be blocked
        state.witch_busy = true;
        let action = state.handle_input(&InputAction::Confirm);
        assert_eq!(action, InsightsAction::None);
    }

    #[test]
    fn test_update_witch_status() {
        let mut state = InsightsViewState::new();
        let no_handled = HashSet::new();

        // No status - should be Ready
        state.update(None, None, &no_handled, false);
        assert!(!state.witch_busy);

        // Pending > 0 - should be busy
        let busy_status = WorkStatus {
            pending: 5,
            ..Default::default()
        };
        state.update(Some(&busy_status), None, &no_handled, false);
        assert!(state.witch_busy);

        // Pending = 0 - should be ready again
        let idle_status = WorkStatus {
            pending: 0,
            ..Default::default()
        };
        state.update(Some(&idle_status), None, &no_handled, false);
        assert!(!state.witch_busy);
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
        state.witch_busy = true;

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
