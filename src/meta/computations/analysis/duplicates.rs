//! Duplicate detection executors.
//!
//! Fingerprint overlaps, duplicate inodes, metadata duplicates, fingerprint
//! analysis, and cross-source overlap detection.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use mm_utils::tag_names::find_tag_in_map;

use crate::logging::log_general;
use crate::meta::computations::helpers::{
    parse_inodes_csv, reconcile_aggregate_signals, reconcile_corpus_signals,
    ComputedAggregateSignal, ComputedCorpusSignal,
};
use crate::meta::computations::types::ComputationWitness;
use crate::db::types::Zone;
use crate::meta::signals::data::{
    TypedSignalWrite, FingerprintOverlapSignal, MetadataDuplicateSignal, MetadataDuplicateData,
    DuplicateInodeSignal, SubparDuplicateSignal, SubparDuplicateData,
    RedundantDuplicateSignal, RedundantDuplicateData,
    CrossSourceOverlapSignal, CrossSourceOverlapData, CrossSourceTrackPair,
};
use crate::db::ReadOnlyDb;
use crate::db::write_thread;

use super::{Computation, Result};

// ============================================================================
// Fingerprint Duplicate Detection
// ============================================================================

/// Simple union-find for grouping similar fingerprints.
struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]);
        }
        self.parent[x]
    }

    fn union(&mut self, x: usize, y: usize) {
        let rx = self.find(x);
        let ry = self.find(y);
        if rx == ry {
            return;
        }
        if self.rank[rx] < self.rank[ry] {
            self.parent[rx] = ry;
        } else if self.rank[rx] > self.rank[ry] {
            self.parent[ry] = rx;
        } else {
            self.parent[ry] = rx;
            self.rank[rx] += 1;
        }
    }
}

/// Execute DetectFingerprintOverlaps - bulk detection of fingerprint overlaps
/// using similarity-based grouping rather than exact byte matching.
pub fn execute_detect_fingerprint_overlaps(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::db::queries::files::fingerprint_to_text;

    let computation = Computation::DetectFingerprintOverlaps;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Load config for similarity threshold and duration tolerance
    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to load config: {}", e),
            );
        }
    };

    let similarity_threshold = config.opinions.duplicate_analysis.fingerprint_similarity_threshold;
    let duration_tolerance_ms = config.opinions.duplicate_analysis.duration_tolerance_ms;

    // Get all corpus audio files with fingerprints and durations
    let all_audio = match read_only_db.get_all_audio_files(Zone::Corpus) {
        Ok(files) => files,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query audio files: {}", e),
            );
        }
    };

    // Filter to files that have both a fingerprint and a duration
    let fingerprinted: Vec<_> = all_audio
        .into_iter()
        .filter(|af| af.audio.fingerprint.is_some() && af.audio.duration_ms.is_some())
        .collect();

    if fingerprinted.is_empty() {
        log_general("[COMPUTE] DetectFingerprintOverlaps: no fingerprinted files");
        // Reconcile with empty set to clear any stale signals
        let (cleared, new_count, updated, unchanged) =
            reconcile_aggregate_signals::<FingerprintOverlapSignal>(
                read_only_db,
                &sender,
                Vec::new(),
                witness,
            );
        log_general(format!(
            "[COMPUTE] DetectFingerprintOverlaps: 0 groups, cleared={}, new={}, updated={}, unchanged={}",
            cleared, new_count, updated, unchanged
        ));
        write_thread::wait_for_queue_drain();
        return Result::success(
            computation,
            start.elapsed().as_millis() as u64,
            vec![
                Computation::AnalyzeFingerprintOverlaps,
                Computation::DetectCrossSourceOverlaps,
            ],
        );
    }

    // Bucket by duration to reduce pairwise comparisons.
    // cluster_by_duration returns Vec<Vec<&AudioFile>>; we need indices into `fingerprinted`.
    // Build index-aware duration buckets manually.
    let mut indexed: Vec<(usize, i64)> = fingerprinted
        .iter()
        .enumerate()
        .map(|(i, af)| (i, af.audio.duration_ms.unwrap_or(0)))
        .collect();
    indexed.sort_by_key(|&(_, dur)| dur);

    let mut buckets: Vec<Vec<usize>> = Vec::new();
    let mut current_bucket: Vec<usize> = vec![indexed[0].0];
    let mut bucket_start_duration = indexed[0].1;

    for &(idx, duration) in indexed.iter().skip(1) {
        if (duration - bucket_start_duration).abs() <= duration_tolerance_ms {
            current_bucket.push(idx);
        } else {
            buckets.push(current_bucket);
            current_bucket = vec![idx];
            bucket_start_duration = duration;
        }
    }
    buckets.push(current_bucket);

    // Union-find across all fingerprinted files
    let mut uf = UnionFind::new(fingerprinted.len());

    for bucket in &buckets {
        if bucket.len() < 2 {
            continue;
        }
        // Pairwise similarity within this duration bucket
        for i in 0..bucket.len() {
            let idx_i = bucket[i];
            let fp_i = fingerprinted[idx_i].audio.fingerprint.as_ref().unwrap();
            for j in (i + 1)..bucket.len() {
                let idx_j = bucket[j];
                let fp_j = fingerprinted[idx_j].audio.fingerprint.as_ref().unwrap();

                let similarity = fingerprint_similarity(fp_i, fp_j);
                if similarity >= similarity_threshold {
                    uf.union(idx_i, idx_j);
                }
            }
        }
    }

    // Collect connected components from union-find
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..fingerprinted.len() {
        let root = uf.find(i);
        groups.entry(root).or_default().push(i);
    }

    // Build computed signals for groups of 2+
    let mut computed = Vec::new();
    let mut total_tracks = 0;

    for (_, members) in &groups {
        if members.len() < 2 {
            continue;
        }

        // Canonical key: fingerprint text of the lowest-inode member
        let min_inode_idx = *members
            .iter()
            .min_by_key(|&&idx| fingerprinted[idx].inode())
            .unwrap();
        let canonical_fp = fingerprinted[min_inode_idx]
            .audio
            .fingerprint
            .as_ref()
            .unwrap();
        let fingerprint_text = fingerprint_to_text(canonical_fp);

        let inodes: Vec<i64> = members
            .iter()
            .map(|&idx| fingerprinted[idx].inode())
            .collect();
        total_tracks += inodes.len();

        computed.push(ComputedAggregateSignal::new(
            fingerprint_text.clone(),
            TypedSignalWrite::FingerprintOverlap(FingerprintOverlapSignal {
                key: fingerprint_text,
                inodes,
            }),
        ));
    }

    // Reconcile with existing signals (handles stale/new/changed/unchanged)
    let (cleared, new_count, updated, unchanged) = reconcile_aggregate_signals::<FingerprintOverlapSignal>(
        read_only_db,
        &sender,
        computed,
        witness,
    );

    let total_groups = new_count + updated + unchanged;
    log_general(format!(
        "[COMPUTE] DetectFingerprintOverlaps: {} groups ({} tracks), cleared={}, new={}, updated={}, unchanged={}",
        total_groups, total_tracks, cleared, new_count, updated, unchanged
    ));

    // Wait for all FingerprintOverlap signals to be written before spawning
    // dependent computations. This ensures AnalyzeFingerprintOverlaps and
    // ClusterDirectoryOverlaps see the fresh signal data.
    write_thread::wait_for_queue_drain();

    // Spawn dependent computations that read FingerprintOverlap signals
    Result::success(
        computation,
        start.elapsed().as_millis() as u64,
        vec![
            Computation::AnalyzeFingerprintOverlaps,
            Computation::DetectCrossSourceOverlaps,
        ],
    )
}

// ============================================================================
// Duplicate Inode Detection
// ============================================================================

/// Execute DetectDuplicateInodes - bulk detection of duplicate inodes.
pub fn execute_detect_duplicate_inodes(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectDuplicateInodes;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    let duplicate_groups = match read_only_db.get_duplicate_inode_groups() {
        Ok(groups) => groups,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query duplicate inodes: {}", e),
            );
        }
    };

    // Build computed signals
    let mut computed = Vec::new();

    for (inode, inodes_str) in duplicate_groups {
        let inodes = parse_inodes_csv(&inodes_str);

        computed.push(ComputedAggregateSignal::new(
            inode.to_string(),
            TypedSignalWrite::DuplicateInode(DuplicateInodeSignal {
                key: inode.to_string(),
                inode,
                inodes,
            }),
        ));
    }

    // Reconcile with existing signals (handles stale/new/changed/unchanged)
    let (cleared, new_count, updated, unchanged) = reconcile_aggregate_signals::<DuplicateInodeSignal>(
        read_only_db,
        &sender,
        computed,
        witness,
    );

    let total_groups = new_count + updated + unchanged;
    log_general(format!(
        "[COMPUTE] DetectDuplicateInodes: {} groups, cleared={}, new={}, updated={}, unchanged={}",
        total_groups, cleared, new_count, updated, unchanged
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Metadata Duplicate Detection
// ============================================================================

/// Execute DetectMetadataDuplicates - detect exact metadata duplicates.
pub fn execute_detect_metadata_duplicates(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectMetadataDuplicates;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    let all_tags = match read_only_db.get_all_tags_ordered() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query tags: {}", e),
            );
        }
    };

    let mut inode_tags: HashMap<i64, Vec<(String, String)>> = HashMap::new();

    for (inode, tag_name, tag_value) in all_tags {
        let upper = tag_name.to_uppercase();
        // Encoder tags are noise — they describe the tool, not the content.
        if upper == "ENCODER"
            || upper == "ENCODED_BY"
            || upper == "ENCODER_SETTINGS"
            || upper == "ENCODER_OPTIONS"
            || upper == "ENCODING"
            || upper == "ENCODING_TOOL"
            || upper == "ENCODERSOFTWARE"
        {
            continue;
        }
        inode_tags
            .entry(inode)
            .or_default()
            .push((upper, tag_value));
    }

    let mut sig_to_inodes: HashMap<String, Vec<i64>> = HashMap::new();

    for (inode, mut tags) in inode_tags {
        tags.sort_by(|a, b| a.0.cmp(&b.0));
        let signature: String = tags
            .iter()
            .map(|(name, value)| format!("{}={}", name, value))
            .collect::<Vec<_>>()
            .join("|");

        sig_to_inodes
            .entry(signature)
            .or_default()
            .push(inode);
    }

    let mut computed = Vec::new();

    for (signature, inodes) in sig_to_inodes {
        if inodes.len() < 2 {
            continue;
        }

        let key_hash = format!("{:x}", md5_hash(&signature));

        computed.push(ComputedAggregateSignal::new(
            key_hash.clone(),
            TypedSignalWrite::MetadataDuplicate(MetadataDuplicateSignal {
                key: key_hash,
                data: MetadataDuplicateData {
                    tag_signature: signature,
                    inodes,
                },
            }),
        ));
    }

    let (cleared, new_count, updated, unchanged) =
        reconcile_aggregate_signals::<MetadataDuplicateSignal>(
            read_only_db,
            &sender,
            computed,
            witness,
        );

    let total_groups = new_count + updated + unchanged;
    log_general(format!(
        "[COMPUTE] DetectMetadataDuplicates: {} duplicate metadata groups, cleared={}, new={}, updated={}, unchanged={}",
        total_groups, cleared, new_count, updated, unchanged
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Simple hash function for signature strings.
pub fn md5_hash(s: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

// ============================================================================
// Fingerprint Duplicate Analysis
// ============================================================================

/// Quality tier for audio format classification.
/// Higher value = better format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FormatClass {
    /// Lossy non-Vorbis (MP3, M4A, AAC, WMA)
    OtherLossy = 0,
    /// Lossless non-Vorbis (WAV, AIFF, APE, WV)
    OtherLossless = 1,
    /// Vorbis lossy (Opus, OGG)
    VorbisLossy = 2,
    /// Vorbis lossless (FLAC)
    VorbisLossless = 3,
}

/// Classify a file type into its format class.
pub(crate) fn classify_format(file_type: &str) -> FormatClass {
    match file_type.to_lowercase().as_str() {
        "flac" => FormatClass::VorbisLossless,
        "opus" | "ogg" => FormatClass::VorbisLossy,
        "wav" | "aiff" | "aif" | "ape" | "wv" => FormatClass::OtherLossless,
        _ => FormatClass::OtherLossy, // mp3, m4a, aac, wma, etc.
    }
}

/// Equivalence key for grouping files into quality tiers.
///
/// Files with the same key are considered equivalent quality.
/// Bitrate is the primary metric for ALL formats (lossy and lossless alike),
/// since sample rate rarely varies (almost always 44.1 or 48 kHz) while
/// bitrate reliably distinguishes true lossless from lossy transcodes
/// packaged in lossless containers (e.g. `.LOSSY.flac`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QualityTier {
    pub(crate) format_class: FormatClass,
    /// Bitrate in kbps — primary quality metric for all formats.
    pub(crate) bitrate: i32,
    /// Sample rate in Hz — secondary tiebreaker.
    pub(crate) sample_rate: i32,
}

impl Ord for QualityTier {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.format_class
            .cmp(&other.format_class)
            .then(self.bitrate.cmp(&other.bitrate))
            .then(self.sample_rate.cmp(&other.sample_rate))
    }
}

impl PartialOrd for QualityTier {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Build a QualityTier for a single audio file.
pub(crate) fn quality_tier_of(file_type: &str, bitrate_kbps: Option<i32>, sample_rate: Option<i32>) -> QualityTier {
    QualityTier {
        format_class: classify_format(file_type),
        bitrate: bitrate_kbps.unwrap_or(0),
        sample_rate: sample_rate.unwrap_or(0),
    }
}

/// Determine the SubparReason when `inferior` is outranked by `superior`.
fn subpar_reason_between(inferior: &QualityTier, superior: &QualityTier) -> SubparReason {
    if inferior.format_class < superior.format_class {
        SubparReason::SubparFormat
    } else if inferior.bitrate < superior.bitrate {
        SubparReason::SubparBitrate
    } else {
        SubparReason::SubparSampleRate
    }
}

/// Compute fingerprint similarity using bit-level Hamming distance.
/// Returns similarity as percentage (0.0 - 100.0).
///
/// Chromaprint fingerprints are Vec<u32> where each u32 encodes 32 bits of spectral features.
/// We use XOR + popcount to count differing bits, then compute similarity.
pub fn fingerprint_similarity(fp1: &[u32], fp2: &[u32]) -> f64 {
    if fp1.is_empty() || fp2.is_empty() {
        return 0.0;
    }

    // Use shorter as reference length
    let min_len = fp1.len().min(fp2.len());
    let max_len = fp1.len().max(fp2.len());

    // If lengths differ significantly, they're likely different recordings
    if max_len > min_len * 2 {
        return 0.0;
    }

    // Compare overlapping portions, find best alignment
    // For simplicity, we compare the overlapping portion without sliding
    // (sliding would be O(n^2) and chromaprint handles alignment internally)
    let total_bits = (min_len * 32) as u64;
    let mut matching_bits = 0u64;

    for i in 0..min_len {
        let xor = fp1[i] ^ fp2[i];
        let differing = xor.count_ones() as u64;
        matching_bits += 32 - differing;
    }

    (matching_bits as f64 / total_bits as f64) * 100.0
}

/// Normalize album name for comparison.
/// Strips edition suffixes, normalizes case and whitespace.
fn normalize_album_name(s: &str) -> String {
    // Common suffixes to strip
    let suffixes = [
        "(deluxe edition)",
        "(deluxe)",
        "[deluxe edition]",
        "[deluxe]",
        "(remastered)",
        "[remastered]",
        "(remaster)",
        "[remaster]",
        "(expanded edition)",
        "[expanded edition]",
        "(special edition)",
        "[special edition]",
        "(anniversary edition)",
        "[anniversary edition]",
        "(bonus track version)",
        "[bonus track version]",
    ];

    let mut normalized = s.to_lowercase();

    for suffix in &suffixes {
        if let Some(pos) = normalized.find(suffix) {
            normalized = normalized[..pos].to_string();
        }
    }

    // Normalize whitespace
    normalized
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

/// Variant keywords that indicate different versions of a track.
const VARIANT_KEYWORDS: &[&str] = &[
    "remix",
    "live",
    "acoustic",
    "remaster",
    "remastered",
    "demo",
    "edit",
    "instrumental",
    "radio edit",
    "extended",
    "alternate",
    "alt",
    "bonus",
    "unplugged",
    "orchestral",
    "piano version",
    "stripped",
];

/// Check if a title contains any variant keyword.
fn has_any_variant_keyword(title: &str) -> bool {
    VARIANT_KEYWORDS.iter().any(|&kw| title.contains(kw))
}

/// Release identity information for variant detection.
#[derive(Debug)]
struct TrackReleaseIdentity {
    inode: i64,
    path: String,
    album: String,
    title: String,
    isrc: String,
    catalog_number: String,
}

/// Check if two tracks are from the same release (true duplicates, not variants).
///
/// Tracks are "same release" if:
/// - Same catalog number (compilation albums with same ISRC but different catalog = different releases), OR
/// - No catalog numbers + same album + ISRC match, OR
/// - Same normalized album AND same normalized title AND no variant keyword mismatch
///
/// IMPORTANT: Catalog number is checked BEFORE ISRC because the same recording (same ISRC)
/// can appear on multiple compilation albums with different catalog numbers. These are
/// legitimate variants that should be kept, not flagged as duplicates.
///
/// IMPORTANT: When no catalog numbers are present, album is checked BEFORE ISRC because
/// ISRC identifies the recording, not the release — same ISRC on different albums (e.g.
/// solo release vs compilation) means different releases, not redundant copies.
fn is_same_release(a: &TrackReleaseIdentity, b: &TrackReleaseIdentity, elide_variants: bool) -> bool {
    // First: Check catalog numbers - different catalog = different release
    // This catches the compilation album case where the same recording (same ISRC)
    // appears on different albums with different catalog numbers.
    if !a.catalog_number.is_empty() && !b.catalog_number.is_empty() {
        if !a.catalog_number.eq_ignore_ascii_case(&b.catalog_number) {
            return false; // Different catalog numbers = different releases (compilation variant)
        }
        return true; // Same catalog number = same release
    }

    // No catalog numbers to differentiate — check album names first.
    // Different albums = different releases, even with matching ISRC.
    let album_a = normalize_album_name(&a.album);
    let album_b = normalize_album_name(&b.album);

    if album_a != album_b {
        return false; // Different albums = different releases (re-release / compilation)
    }

    // Same album — ISRC match confirms same release
    if !a.isrc.is_empty() && !b.isrc.is_empty() && a.isrc.eq_ignore_ascii_case(&b.isrc) {
        return true;
    }

    // Variant title elision: if either title contains a variant keyword (remix, live,
    // instrumental, etc.) and the titles aren't the same, treat as different variants.
    // This avoids false matches between e.g. "Track (Remix)" and "Track (Remix) -Instrumental-"
    // where both contain "remix" but are clearly different versions.
    if elide_variants {
        let title_a = a.title.to_lowercase();
        let title_b = b.title.to_lowercase();

        if title_a != title_b {
            let has_variant_a = has_any_variant_keyword(&title_a);
            let has_variant_b = has_any_variant_keyword(&title_b);

            if has_variant_a || has_variant_b {
                return false; // Different titles + variant keyword = different versions
            }
        }
    }

    true
}

/// Reason why a track is subpar.
#[derive(Debug, Clone, Copy)]
enum SubparReason {
    SubparFormat,
    SubparBitrate,
    SubparSampleRate,
}

impl SubparReason {
    fn as_str(&self) -> &'static str {
        match self {
            Self::SubparFormat => "SubparFormat",
            Self::SubparBitrate => "SubparBitrate",
            Self::SubparSampleRate => "SubparSampleRate",
        }
    }
}

/// Execute AnalyzeFingerprintOverlaps - deep analysis of fingerprint overlap groups.
pub fn execute_analyze_fingerprint_overlaps(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::AnalyzeFingerprintOverlaps;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Load configuration
    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to load config: {}", e),
            );
        }
    };

    let similarity_threshold = config.opinions.duplicate_analysis.fingerprint_similarity_threshold;
    let duration_tolerance_ms = config.opinions.duplicate_analysis.duration_tolerance_ms;
    let elide_variant_titles = config.opinions.duplicate_analysis.elide_variant_titles;

    // Get all FingerprintOverlap signals
    let fp_dup_signals = read_only_db
        .get_fingerprint_overlap_signals()
        .unwrap_or_default();

    if fp_dup_signals.is_empty() {
        log_general("[COMPUTE] AnalyzeFingerprintOverlaps: no fingerprint overlap signals to analyze");
        // Reconcile with empty sets to clear any stale signals
        let (subpar_cleared, _, _, _) =
            reconcile_corpus_signals::<SubparDuplicateSignal>(read_only_db, &sender, Vec::new(), witness);
        let (redundant_cleared, _, _, _) =
            reconcile_aggregate_signals::<RedundantDuplicateSignal>(read_only_db, &sender, Vec::new(), witness);
        if subpar_cleared > 0 || redundant_cleared > 0 {
            log_general(format!(
                "[COMPUTE] AnalyzeFingerprintOverlaps: cleared {} stale SubparDuplicate + {} stale RedundantDuplicate",
                subpar_cleared, redundant_cleared
            ));
        }
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    let mut total_groups = 0;
    let mut variant_skipped = 0;
    let mut expected_skipped = 0;
    let mut interior_skipped = 0;

    // Collect all computed signals for reconciliation
    let mut computed_subpar: Vec<ComputedCorpusSignal> = Vec::new();
    let mut computed_redundant: Vec<ComputedAggregateSignal> = Vec::new();

    for signal in &fp_dup_signals {
        let inodes = signal.inodes.clone();
        if inodes.len() < 2 {
            continue;
        }

        // Skip groups marked as expected duplicates by the operator
        if read_only_db.is_expected_duplicate(&signal.key).unwrap_or(false) {
            expected_skipped += 1;
            continue;
        }

        total_groups += 1;

        // Get corpus audio files for this group
        let audio_files = match read_only_db.get_audio_files_by_inodes(&inodes, Zone::Corpus) {
            Ok(af) => af,
            Err(_) => continue,
        };

        if audio_files.len() < 2 {
            continue;
        }

        // Cluster by duration (files with similar duration are more likely true duplicates)
        let duration_clusters = cluster_by_duration(&audio_files, duration_tolerance_ms);

        for cluster in duration_clusters {
            if cluster.len() < 2 {
                continue;
            }

            // Get release identity info for each audio file
            let mut identities: Vec<TrackReleaseIdentity> = Vec::new();
            for audio_file in &cluster {
                let inode = audio_file.inode();
                let tags = read_only_db.get_corpus_tags(inode).unwrap_or_default();
                let tag_map: HashMap<String, String> = tags
                    .into_iter()
                    .map(|t| (t.tag_name.to_uppercase(), t.tag_value))
                    .collect();

                identities.push(TrackReleaseIdentity {
                    inode,
                    path: audio_file.path().to_string(),
                    album: tag_map.get("ALBUM").cloned().unwrap_or_default(),
                    title: tag_map.get("TITLE").cloned().unwrap_or_default(),
                    isrc: tag_map.get("ISRC").cloned().unwrap_or_default(),
                    // Use fuzzy lookup for catalog number - handles "catalog_number" vs "catalognumber"
                    catalog_number: find_tag_in_map(&tag_map, "catalognumber")
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                });
            }

            // Check fingerprint similarity between all pairs
            // Build groups of "true duplicates" (high similarity + same release)
            let mut true_duplicate_groups: Vec<Vec<usize>> = Vec::new();
            let mut assigned: Vec<bool> = vec![false; cluster.len()];

            for i in 0..cluster.len() {
                if assigned[i] {
                    continue;
                }

                let mut group = vec![i];
                assigned[i] = true;

                for j in (i + 1)..cluster.len() {
                    if assigned[j] {
                        continue;
                    }

                    // Check fingerprint similarity
                    let fp_i = cluster[i].audio.fingerprint.as_ref();
                    let fp_j = cluster[j].audio.fingerprint.as_ref();

                    if let (Some(fp1), Some(fp2)) = (fp_i, fp_j) {
                        let similarity = fingerprint_similarity(fp1, fp2);
                        if similarity < similarity_threshold {
                            continue; // Not similar enough
                        }
                    }

                    // Check if same release
                    if !is_same_release(&identities[i], &identities[j], elide_variant_titles) {
                        variant_skipped += 1;
                        continue; // Different variants
                    }

                    // This is a true duplicate of the group leader
                    group.push(j);
                    assigned[j] = true;
                }

                if group.len() >= 2 {
                    true_duplicate_groups.push(group);
                }
            }

            // For each true duplicate group, partition into quality tiers and collect signals
            for group in true_duplicate_groups {
                // Check if all files in this group are interior to a single source
                // that has interior_dupes disabled
                let group_sources: HashSet<Option<PathBuf>> = group.iter()
                    .map(|&idx| {
                        let path = cluster[idx].path();
                        let relative = path.strip_prefix("corpus/").unwrap_or(path);
                        config.resolve_source_config(Path::new(relative))
                            .map(|r| r.source_path)
                    })
                    .collect();

                if group_sources.len() == 1 {
                    if let Some(Some(source_path)) = group_sources.iter().next() {
                        if let Some(resolved) = config.resolve_source_config(source_path) {
                            if !resolved.interior_dupes {
                                interior_skipped += 1;
                                continue;
                            }
                        }
                    }
                }

                // Build quality tiers for each file in the group
                let mut tiered: Vec<(usize, QualityTier)> = group
                    .iter()
                    .map(|&idx| {
                        let af = &cluster[idx];
                        let tier = quality_tier_of(
                            &af.audio.file_type,
                            af.audio.bitrate_kbps,
                            af.audio.sample_rate,
                        );
                        (idx, tier)
                    })
                    .collect();

                // Sort by tier descending (best first)
                tiered.sort_by(|a, b| b.1.cmp(&a.1));

                let best_tier = tiered[0].1;

                // Partition: best-tier files vs lower-tier files
                let best_indices: Vec<usize> = tiered.iter()
                    .filter(|(_, t)| *t == best_tier)
                    .map(|(idx, _)| *idx)
                    .collect();

                let lower: Vec<(usize, QualityTier)> = tiered.iter()
                    .filter(|(_, t)| *t != best_tier)
                    .copied()
                    .collect();

                // Best-tier files with >1 member -> RedundantDuplicate
                if best_indices.len() > 1 {
                    let inodes: Vec<i64> = best_indices.iter().map(|&idx| cluster[idx].inode()).collect();
                    let paths: Vec<String> = best_indices.iter().map(|&idx| cluster[idx].path().to_string()).collect();
                    let file_type = cluster[best_indices[0]].audio.file_type.clone();

                    computed_redundant.push(ComputedAggregateSignal::new(
                        signal.key.clone(),
                        TypedSignalWrite::RedundantDuplicate(RedundantDuplicateSignal {
                            key: signal.key.clone(),
                            data: RedundantDuplicateData {
                                file_type,
                                inodes,
                                paths,
                            },
                        }),
                    ));
                }

                // Lower-tier files -> SubparDuplicate (reference: first best-tier file)
                if !lower.is_empty() {
                    let superior_idx = best_indices[0];
                    let superior_identity = &identities[superior_idx];
                    let superior_fp = cluster[superior_idx].audio.fingerprint.as_ref();

                    for (idx, tier) in &lower {
                        let audio_file = &cluster[*idx];
                        let reason = subpar_reason_between(tier, &best_tier);

                        // Compute similarity score between subpar and superior
                        let similarity_score = match (audio_file.audio.fingerprint.as_ref(), superior_fp) {
                            (Some(fp_sub), Some(fp_sup)) => fingerprint_similarity(fp_sub, fp_sup),
                            _ => 0.0,
                        };

                        let inode = audio_file.inode();
                        computed_subpar.push(ComputedCorpusSignal::new(
                            inode,
                            TypedSignalWrite::SubparDuplicate(SubparDuplicateSignal {
                                inode,
                                path: audio_file.path().to_string(),
                                data: SubparDuplicateData {
                                    reason: reason.as_str().to_string(),
                                    superior_inode: superior_identity.inode,
                                    superior_path: superior_identity.path.clone(),
                                    dupe_group_fingerprint: signal.key.clone(),
                                    similarity_score,
                                },
                            }),
                        ));
                    }
                }
            }
        }
    }

    // Reconcile SubparDuplicate (corpus signals, keyed by inode)
    let (subpar_cleared, subpar_new, subpar_updated, subpar_unchanged) =
        reconcile_corpus_signals::<SubparDuplicateSignal>(read_only_db, &sender, computed_subpar, witness);

    // Reconcile RedundantDuplicate (aggregate signals, keyed by fingerprint key)
    let (redundant_cleared, redundant_new, redundant_updated, redundant_unchanged) =
        reconcile_aggregate_signals::<RedundantDuplicateSignal>(read_only_db, &sender, computed_redundant, witness);

    let subpar_total = subpar_new + subpar_updated + subpar_unchanged;
    let redundant_total = redundant_new + redundant_updated + redundant_unchanged;
    log_general(format!(
        "[COMPUTE] AnalyzeFingerprintOverlaps: analyzed {} groups, {} SubparDuplicate (cleared={}, new={}, updated={}, unchanged={}) + {} RedundantDuplicate (cleared={}, new={}, updated={}, unchanged={}), skipped {} variants, {} expected, {} interior-suppressed",
        total_groups,
        subpar_total, subpar_cleared, subpar_new, subpar_updated, subpar_unchanged,
        redundant_total, redundant_cleared, redundant_new, redundant_updated, redundant_unchanged,
        variant_skipped, expected_skipped, interior_skipped
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Cluster audio files by duration within tolerance.
fn cluster_by_duration(
    audio_files: &[crate::db::types::AudioFile],
    tolerance_ms: i64,
) -> Vec<Vec<&crate::db::types::AudioFile>> {
    if audio_files.is_empty() {
        return Vec::new();
    }

    // Sort by duration
    let mut sorted: Vec<_> = audio_files.iter().collect();
    sorted.sort_by_key(|af| af.audio.duration_ms.unwrap_or(0));

    let mut clusters: Vec<Vec<&crate::db::types::AudioFile>> = Vec::new();
    let mut current_cluster: Vec<&crate::db::types::AudioFile> = vec![sorted[0]];
    let mut cluster_start_duration = sorted[0].audio.duration_ms.unwrap_or(0);

    for audio_file in sorted.iter().skip(1) {
        let duration = audio_file.audio.duration_ms.unwrap_or(0);

        // If within tolerance of cluster start, add to cluster
        if (duration - cluster_start_duration).abs() <= tolerance_ms {
            current_cluster.push(audio_file);
        } else {
            // Start new cluster
            if !current_cluster.is_empty() {
                clusters.push(current_cluster);
            }
            current_cluster = vec![audio_file];
            cluster_start_duration = duration;
        }
    }

    // Don't forget the last cluster
    if !current_cluster.is_empty() {
        clusters.push(current_cluster);
    }

    clusters
}

// ============================================================================
// Cross-Source Overlap Detection
// ============================================================================

/// Execute DetectCrossSourceOverlaps - cluster fingerprint overlaps by source directory.
///
/// Groups FingerprintOverlap signals by their configured source directories (from config
/// `dir` stanzas), emitting CrossSourceOverlap signals for overlaps spanning different sources.
///
/// Within-source overlaps are ignored (they're legitimate variants/releases within a collection).
/// Files not in any configured source are classified under "undeployed" pseudo-source.
///
/// Example output:
/// - "web/releases/bandcamp|web/releases/indie" -> 1023 overlapping tracks
/// - "tracks-trans|tracks-dab" -> 47 overlapping tracks
pub fn execute_detect_cross_source_overlaps(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectCrossSourceOverlaps;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Load config to get source directories
    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to load config: {}", e),
            );
        }
    };

    // Get all FingerprintOverlap signals
    let fp_overlap_signals = read_only_db
        .get_fingerprint_overlap_signals()
        .unwrap_or_default();

    if fp_overlap_signals.is_empty() {
        log_general("[COMPUTE] DetectCrossSourceOverlaps: no fingerprint overlap signals to analyze");
        // Reconcile with empty set to clear any stale signals
        let (cleared, _, _, _) =
            reconcile_aggregate_signals::<CrossSourceOverlapSignal>(read_only_db, &sender, Vec::new(), witness);
        if cleared > 0 {
            log_general(format!(
                "[COMPUTE] DetectCrossSourceOverlaps: cleared {} stale signals",
                cleared
            ));
        }
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // =========================================================================
    // Pass 1: Classify each overlap by source pair
    // =========================================================================
    // Key: sorted source pair (e.g., "web/releases/bandcamp|web/releases/indie")
    // Value: (fingerprint_keys, track_pairs)
    let mut source_pair_overlaps: HashMap<String, SourcePairOverlap> = HashMap::new();

    let mut total_overlaps = 0;
    let mut within_source_skipped = 0;
    let mut expected_skipped = 0;

    // Build an inode->path lookup for generating track pairs with paths
    let mut inode_path_map: HashMap<i64, String> = HashMap::new();

    for signal in &fp_overlap_signals {
        let inodes = signal.inodes.clone();
        if inodes.len() < 2 {
            continue;
        }

        // Get corpus audio files for this overlap group
        let audio_files = match read_only_db.get_audio_files_by_inodes(&inodes, Zone::Corpus) {
            Ok(f) => f,
            Err(_) => continue,
        };

        if audio_files.len() < 2 {
            continue;
        }

        total_overlaps += 1;

        // Classify each file by its source directory
        let mut files_by_source: HashMap<String, Vec<i64>> = HashMap::new();

        for audio_file in &audio_files {
            let path = audio_file.path();
            let inode = audio_file.inode();

            // Cache inode->path for track pair construction
            inode_path_map.insert(inode, path.to_string());

            // Strip "corpus/" prefix if present to get relative path
            let relative_path = path.strip_prefix("corpus/").unwrap_or(path);

            // Look up source directory from config
            let source_key = config
                .resolve_source_config(Path::new(relative_path))
                .map(|r| r.source_path.to_string_lossy().to_string())
                .unwrap_or_else(|| "undeployed".to_string());

            files_by_source
                .entry(source_key)
                .or_default()
                .push(inode);
        }

        // If all files are in the SAME source, skip (within-source overlap)
        if files_by_source.len() < 2 {
            within_source_skipped += 1;
            continue;
        }

        // Generate cross-source pairs
        let source_keys: Vec<&String> = files_by_source.keys().collect();
        for i in 0..source_keys.len() {
            for j in (i + 1)..source_keys.len() {
                let source_a = source_keys[i];
                let source_b = source_keys[j];

                // Create sorted pair key
                let (key_a, key_b) = if source_a < source_b {
                    (source_a.as_str(), source_b.as_str())
                } else {
                    (source_b.as_str(), source_a.as_str())
                };
                let pair_key = format!("{}|{}", key_a, key_b);

                // Skip expected overlaps
                if read_only_db.is_expected_overlap(&pair_key).unwrap_or(false) {
                    expected_skipped += 1;
                    continue;
                }

                let overlap = source_pair_overlaps.entry(pair_key.clone()).or_insert_with(|| {
                    SourcePairOverlap {
                        source_a: key_a.to_string(),
                        source_b: key_b.to_string(),
                        fingerprint_keys: Vec::new(),
                        track_pairs: Vec::new(),
                    }
                });

                // Add fingerprint key
                if !overlap.fingerprint_keys.contains(&signal.key) {
                    overlap.fingerprint_keys.push(signal.key.clone());
                }

                // Add track pairs (inodes from each source)
                let inodes_a = &files_by_source[source_a];
                let inodes_b = &files_by_source[source_b];

                for &inode_a in inodes_a {
                    for &inode_b in inodes_b {
                        let (pair_a_inode, pair_b_inode) = if source_a < source_b {
                            (inode_a, inode_b)
                        } else {
                            (inode_b, inode_a)
                        };

                        // Deduplicate by checking if this exact pair already exists
                        let already_exists = overlap.track_pairs.iter().any(|tp| {
                            tp.source_a_inode == pair_a_inode && tp.source_b_inode == pair_b_inode
                        });

                        if !already_exists {
                            overlap.track_pairs.push(CrossSourceTrackPair {
                                fingerprint_key: signal.key.clone(),
                                source_a_inode: pair_a_inode,
                                source_a_path: inode_path_map.get(&pair_a_inode)
                                    .cloned()
                                    .unwrap_or_default(),
                                source_b_inode: pair_b_inode,
                                source_b_path: inode_path_map.get(&pair_b_inode)
                                    .cloned()
                                    .unwrap_or_default(),
                            });
                        }
                    }
                }
            }
        }
    }

    // =========================================================================
    // Pass 2: Build computed CrossSourceOverlap signals for reconciliation
    // =========================================================================
    let mut computed = Vec::new();

    for (pair_key, overlap) in source_pair_overlaps {
        if overlap.track_pairs.is_empty() {
            continue;
        }

        // Look up source configs for can_stash info
        let source_a_resolved = config.resolve_source_config(Path::new(&overlap.source_a));
        let source_b_resolved = config.resolve_source_config(Path::new(&overlap.source_b));

        computed.push(ComputedAggregateSignal::new(
            pair_key.clone(),
            TypedSignalWrite::CrossSourceOverlap(CrossSourceOverlapSignal {
                key: pair_key,
                data: CrossSourceOverlapData {
                    source_a: overlap.source_a,
                    source_b: overlap.source_b,
                    source_a_can_stash: source_a_resolved.as_ref().map(|r| r.can_stash_dupes).unwrap_or(true),
                    source_b_can_stash: source_b_resolved.as_ref().map(|r| r.can_stash_dupes).unwrap_or(true),
                    overlap_count: overlap.track_pairs.len(),
                    fingerprint_count: overlap.fingerprint_keys.len(),
                    fingerprint_keys: overlap.fingerprint_keys,
                    track_pairs: overlap.track_pairs,
                },
            }),
        ));
    }

    let (cleared, new_count, updated, unchanged) =
        reconcile_aggregate_signals::<CrossSourceOverlapSignal>(read_only_db, &sender, computed, witness);

    let emitted_count = new_count + updated + unchanged;
    log_general(format!(
        "[COMPUTE] DetectCrossSourceOverlaps: {} cross-source pairs ({} fingerprint overlaps, {} within-source skipped, {} expected skipped), cleared={}, new={}, updated={}, unchanged={}",
        emitted_count, total_overlaps, within_source_skipped, expected_skipped,
        cleared, new_count, updated, unchanged
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Accumulated data for a source pair overlap.
struct SourcePairOverlap {
    source_a: String,
    source_b: String,
    fingerprint_keys: Vec<String>,
    track_pairs: Vec<CrossSourceTrackPair>,
}
