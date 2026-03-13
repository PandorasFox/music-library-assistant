//! Shared types, constants, and classification helpers for release packing.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::corpus::tags::TagSet;
use crate::db::queries::external::{ExternalMatchRow, OptimalPackingScoreRow, PackingManifestRow};
use crate::meta::signals::data::PackingScoreBreakdown;

// ============================================================================
// Shared types
// ============================================================================

/// Corpus file metadata needed for scoring.
pub(super) struct CorpusFileInfo {
    pub parent_dir: String,
    pub tags: TagSet,
    pub duration_ms: Option<i64>,
}

/// A recording match for an inode, filtered for quality.
pub(super) struct RecordingMatch {
    pub recording_id: String,
    pub confidence: f64,
}

/// A candidate assignment of an inode to a (release, medium, track) slot.
pub(super) struct CandidateAssignment {
    pub inode: i64,
    pub recording_id: String,
    pub release_id: String,
    pub medium_pos: u32,
    pub track_pos: u32,
    pub medium_format: Option<String>,
    pub track_number: String,
    pub track_title: String,
    pub score: f64,
    pub breakdown: PackingScoreBreakdown,
}

// ============================================================================
// Proposal types (Stage 3 conflict resolution)
// ============================================================================

/// Quality tier for a release proposal. Determines which MIS round it enters.
/// Each tier is a separate pool — proposals enter exactly one pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProposalTier {
    /// Every slot filled, 1:1 dir↔release (per-medium for multi-medium), no leftover files.
    Perfect,
    /// Every slot filled, but cross-directory or directory has extra files.
    FullMatch,
    /// Some slots filled but not all (includes near-misses).
    Incomplete,
    /// Single-track release.
    Single,
}

impl ProposalTier {
    pub(super) fn as_str(&self) -> &'static str {
        match self {
            ProposalTier::Perfect => "perfect",
            ProposalTier::FullMatch => "full_match",
            ProposalTier::Incomplete => "incomplete",
            ProposalTier::Single => "single",
        }
    }
}

/// A release-level proposal: a complete assignment of inodes to track slots.
/// Proposals are the unit of selection in MIS rounds — they stay intact.
pub(crate) struct Proposal {
    pub total_tracks: i32,
    pub rows: Vec<OptimalPackingScoreRow>,
    pub inode_set: HashSet<i64>,
    pub total_score: f64,
    pub tier: ProposalTier,
}

/// Arc<Mutex<Option<Box<...>>>> wrapper that derives Clone + Debug + Serialize + Deserialize.
///
/// Clone is cheap (Arc refcount). Serialize/Deserialize skip the inner state
/// (these variants are transient pipeline state, never persisted).
#[derive(Clone)]
pub struct SharedMappingState(
    std::sync::Arc<std::sync::Mutex<Option<Box<ReleaseMappingState>>>>,
);

impl SharedMappingState {
    pub(crate) fn new(state: ReleaseMappingState) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(Some(Box::new(
            state,
        )))))
    }

    /// Take the state out of the box. Panics if called twice (state already consumed).
    pub(crate) fn take(&self) -> ReleaseMappingState {
        *self
            .0
            .lock()
            .unwrap()
            .take()
            .expect("ReleaseMappingState consumed twice")
    }
}

impl std::fmt::Debug for SharedMappingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedMappingState(..)")
    }
}

impl serde::Serialize for SharedMappingState {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_unit()
    }
}

impl<'de> serde::Deserialize<'de> for SharedMappingState {
    fn deserialize<D: serde::Deserializer<'de>>(
        _deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "SharedMappingState cannot be deserialized",
        ))
    }
}

/// A release that was dedup-removed because it has an identical inode signature
/// to the winning proposal (same set of corpus files, different pressing/edition).
#[derive(Debug, Clone)]
pub(crate) struct AlternativeRelease {
    pub release_id: String,
    pub release_title: String,
    pub release_artist: String,
    pub total_score: f64,
}

/// Data for a single connected component to be solved independently.
pub(crate) struct ComponentData {
    pub proposals: Vec<Arc<Proposal>>,
    pub tier: ProposalTier,
    pub corpus_paths: Arc<HashMap<i64, String>>,
    pub manifest_map: Arc<HashMap<String, (String, String, i32)>>,
    /// Per proposal index in `proposals`: dedup-removed siblings with identical inode signature.
    pub signature_siblings: HashMap<usize, Vec<AlternativeRelease>>,
}

/// Arc<Mutex<Option<Box<...>>>> wrapper for component data.
/// Same transient-pipeline-state pattern as SharedMappingState.
#[derive(Clone)]
pub struct SharedComponentData(
    std::sync::Arc<std::sync::Mutex<Option<Box<ComponentData>>>>,
);

impl SharedComponentData {
    pub(crate) fn new(data: ComponentData) -> Self {
        Self(std::sync::Arc::new(std::sync::Mutex::new(Some(Box::new(
            data,
        )))))
    }

    pub(crate) fn take(&self) -> ComponentData {
        *self
            .0
            .lock()
            .unwrap()
            .take()
            .expect("ComponentData consumed twice")
    }
}

impl std::fmt::Debug for SharedComponentData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedComponentData(..)")
    }
}

impl serde::Serialize for SharedComponentData {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_unit()
    }
}

impl<'de> serde::Deserialize<'de> for SharedComponentData {
    fn deserialize<D: serde::Deserializer<'de>>(
        _deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "SharedComponentData cannot be deserialized",
        ))
    }
}

/// Shared state passed between MIS round computations via boxed move semantics.
///
/// Each round takes ownership via `SharedMappingState::take()`, runs its MIS,
/// and packages the remaining state for the next round. Signal emission happens
/// per-component within each tier — no accumulation across rounds.
pub(crate) struct ReleaseMappingState {
    /// Proposal pools — each consumed by its corresponding tier orchestrator.
    pub perfect_pool: Vec<Arc<Proposal>>,
    pub full_match_pool: Vec<Arc<Proposal>>,
    pub incomplete_pool: Vec<Arc<Proposal>>,
    pub single_pool: Vec<Arc<Proposal>>,
    /// Knot extraction threshold: connected components where
    /// proposals/inodes >= this ratio are too tangled for MIS (many releases
    /// competing over few files). Extracted and resolved by best-scorer.
    pub knot_ratio: f64,
    /// Maximum component size before knot extraction kicks in regardless of ratio.
    pub knot_size_limit: usize,
    /// When true, run Singles before Incompletes in MIS ordering.
    /// Singles claim one inode each (no MIS needed), preventing single-file
    /// incompletes from competing in the expensive Incomplete MIS round.
    pub singles_before_incompletes: bool,
    /// When true, knots containing proposals that cover ALL contested inodes
    /// are reduced to only those covering proposals before greedy resolution.
    pub allow_resolve_knots_with_discographies: bool,
    /// Low-confidence downgrade: max AcoustID ratio threshold.
    pub low_confidence_max_acoustid_ratio: f64,
    /// Low-confidence downgrade: max average album_match threshold.
    pub low_confidence_max_album_match: f64,
}


/// Classify a proposal into a quality tier based on slot coverage and directory purity.
///
/// `media_count` is the number of MbMedium entries for this release (from the tracklist).
/// `inode_dir_map` maps each inode to its (parent_dir, dir_file_count).
pub(super) fn classify_proposal(
    rows: &[OptimalPackingScoreRow],
    total_tracks: i32,
    media_count: usize,
    inode_dir_map: &HashMap<i64, (String, i32)>,
) -> ProposalTier {
    if total_tracks <= 1 {
        return ProposalTier::Single;
    }
    if (rows.len() as i32) < total_tracks {
        return ProposalTier::Incomplete;
    }

    // All slots filled — check directory purity for Perfect vs FullMatch.
    //
    // Perfect requires:
    //   Single-medium: exactly 1 directory, dir_file_count == total_tracks
    //   Multi-medium: each medium's inodes from exactly 1 directory, each directory
    //     maps to exactly 1 medium, dir_file_count == medium track count,
    //     all directories are siblings (same parent)

    // Build dir → inodes and dir → media mapping
    let mut dir_inodes: HashMap<&str, Vec<i64>> = HashMap::new();
    let mut dir_media: HashMap<&str, HashSet<i32>> = HashMap::new();
    let mut medium_dirs: HashMap<i32, HashSet<&str>> = HashMap::new();

    for row in rows {
        if let Some((dir, _)) = inode_dir_map.get(&row.inode) {
            dir_inodes.entry(dir.as_str()).or_default().push(row.inode);
            dir_media
                .entry(dir.as_str())
                .or_default()
                .insert(row.medium_pos);
            medium_dirs
                .entry(row.medium_pos)
                .or_default()
                .insert(dir.as_str());
        }
    }

    if dir_inodes.is_empty() {
        return ProposalTier::FullMatch;
    }

    if media_count <= 1 {
        // Single-medium: Perfect iff exactly 1 directory, file count matches total tracks
        if dir_inodes.len() == 1 {
            let dir = *dir_inodes.keys().next().unwrap();
            let dir_file_count = inode_dir_map
                .values()
                .find(|(d, _)| d.as_str() == dir)
                .map(|(_, c)| *c)
                .unwrap_or(0);
            if dir_file_count == total_tracks {
                return ProposalTier::Perfect;
            }
        }
        return ProposalTier::FullMatch;
    }

    // Multi-medium: each medium must map to exactly 1 directory and vice versa
    for medium_dir_set in medium_dirs.values() {
        if medium_dir_set.len() != 1 {
            return ProposalTier::FullMatch;
        }
    }
    for dir_medium_set in dir_media.values() {
        if dir_medium_set.len() != 1 {
            return ProposalTier::FullMatch;
        }
    }

    // Check that each directory's file count matches its medium's track count
    // and all directories are siblings (same parent)
    let mut parents: HashSet<&str> = HashSet::new();
    for (dir, inodes) in &dir_inodes {
        let dir_file_count = inode_dir_map
            .values()
            .find(|(d, _)| d.as_str() == *dir)
            .map(|(_, c)| *c)
            .unwrap_or(0);
        if dir_file_count != inodes.len() as i32 {
            return ProposalTier::FullMatch;
        }
        if let Some(parent) = Path::new(dir).parent() {
            parents.insert(parent.to_str().unwrap_or(""));
        }
    }
    if parents.len() > 1 {
        return ProposalTier::FullMatch;
    }

    ProposalTier::Perfect
}

// ============================================================================
// Shared helpers
// ============================================================================

/// Build a manifest lookup map: `release_id → (title, artist, total_tracks)`.
///
/// This pattern appears in many packing stages — centralized here.
pub(super) fn build_manifest_map(
    manifest: &[PackingManifestRow],
) -> HashMap<&str, (&str, &str, i32)> {
    manifest
        .iter()
        .map(|r| {
            (
                r.release_id.as_str(),
                (
                    r.release_title.as_str(),
                    r.release_artist.as_str(),
                    r.total_tracks,
                ),
            )
        })
        .collect()
}

/// Build an owned manifest map: `release_id → (title, artist, total_tracks)`.
///
/// Used when the map must outlive the manifest slice (e.g. packaged into component data).
pub(super) fn build_manifest_map_owned(
    manifest: &[PackingManifestRow],
) -> HashMap<String, (String, String, i32)> {
    manifest
        .iter()
        .map(|r| {
            (
                r.release_id.clone(),
                (
                    r.release_title.clone(),
                    r.release_artist.clone(),
                    r.total_tracks,
                ),
            )
        })
        .collect()
}

/// Group external match rows by inode, keeping all recordings per inode.
pub(super) fn group_all_per_inode(
    rows: Vec<ExternalMatchRow>,
) -> HashMap<i64, Vec<ExternalMatchRow>> {
    let mut grouped: HashMap<i64, Vec<ExternalMatchRow>> = HashMap::new();
    for row in rows {
        grouped.entry(row.inode).or_default().push(row);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_row(inode: i64, medium: i32, track: i32) -> OptimalPackingScoreRow {
        OptimalPackingScoreRow {
            release_id: "rel-1".to_string(),
            inode,
            recording_id: format!("rec-{}", inode),
            medium_pos: medium,
            track_pos: track,
            track_title: format!("Track {}", track),
            medium_format: Some("CD".to_string()),
            track_number: track.to_string(),
            score: 0.9,
            score_breakdown: Vec::new(),
            match_method: 0,
            fingerprint_hex: None,
            raw_duration_ms: None,
        }
    }

    fn dir_map(pairs: &[(i64, &str, i32)]) -> HashMap<i64, (String, i32)> {
        pairs
            .iter()
            .map(|(inode, dir, count)| (*inode, (dir.to_string(), *count)))
            .collect()
    }

    #[test]
    fn test_classify_perfect() {
        // Single medium, all slots filled, 1 dir, dir file count == total tracks
        let rows = vec![
            make_row(100, 1, 1),
            make_row(200, 1, 2),
            make_row(300, 1, 3),
        ];
        let inode_dirs = dir_map(&[
            (100, "/music/album", 3),
            (200, "/music/album", 3),
            (300, "/music/album", 3),
        ]);
        assert_eq!(classify_proposal(&rows, 3, 1, &inode_dirs), ProposalTier::Perfect);
    }

    #[test]
    fn test_classify_full_match() {
        // All slots filled, but dir has extra files (count=5 > total_tracks=3)
        let rows = vec![
            make_row(100, 1, 1),
            make_row(200, 1, 2),
            make_row(300, 1, 3),
        ];
        let inode_dirs = dir_map(&[
            (100, "/music/album", 5),
            (200, "/music/album", 5),
            (300, "/music/album", 5),
        ]);
        assert_eq!(classify_proposal(&rows, 3, 1, &inode_dirs), ProposalTier::FullMatch);
    }

    #[test]
    fn test_classify_incomplete() {
        // Only 2 of 3 slots filled
        let rows = vec![
            make_row(100, 1, 1),
            make_row(200, 1, 2),
        ];
        let inode_dirs = dir_map(&[
            (100, "/music/album", 3),
            (200, "/music/album", 3),
        ]);
        assert_eq!(classify_proposal(&rows, 3, 1, &inode_dirs), ProposalTier::Incomplete);
    }

    #[test]
    fn test_classify_single() {
        let rows = vec![make_row(100, 1, 1)];
        let inode_dirs = dir_map(&[(100, "/music/album", 1)]);
        assert_eq!(classify_proposal(&rows, 1, 1, &inode_dirs), ProposalTier::Single);
    }

    #[test]
    fn test_classify_multi_medium_perfect() {
        // Two media, two sibling dirs, each with correct file counts
        let rows = vec![
            make_row(100, 1, 1),
            make_row(200, 1, 2),
            make_row(300, 2, 1),
            make_row(400, 2, 2),
        ];
        let inode_dirs = dir_map(&[
            (100, "/music/boxset/disc1", 2),
            (200, "/music/boxset/disc1", 2),
            (300, "/music/boxset/disc2", 2),
            (400, "/music/boxset/disc2", 2),
        ]);
        assert_eq!(classify_proposal(&rows, 4, 2, &inode_dirs), ProposalTier::Perfect);
    }

    #[test]
    fn test_classify_multi_medium_cross_dir() {
        // Two media, but inodes from same directory → FullMatch
        let rows = vec![
            make_row(100, 1, 1),
            make_row(200, 2, 1),
        ];
        let inode_dirs = dir_map(&[
            (100, "/music/album", 2),
            (200, "/music/album", 2),
        ]);
        assert_eq!(classify_proposal(&rows, 2, 2, &inode_dirs), ProposalTier::FullMatch);
    }
}
