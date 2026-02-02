//! Awake-phase computation executors.
//!
//! These functions implement the actual logic for Awake computations.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use mla_utils::tag_names::find_tag_in_map;

use crate::logging::log_general;
use crate::corpus::computations::helpers::{
    drop_stale_file_signal, ensure_file_signal_if_missing,
    ensure_file_signal_with_metadata_if_missing, get_configured_library_names,
    parse_track_ids_csv, reconcile_aggregate_signals, ComputedAggregateSignal,
};
use crate::corpus::computations::types::ComputationWitness;
use crate::corpus::db::types::{AggregateSignal, AggregateSignalType, FileSource, LibraryFileSignalType, SignalType};
use crate::corpus::deploy::compute_deployment_path_with_tags;
use crate::corpus::db::ReadOnlyDb;
use crate::db_thread;

use super::{Computation, Result};

// ============================================================================
// Schedule Content Analysis
// ============================================================================

/// Execute ScheduleContentAnalysis - spawns all content detection computations.
pub fn execute_schedule_content_analysis(
    read_only_db: &ReadOnlyDb<'_>,
    start: Instant,
) -> Result {
    log_general("[COMPUTE] ScheduleContentAnalysis: spawning all detection computations");

    // Note: OOB tag change classification is now handled in Asleep phase by VerifyTags
    //
    // AnalyzeFingerprintOverlaps and ClusterDirectoryOverlaps are NOT spawned here.
    // They depend on FingerprintOverlap signals written by DetectFingerprintOverlaps,
    // so DetectFingerprintOverlaps spawns them after calling wait_for_queue_drain().
    let mut spawn = vec![
        Computation::DetectFingerprintOverlaps,
        Computation::DetectDuplicateInodes,
        Computation::DetectMissingTags,
        Computation::DetectMetadataDuplicates,
        Computation::DetectTagCanonicalizations,
        Computation::DetectInconsistentAlbumArtist,
        Computation::DetectCompoundTagValues,
        Computation::DetectShitFormats,
        Computation::DetectDeployConflicts,
        Computation::DeriveCorpusDeployStatus,
    ];

    // Also spawn DeriveDeployHealthSignals for each configured library
    // The scan data was stored in files table (source='library') during Awakening phase
    if let Ok(config) = crate::config::load_config() {
        let library_names = get_configured_library_names(&config);
        for library_name in library_names {
            let library_root = config.libraries_dir().join(&library_name);
            let corpus_path_prefixes = config.get_corpus_paths_for_library(&library_name);
            spawn.push(Computation::DeriveDeployHealthSignals {
                library_name,
                library_root,
                corpus_path_prefixes,
            });
        }
    }

    // Suppress unused read_only_db warning - not used in this function
    let _ = read_only_db;

    Result::success(
        Computation::ScheduleContentAnalysis,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

// ============================================================================
// Fingerprint Duplicate Detection
// ============================================================================

/// Execute DetectFingerprintOverlaps - bulk detection of fingerprint overlaps.
pub fn execute_detect_fingerprint_overlaps(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::db::queries::files::fingerprint_to_text;

    let computation = Computation::DetectFingerprintOverlaps;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Find all fingerprints with duplicates
    let duplicate_groups = match read_only_db.get_duplicate_fingerprint_groups() {
        Ok(groups) => groups,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query fingerprint duplicates: {}", e),
            );
        }
    };

    // Build computed signals
    let mut computed = Vec::new();
    let mut total_tracks = 0;

    for (fp_blob, track_ids_str) in duplicate_groups {
        // Convert BLOB to Vec<u32> then to text for signal key
        let fp_u32: Vec<u32> = fp_blob
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect();
        let fingerprint_text = fingerprint_to_text(&fp_u32);

        let track_ids = parse_track_ids_csv(&track_ids_str);
        total_tracks += track_ids.len();

        // Metadata no longer stores fingerprint (it's already the signal key)
        let metadata = serde_json::json!({
            "track_ids": &track_ids,
        })
        .to_string();

        computed.push(ComputedAggregateSignal {
            key: fingerprint_text,
            track_ids,
            metadata_json: metadata,
        });
    }

    // Reconcile with existing signals (handles stale/new/changed/unchanged)
    let (cleared, new_count, updated, unchanged) = reconcile_aggregate_signals(
        read_only_db,
        &sender,
        AggregateSignalType::FingerprintOverlap,
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
    db_thread::wait_for_queue_drain();

    // Spawn dependent computations that read FingerprintOverlap signals
    Result::success(
        computation,
        start.elapsed().as_millis() as u64,
        vec![
            Computation::AnalyzeFingerprintOverlaps,
            Computation::ClusterDirectoryOverlaps,
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

    let sender = match db_thread::signal_sender() {
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

    for (inode, track_ids_str) in duplicate_groups {
        let track_ids = parse_track_ids_csv(&track_ids_str);

        let metadata = serde_json::json!({
            "inode": inode,
            "track_ids": &track_ids,
        })
        .to_string();

        computed.push(ComputedAggregateSignal {
            key: inode.to_string(),
            track_ids,
            metadata_json: metadata,
        });
    }

    // Reconcile with existing signals (handles stale/new/changed/unchanged)
    let (cleared, new_count, updated, unchanged) = reconcile_aggregate_signals(
        read_only_db,
        &sender,
        AggregateSignalType::DuplicateInode,
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
// Missing Tags Detection
// ============================================================================

/// Execute DetectMissingTags - detect tracks missing required tags.
pub fn execute_detect_missing_tags(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use std::collections::HashSet;

    let computation = Computation::DetectMissingTags;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

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

    let required_tags: HashSet<String> = config
        .opinions
        .health_detection
        .required_tags
        .iter()
        .map(|s| s.to_lowercase())
        .collect();

    // Clear all existing MissingTag signals (routes through db_thread)
    sender.clear_signals_by_type(SignalType::MissingTag, witness);

    let tracks_with_tags = match read_only_db.get_audio_files_with_tag_presence() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query files with tag presence: {}", e),
            );
        }
    };

    let mut groups: HashMap<String, (HashSet<String>, Vec<i64>)> = HashMap::new();

    for (track_id, path, album, present_tags_str) in tracks_with_tags {

        let present_tags: HashSet<String> = present_tags_str
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        let missing: HashSet<String> = required_tags
            .difference(&present_tags)
            .cloned()
            .collect();

        if missing.is_empty() {
            continue;
        }

        let key = if let Some(album_name) = album {
            format!("album={}", album_name)
        } else {
            let parent = Path::new(&path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string());
            format!("dir={}", parent)
        };

        let entry = groups.entry(key).or_insert_with(|| (HashSet::new(), Vec::new()));
        entry.0.extend(missing);
        entry.1.push(track_id);
    }

    let mut total_groups = 0;

    for (key, (missing_tags, track_ids)) in groups {
        total_groups += 1;

        let mut missing_list: Vec<String> = missing_tags.into_iter().collect();
        missing_list.sort();

        let signal = AggregateSignal {
            id: None,
            signal_type: AggregateSignalType::MissingTag,
            key: key.clone(),
            discovered_at: None,
            metadata_json: Some(serde_json::json!({
                "missing_tags": missing_list,
            }).to_string()),
        }
        .with_track_ids(&track_ids);

        sender.replace_aggregate_signal(signal, witness);
    }

    log_general(format!(
        "[COMPUTE] DetectMissingTags: {} groups with missing tags",
        total_groups
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

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Clear all MetadataDuplicate signals (routes through db_thread)
    sender.clear_signals_by_type(SignalType::MetadataDuplicate, witness);

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

    let mut track_tags: HashMap<i64, Vec<(String, String)>> = HashMap::new();

    for (track_id, tag_name, tag_value) in all_tags {
        track_tags
            .entry(track_id)
            .or_default()
            .push((tag_name.to_lowercase(), tag_value));
    }

    let mut sig_to_tracks: HashMap<String, Vec<i64>> = HashMap::new();

    for (track_id, mut tags) in track_tags {
        tags.sort_by(|a, b| a.0.cmp(&b.0));
        let signature: String = tags
            .iter()
            .map(|(name, value)| format!("{}={}", name, value))
            .collect::<Vec<_>>()
            .join("|");

        sig_to_tracks
            .entry(signature)
            .or_default()
            .push(track_id);
    }

    let mut total_groups = 0;

    for (signature, track_ids) in sig_to_tracks {
        if track_ids.len() < 2 {
            continue;
        }

        total_groups += 1;

        let key_hash = format!("{:x}", md5_hash(&signature));

        let signal = AggregateSignal {
            id: None,
            signal_type: AggregateSignalType::MetadataDuplicate,
            key: key_hash,
            discovered_at: None,
            metadata_json: Some(serde_json::json!({
                "tag_signature": signature,
            }).to_string()),
        }
        .with_track_ids(&track_ids);

        sender.replace_aggregate_signal(signal, witness);
    }

    log_general(format!(
        "[COMPUTE] DetectMetadataDuplicates: {} duplicate metadata groups",
        total_groups
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
// Tag Canonicalization Detection
// ============================================================================

/// Execute DetectTagCanonicalizations - detect tag canonicalization opportunities.
///
/// Emits TagCanonicity aggregate signals for each detected collision cluster.
pub fn execute_detect_tag_canonicalizations(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::health::collision::{
        get_album_artist_collisions, get_album_collisions, get_artist_collisions,
        get_genre_collisions,
    };

    let computation = Computation::DetectTagCanonicalizations;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Clear stale TagCanonicity signals before re-detecting
    sender.clear_signals_by_type(SignalType::TagCanonicity, witness);

    let mut signal_count = 0;

    // Helper to emit signals for a set of collisions
    let emit_collision_signals = |collisions: Vec<crate::corpus::health::collision::TagCollision>,
                                   sender: &crate::db_thread::SignalWriteSender,
                                   witness: &ComputationWitness,
                                   count: &mut usize| {
        for collision in collisions {
            // Get track_ids for all variants in this collision
            let variant_refs: Vec<&str> = collision.variants.iter().map(|s| s.as_str()).collect();
            let track_ids = read_only_db
                .get_inodes_for_tag_values(&collision.tag_name, &variant_refs)
                .unwrap_or_default();

            // Build metadata JSON
            let variants_json: serde_json::Map<String, serde_json::Value> = collision
                .variant_counts
                .iter()
                .map(|(k, v)| (k.clone(), serde_json::Value::Number((*v as u64).into())))
                .collect();

            let metadata = serde_json::json!({
                "tag_name": collision.tag_name,
                "variants": variants_json,
                "track_ids": track_ids,
            });

            // Signal key: "{tag_name}:{normalized_key}"
            let key = format!("{}:{}", collision.tag_name, collision.normalized_key);

            sender.ensure_aggregate_signal(
                AggregateSignalType::TagCanonicity,
                &key,
                Some(&metadata.to_string()),
                witness,
            );

            *count += 1;
        }
    };

    // Detect and emit signals for each tag type
    if let Ok(collisions) = get_artist_collisions(read_only_db) {
        emit_collision_signals(collisions, &sender, witness, &mut signal_count);
    }

    if let Ok(collisions) = get_album_artist_collisions(read_only_db) {
        emit_collision_signals(collisions, &sender, witness, &mut signal_count);
    }

    if let Ok(collisions) = get_album_collisions(read_only_db) {
        emit_collision_signals(collisions, &sender, witness, &mut signal_count);
    }

    if let Ok(collisions) = get_genre_collisions(read_only_db) {
        emit_collision_signals(collisions, &sender, witness, &mut signal_count);
    }

    log_general(format!(
        "[COMPUTE] DetectTagCanonicalizations: emitted {} TagCanonicity signals",
        signal_count
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Execute DetectCompoundTagValues - detect tag values that should be split.
///
/// Emits CompoundTagValue aggregate signals for each detected compound value.
pub fn execute_detect_compound_tag_values(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::health::compound::{detect_all_compound_values, get_inodes_for_compound_value};

    let computation = Computation::DetectCompoundTagValues;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Clear stale CompoundTagValue signals before re-detecting
    sender.clear_signals_by_type(SignalType::CompoundTagValue, witness);

    // Get tag splitting config from opinions
    let tag_separators = crate::config::load_config()
        .map(|c| c.opinions.tag_splitting.tag_separators.clone())
        .unwrap_or_default();

    if tag_separators.is_empty() {
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    let compound_values = match detect_all_compound_values(read_only_db, &tag_separators) {
        Ok(v) => v,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to detect compound values: {}", e),
            );
        }
    };

    let mut signal_count = 0;

    for cv in compound_values {
        // Get inodes for this compound value
        let track_ids = get_inodes_for_compound_value(read_only_db, &cv.tag_name, &cv.compound_value)
            .unwrap_or_default();

        if track_ids.is_empty() {
            continue;
        }

        // Build metadata JSON
        let metadata = serde_json::json!({
            "tag_name": cv.tag_name,
            "compound_value": cv.compound_value,
            "split_parts": cv.split_parts,
            "separator": cv.separator,
            "track_ids": track_ids,
        });

        // Signal key: "{tag_name}:{hash}" - use a simple hash of the compound value
        let hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            cv.compound_value.hash(&mut hasher);
            format!("{:x}", hasher.finish())
        };
        let key = format!("{}:{}", cv.tag_name, hash);

        sender.ensure_aggregate_signal(
            AggregateSignalType::CompoundTagValue,
            &key,
            Some(&metadata.to_string()),
            witness,
        );

        signal_count += 1;
    }

    log_general(format!(
        "[COMPUTE] DetectCompoundTagValues: emitted {} CompoundTagValue signals",
        signal_count
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Shit Format Detection
// ============================================================================

/// Execute DetectShitFormats - detect files with non-Vorbis container formats.
///
/// Queries all tracks and emits ShitFormat signals for those with file types
/// that have poor metadata support or inefficient containers (MP3, M4A, WAV, etc).
pub fn execute_detect_shit_formats(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::db::types::CorpusFileSignalType;

    let computation = Computation::DetectShitFormats;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    /// File types that should trigger ShitFormat signal (non-Vorbis containers)
    /// Includes lossy formats with poor metadata and lossless needing remux
    const SHIT_FORMAT_TYPES: &[&str] = &["mp3", "m4a", "aac", "wma", "wav", "aiff", "aif", "ape", "wv"];

    // Clear all existing ShitFormat signals and rebuild
    sender.clear_signals_by_type(SignalType::ShitFormat, witness);

    // Query all audio files and filter for shit formats
    let audio_files = match read_only_db.get_all_audio_files(FileSource::Corpus) {
        Ok(f) => f,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query audio files: {}", e),
            );
        }
    };

    let mut signal_count = 0;

    for audio_file in audio_files {
        let file_type_lower = audio_file.audio.file_type.to_lowercase();
        if SHIT_FORMAT_TYPES.contains(&file_type_lower.as_str()) {
            let metadata_json = serde_json::json!({
                "file_type": audio_file.audio.file_type
            }).to_string();

            sender.ensure_file_signal_with_metadata(
                CorpusFileSignalType::ShitFormat.into(),
                audio_file.path(),
                Some(&metadata_json),
                witness,
            );

            signal_count += 1;
        }
    }

    log_general(format!(
        "[COMPUTE] DetectShitFormats: emitted {} ShitFormat signals",
        signal_count
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Deploy Conflict Detection
// ============================================================================

/// Execute DetectDeployConflicts - bulk detection of deploy path collisions.
pub fn execute_detect_deploy_conflicts(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::deploy::compute_deployment_path_with_tags;

    let computation = Computation::DetectDeployConflicts;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Clear all existing DeployConflict signals (routes through db_thread)
    sender.clear_signals_by_type(SignalType::DeployConflict, witness);

    let healthy_signals = read_only_db
        .get_signals(Some(SignalType::HealthyFile))
        .unwrap_or_default();

    let mut deploy_path_to_tracks: HashMap<String, Vec<i64>> = HashMap::new();

    for signal in &healthy_signals {
        let corpus_path = &signal.issue_key;
        if let Ok(Some(audio_file)) = read_only_db.get_audio_file_by_path(corpus_path) {
            let inode = audio_file.inode();
            let tags = read_only_db.get_corpus_tags(inode).unwrap_or_default();
            let tag_map: HashMap<String, String> = tags
                .into_iter()
                .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                .collect();

            let deploy_path = compute_deployment_path_with_tags(corpus_path, &tag_map)
                .to_string_lossy()
                .to_string();

            deploy_path_to_tracks
                .entry(deploy_path)
                .or_default()
                .push(inode);
        }
    }

    let mut conflict_count = 0;
    for (deploy_path, track_ids) in deploy_path_to_tracks {
        if track_ids.len() > 1 {
            conflict_count += 1;
            let signal = AggregateSignal {
                id: None,
                signal_type: AggregateSignalType::DeployConflict,
                key: deploy_path.clone(),
                discovered_at: None,
                metadata_json: Some(
                    serde_json::json!({
                        "deploy_path": deploy_path,
                    })
                    .to_string(),
                ),
            }
            .with_track_ids(&track_ids);

            sender.replace_aggregate_signal(signal, witness);
        }
    }

    log_general(format!(
        "[COMPUTE] DetectDeployConflicts: {} conflicts among {} healthy files",
        conflict_count,
        healthy_signals.len()
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Deploy Health Signals
// ============================================================================

/// Execute DeriveDeployHealthSignals - derive library health from scan data.
///
/// Reads library file data from files table (source='library') and compares against
/// corpus index to identify leftovers and stale deployments.
pub fn execute_derive_deploy_health_signals(
    read_only_db: &ReadOnlyDb<'_>,
    library_name: &str,
    library_root: &Path,
    corpus_path_prefixes: &[std::path::PathBuf],
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::deploy::compute_deployment_path_with_tags;

    let computation = Computation::DeriveDeployHealthSignals {
        library_name: library_name.to_string(),
        library_root: library_root.to_path_buf(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
    };

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Query library file data from Awakening phase
    let library_scan_entries = match read_only_db.get_library_files(library_name) {
        Ok(entries) => entries,
        Err(e) => {
            log_general(format!(
                "[COMPUTE] DeriveDeployHealthSignals '{}': failed to get scan data: {}",
                library_name, e
            ));
            return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
        }
    };

    // Convert to (path, inode) tuples for processing
    let library_files: Vec<(std::path::PathBuf, i64)> = library_scan_entries
        .into_iter()
        .map(|entry| (entry.file_path, entry.inode))
        .collect();

    // Get all corpus audio file inodes
    let corpus_inodes = read_only_db.get_all_corpus_inodes().unwrap_or_default();

    let mut healthy_count: usize = 0;
    let mut stale_count: usize = 0;
    let mut leftover_count: usize = 0;

    for (library_path, library_inode) in &library_files {
        let leftover_key = format!(
            "library_leftover:{}:{}",
            library_name,
            library_path.display()
        );
        let stale_key = format!(
            "library_stale:{}:{}",
            library_name,
            library_path.display()
        );

        if let Some(corpus_path) = corpus_inodes.get(library_inode) {
            drop_stale_file_signal(read_only_db, &sender, LibraryFileSignalType::LibraryLeftover.into(), &leftover_key, witness);

            // Check if stale and capture metadata for the signal
            let stale_metadata = if let Ok(Some(audio_file)) = read_only_db.get_audio_file_by_path(corpus_path) {
                let inode = audio_file.inode();
                let tags = read_only_db.get_corpus_tags(inode).unwrap_or_default();
                let tag_map: std::collections::HashMap<String, String> = tags
                    .into_iter()
                    .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                    .collect();

                // Compute expected relative path within the library
                let expected_relative = compute_deployment_path_with_tags(corpus_path, &tag_map);

                // library_path is library-name-prefixed (e.g., "soundtracks/Artist/Album/track.mp3")
                // expected_relative is just "Artist/Album/track.mp3" (no library prefix)
                // Strip the library name prefix for comparison
                let library_path_suffix = library_path
                    .strip_prefix(library_name)
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|_| library_path.clone());

                if library_path_suffix != expected_relative {
                    // Stale: store paths with consistent library prefix for display and mutations
                    // Both paths stored as "{library_name}/path/..." for consistency
                    let expected_with_prefix = std::path::Path::new(library_name).join(&expected_relative);
                    Some(serde_json::json!({
                        "library_path": library_path.to_string_lossy(),
                        "expected_path": expected_with_prefix.to_string_lossy(),
                        "corpus_path": corpus_path,
                        "inode": inode
                    }))
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(metadata) = stale_metadata {
                stale_count += 1;
                ensure_file_signal_with_metadata_if_missing(
                    read_only_db,
                    &sender,
                    LibraryFileSignalType::LibraryStale.into(),
                    &stale_key,
                    &metadata.to_string(),
                    witness,
                );
            } else {
                healthy_count += 1;
                drop_stale_file_signal(read_only_db, &sender, LibraryFileSignalType::LibraryStale.into(), &stale_key, witness);
            }
        } else {
            leftover_count += 1;
            ensure_file_signal_if_missing(read_only_db, &sender, LibraryFileSignalType::LibraryLeftover.into(), &leftover_key, witness);
            drop_stale_file_signal(read_only_db, &sender, LibraryFileSignalType::LibraryStale.into(), &stale_key, witness);
        }
    }

    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': {} files, {} healthy, {} stale, {} leftover",
        library_name,
        library_files.len(),
        healthy_count,
        stale_count,
        leftover_count,
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Corpus Deploy Status
// ============================================================================

/// Execute DeriveCorpusDeployStatus - derive corpus-side deployment signals.
///
/// For each HealthyFile signal, checks if the file's inode exists in any library
/// (via files table) and emits:
/// - DeployReady: healthy file not in any library
/// - DeployedHealthy: healthy file correctly deployed (in library, not stale)
pub fn execute_derive_corpus_deploy_status(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DeriveCorpusDeployStatus;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Get all HealthyFile signals
    let healthy_signals = read_only_db
        .get_signals(Some(SignalType::HealthyFile))
        .unwrap_or_default();

    // Build set of deployed inodes from files table (source='library')
    let deployed_inodes = read_only_db.get_all_library_inodes().unwrap_or_default();

    // Get set of stale library paths (files in library but at wrong path)
    // LibraryStale keys have format "library_stale:{library_name}:{library_path}"
    let stale_signals = read_only_db
        .get_signals(Some(SignalType::LibraryStale))
        .unwrap_or_default();

    // Build set of inodes that are deployed but stale
    let mut stale_inodes: std::collections::HashSet<i64> = std::collections::HashSet::new();
    if let Ok(all_library_files) = read_only_db.get_all_library_files() {
        for entry in all_library_files {
            // Check if this library file has a stale signal
            let is_stale = stale_signals.iter().any(|s| {
                // Parse the stale key to get library_path
                let parts: Vec<&str> = s.issue_key.splitn(3, ':').collect();
                if parts.len() >= 3 {
                    let stale_library_path = parts[2];
                    entry.file_path.to_string_lossy() == stale_library_path
                } else {
                    false
                }
            });
            if is_stale {
                stale_inodes.insert(entry.inode);
            }
        }
    }

    // Get deploy-configured corpus paths to filter healthy files
    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(_) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "Failed to load config".to_string(),
            );
        }
    };

    let mut deploy_ready_count = 0usize;
    let mut deployed_healthy_count = 0usize;
    let mut skipped_not_configured = 0usize;

    for signal in &healthy_signals {
        let corpus_path = &signal.issue_key;
        let corpus_path_buf = std::path::Path::new(corpus_path);

        // Skip files not configured for deployment
        if !config.is_path_configured_for_deploy(corpus_path_buf) {
            skipped_not_configured += 1;
            // Clear any stale deploy signals for unconfigured files
            drop_stale_file_signal(
                read_only_db,
                &sender,
                LibraryFileSignalType::DeployReady.into(),
                corpus_path,
                witness,
            );
            drop_stale_file_signal(
                read_only_db,
                &sender,
                LibraryFileSignalType::DeployedHealthy.into(),
                corpus_path,
                witness,
            );
            continue;
        }

        // Get the audio file (we need inode for tags lookup)
        let audio_file = match read_only_db.get_audio_file_by_path(corpus_path) {
            Ok(Some(af)) => af,
            _ => continue, // Skip if file not found
        };
        let inode = audio_file.inode();

        // Check if this inode is deployed anywhere and not stale
        let is_deployed = deployed_inodes.contains(&inode);
        let is_stale = stale_inodes.contains(&inode);

        if is_deployed && !is_stale {
            // File is correctly deployed
            deployed_healthy_count += 1;
            ensure_file_signal_if_missing(
                read_only_db,
                &sender,
                LibraryFileSignalType::DeployedHealthy.into(),
                corpus_path,
                witness,
            );
            drop_stale_file_signal(
                read_only_db,
                &sender,
                LibraryFileSignalType::DeployReady.into(),
                corpus_path,
                witness,
            );
        } else {
            // File is not deployed (or deployed but stale)
            deploy_ready_count += 1;

            // Compute the deploy path for this file (relative)
            let tags = read_only_db.get_corpus_tags(inode).unwrap_or_default();
            let tag_map: HashMap<String, String> = tags
                .into_iter()
                .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                .collect();
            // Compute relative deploy path (relative to library root)
            let deploy_path = compute_deployment_path_with_tags(corpus_path, &tag_map);

            // Store relative path in metadata
            let metadata = serde_json::json!({
                "deploy_path": deploy_path.to_string_lossy(),
            });
            ensure_file_signal_with_metadata_if_missing(
                read_only_db,
                &sender,
                LibraryFileSignalType::DeployReady.into(),
                corpus_path,
                &metadata.to_string(),
                witness,
            );
            drop_stale_file_signal(
                read_only_db,
                &sender,
                LibraryFileSignalType::DeployedHealthy.into(),
                corpus_path,
                witness,
            );
        }
    }

    log_general(format!(
        "[COMPUTE] DeriveCorpusDeployStatus: {} healthy files, {} deploy-ready, {} deployed-healthy, {} not configured",
        healthy_signals.len(),
        deploy_ready_count,
        deployed_healthy_count,
        skipped_not_configured,
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Inconsistent Album Artist Detection
// ============================================================================

/// Execute DetectInconsistentAlbumArtist - detect albums with inconsistent album_artist tags.
///
/// Emits InconsistentAlbumArtist aggregate signals for albums where:
/// - Multiple artists are present on the same album
/// - album_artist tags are missing or inconsistent
pub fn execute_detect_inconsistent_album_artist(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::health::album_artist_detection::detect_inconsistent_album_artist;

    let computation = Computation::DetectInconsistentAlbumArtist;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Clear stale InconsistentAlbumArtist signals before re-detecting
    sender.clear_signals_by_type(SignalType::InconsistentAlbumArtist, witness);

    let issues = match detect_inconsistent_album_artist(read_only_db) {
        Ok(i) => i,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to detect inconsistent album_artist: {}", e),
            );
        }
    };

    let mut signal_count = 0;

    for issue in issues {
        // Build metadata JSON
        let artist_variants_json: serde_json::Map<String, serde_json::Value> = issue
            .artist_variants
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::Number((*v as u64).into())))
            .collect();

        let album_artist_variants_json: serde_json::Map<String, serde_json::Value> = issue
            .album_artist_variants
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::Number((*v as u64).into())))
            .collect();

        let metadata = serde_json::json!({
            "album": issue.album,
            "artist_variants": artist_variants_json,
            "album_artist_variants": album_artist_variants_json,
            "track_ids": issue.track_ids,
        });

        // Signal key: normalized album name
        sender.ensure_aggregate_signal(
            AggregateSignalType::InconsistentAlbumArtist,
            &issue.normalized_album,
            Some(&metadata.to_string()),
            witness,
        );

        signal_count += 1;
    }

    log_general(format!(
        "[COMPUTE] DetectInconsistentAlbumArtist: emitted {} signals",
        signal_count
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Fingerprint Duplicate Analysis
// ============================================================================

/// Quality tier for audio format classification.
/// Higher value = better format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum FormatClass {
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
fn classify_format(file_type: &str) -> FormatClass {
    match file_type.to_lowercase().as_str() {
        "flac" => FormatClass::VorbisLossless,
        "opus" | "ogg" => FormatClass::VorbisLossy,
        "wav" | "aiff" | "aif" | "ape" | "wv" => FormatClass::OtherLossless,
        _ => FormatClass::OtherLossy, // mp3, m4a, aac, wma, etc.
    }
}

/// Check if a format is lossless.
fn is_lossless(file_type: &str) -> bool {
    matches!(
        file_type.to_lowercase().as_str(),
        "flac" | "wav" | "aiff" | "aif" | "ape" | "wv"
    )
}

/// Compute quality score for a track (0-1000 range).
/// Format class provides the major tier (0-750), audio quality provides the minor score.
fn compute_quality_score(
    file_type: &str,
    bitrate_kbps: Option<i32>,
    sample_rate: Option<i32>,
) -> u32 {
    let format_class = classify_format(file_type);
    let format_score = (format_class as u32) * 250; // 0, 250, 500, 750

    // For lossless, use sample rate (higher = better)
    // For lossy, use bitrate (higher = better)
    let audio_score = if is_lossless(file_type) {
        // Sample rate: 44100 -> 100, 48000 -> 109, 96000 -> 218, 192000 -> 436
        // Cap at 250 to not exceed format tier
        sample_rate.unwrap_or(44100).min(192000) as u32 * 250 / 192000
    } else {
        // Bitrate: 128 -> 80, 192 -> 120, 256 -> 160, 320 -> 200, etc.
        // Cap at 250 to not exceed format tier
        bitrate_kbps.unwrap_or(128).min(500) as u32 * 250 / 500
    };

    format_score + audio_score.min(249) // Max 999, never overflow into next tier
}

/// Compute fingerprint similarity using bit-level Hamming distance.
/// Returns similarity as percentage (0.0 - 100.0).
///
/// Chromaprint fingerprints are Vec<u32> where each u32 encodes 32 bits of spectral features.
/// We use XOR + popcount to count differing bits, then compute similarity.
fn fingerprint_similarity(fp1: &[u32], fp2: &[u32]) -> f64 {
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
    // (sliding would be O(n²) and chromaprint handles alignment internally)
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
    "bonus",
    "unplugged",
    "orchestral",
    "piano version",
    "stripped",
];

/// Check if a title contains variant keywords.
fn has_variant_keyword(title: &str) -> Option<&'static str> {
    let lower = title.to_lowercase();
    for keyword in VARIANT_KEYWORDS {
        if lower.contains(keyword) {
            return Some(keyword);
        }
    }
    None
}

/// Release identity information for variant detection.
#[derive(Debug)]
struct TrackReleaseIdentity {
    track_id: i64,
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
/// - Same ISRC (if no catalog numbers to differentiate), OR
/// - Same normalized album AND same normalized title AND no exclusive variant keywords
///
/// IMPORTANT: Catalog number is checked BEFORE ISRC because the same recording (same ISRC)
/// can appear on multiple compilation albums with different catalog numbers. These are
/// legitimate variants that should be kept, not flagged as duplicates.
fn is_same_release(a: &TrackReleaseIdentity, b: &TrackReleaseIdentity) -> bool {
    // First: Check catalog numbers - different catalog = different release
    // This catches the compilation album case where the same recording (same ISRC)
    // appears on different albums with different catalog numbers.
    if !a.catalog_number.is_empty() && !b.catalog_number.is_empty() {
        if !a.catalog_number.eq_ignore_ascii_case(&b.catalog_number) {
            return false; // Different catalog numbers = different releases (compilation variant)
        }
        return true; // Same catalog number = same release
    }

    // Second: ISRC match (only if no catalog numbers to differentiate)
    // If we get here, at least one track is missing a catalog number,
    // so ISRC is the best differentiator available.
    if !a.isrc.is_empty() && !b.isrc.is_empty() && a.isrc.eq_ignore_ascii_case(&b.isrc) {
        return true;
    }

    // Fall through: Check album names (normalized)
    let album_a = normalize_album_name(&a.album);
    let album_b = normalize_album_name(&b.album);

    if album_a != album_b {
        return false; // Different albums = different releases
    }

    // Check for exclusive variant keywords in titles
    let title_a = a.title.to_lowercase();
    let title_b = b.title.to_lowercase();

    let variant_a = has_variant_keyword(&title_a);
    let variant_b = has_variant_keyword(&title_b);

    // If one has a variant keyword the other doesn't, they're different versions
    match (variant_a, variant_b) {
        (Some(kw_a), Some(kw_b)) if kw_a != kw_b => false, // Different variant types
        (Some(_), None) | (None, Some(_)) => false,       // One is variant, other isn't
        _ => true, // Both have same variant or neither has variant
    }
}

/// Reason why a track is subpar.
#[derive(Debug, Clone, Copy)]
enum SubparReason {
    SubparFormat,
    SubparBitrate,
}

impl SubparReason {
    fn as_str(&self) -> &'static str {
        match self {
            Self::SubparFormat => "SubparFormat",
            Self::SubparBitrate => "SubparBitrate",
        }
    }
}

/// Execute AnalyzeFingerprintOverlaps - deep analysis of fingerprint overlap groups.
pub fn execute_analyze_fingerprint_overlaps(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::db::types::CorpusFileSignalType;

    let computation = Computation::AnalyzeFingerprintOverlaps;

    let sender = match db_thread::signal_sender() {
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

    // Clear all existing SubparDuplicate signals
    sender.clear_signals_by_type(SignalType::from(CorpusFileSignalType::SubparDuplicate), witness);

    // Get all FingerprintOverlap signals
    let fp_dup_signals = read_only_db
        .get_aggregate_signals(Some(AggregateSignalType::FingerprintOverlap))
        .unwrap_or_default();

    if fp_dup_signals.is_empty() {
        log_general("[COMPUTE] AnalyzeFingerprintOverlaps: no fingerprint overlap signals to analyze");
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    let mut total_groups = 0;
    let mut subpar_count = 0;
    let mut variant_skipped = 0;

    for signal in &fp_dup_signals {
        let track_ids = signal.track_ids();
        if track_ids.len() < 2 {
            continue;
        }

        total_groups += 1;

        // Get corpus audio files for this group (track_ids in signals are actually inodes)
        let audio_files = match read_only_db.get_audio_files_by_inodes(&track_ids, FileSource::Corpus) {
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
                    .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                    .collect();

                identities.push(TrackReleaseIdentity {
                    track_id: inode,
                    path: audio_file.path().to_string(),
                    album: tag_map.get("album").cloned().unwrap_or_default(),
                    title: tag_map.get("title").cloned().unwrap_or_default(),
                    isrc: tag_map.get("isrc").cloned().unwrap_or_default(),
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
                    if !is_same_release(&identities[i], &identities[j]) {
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

            // For each true duplicate group, rank by quality and emit SubparDuplicate signals
            for group in true_duplicate_groups {
                // Compute quality scores
                let mut scored: Vec<(usize, u32)> = group
                    .iter()
                    .map(|&idx| {
                        let audio_file = &cluster[idx];
                        let score = compute_quality_score(
                            &audio_file.audio.file_type,
                            audio_file.audio.bitrate_kbps,
                            audio_file.audio.sample_rate,
                        );
                        (idx, score)
                    })
                    .collect();

                // Sort by score descending (best first)
                scored.sort_by(|a, b| b.1.cmp(&a.1));

                // Best audio file is the first one
                let (best_idx, best_score) = scored[0];
                let best_audio_file = &cluster[best_idx];
                let best_identity = &identities[best_idx];

                // Emit SubparDuplicate for all others
                for &(idx, score) in scored.iter().skip(1) {
                    let audio_file = &cluster[idx];

                    // Determine reason: format difference or bitrate/quality difference
                    let reason = if classify_format(&audio_file.audio.file_type) < classify_format(&best_audio_file.audio.file_type) {
                        SubparReason::SubparFormat
                    } else {
                        SubparReason::SubparBitrate
                    };

                    let metadata = serde_json::json!({
                        "reason": reason.as_str(),
                        "superior_inode": best_identity.track_id,
                        "superior_path": best_identity.path,
                        "dupe_group_fingerprint": signal.key.clone(),
                        "quality_score": score,
                        "superior_quality_score": best_score,
                    });

                    sender.ensure_file_signal_with_metadata(
                        CorpusFileSignalType::SubparDuplicate.into(),
                        audio_file.path(),
                        Some(&metadata.to_string()),
                        witness,
                    );

                    subpar_count += 1;
                }
            }
        }
    }

    log_general(format!(
        "[COMPUTE] AnalyzeFingerprintOverlaps: analyzed {} groups, emitted {} SubparDuplicate signals, skipped {} variants",
        total_groups, subpar_count, variant_skipped
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Cluster audio files by duration within tolerance.
fn cluster_by_duration<'a>(
    audio_files: &'a [crate::corpus::db::types::AudioFile],
    tolerance_ms: i64,
) -> Vec<Vec<&'a crate::corpus::db::types::AudioFile>> {
    if audio_files.is_empty() {
        return Vec::new();
    }

    // Sort by duration
    let mut sorted: Vec<_> = audio_files.iter().collect();
    sorted.sort_by_key(|af| af.audio.duration_ms.unwrap_or(0));

    let mut clusters: Vec<Vec<&crate::corpus::db::types::AudioFile>> = Vec::new();
    let mut current_cluster: Vec<&crate::corpus::db::types::AudioFile> = vec![sorted[0]];
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
// Directory Overlap Clustering
// ============================================================================

/// Execute ClusterDirectoryOverlaps - cluster fingerprint overlaps by directory.
///
/// Sibling-aware two-pass clustering:
/// 1. Group overlaps by common_root (not by component-pair key)
/// 2. For each common_root, count sibling directories at divergence point:
///    - Many siblings (> many_siblings_threshold): Aggregate into ONE cluster keyed by
///      common_root's last component (e.g., "monstercat")
///    - Few siblings (≤ threshold): Use traditional component-pair keying (e.g., "bandcamp|indie")
///
/// This prevents explosion of clusters when a common_root like "monstercat" has 1500+
/// subdirectories (one per catalog number) while preserving fine-grained clustering for
/// directories with few siblings.
pub fn execute_cluster_directory_overlaps(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::ClusterDirectoryOverlaps;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Load config for threshold values
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

    let cross_directory_max_keys = config.opinions.duplicate_analysis.cross_directory_max_keys;
    let within_directory_min_keys = config.opinions.duplicate_analysis.within_directory_min_keys;
    let many_siblings_threshold = config.opinions.duplicate_analysis.many_siblings_threshold;

    // Clear all existing DirectoryOverlapCluster signals
    sender.clear_signals_by_type(SignalType::DirectoryOverlapCluster, witness);

    // Get all FingerprintOverlap signals
    let fp_overlap_signals = read_only_db
        .get_aggregate_signals(Some(AggregateSignalType::FingerprintOverlap))
        .unwrap_or_default();

    if fp_overlap_signals.is_empty() {
        log_general("[COMPUTE] ClusterDirectoryOverlaps: no fingerprint overlap signals to cluster");
        return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
    }

    // =========================================================================
    // Pass 1: Group by common_root (not by component-pair key)
    // =========================================================================
    // common_root -> RootBuilder (accumulates all overlaps under that root)
    let mut root_map: HashMap<String, RootBuilder> = HashMap::new();

    for signal in &fp_overlap_signals {
        let track_ids = signal.track_ids();
        if track_ids.len() < 2 {
            continue;
        }

        // Get corpus audio files for this overlap group
        let audio_files = match read_only_db.get_audio_files_by_inodes(&track_ids, FileSource::Corpus) {
            Ok(f) => f,
            Err(_) => continue,
        };

        if audio_files.len() < 2 {
            continue;
        }

        // Build (path, track_id) pairs
        let path_track_pairs: Vec<(&str, i64)> = audio_files
            .iter()
            .map(|af| (af.path(), af.inode()))
            .collect();

        // Find directory divergence point
        let (common_root, divergence) = find_directory_divergence_with_root(&path_track_pairs);
        if divergence.is_empty() || divergence.len() < 2 {
            continue;
        }

        // Group by common_root
        let builder = root_map.entry(common_root.clone()).or_insert_with(|| {
            RootBuilder {
                common_root: common_root.clone(),
                // Map: first_component -> track_ids
                directories: HashMap::new(),
                fingerprint_keys: Vec::new(),
            }
        });

        builder.fingerprint_keys.push(signal.key.clone());

        // Add track IDs to their respective directories (by first component)
        for (suffix, track_ids_for_suffix) in divergence {
            let first_comp = extract_first_component(&suffix).to_string();
            let dir_entry = builder.directories.entry(first_comp).or_default();
            for tid in track_ids_for_suffix {
                if !dir_entry.contains(&tid) {
                    dir_entry.push(tid);
                }
            }
        }
    }

    // =========================================================================
    // Pass 2: Decide clustering strategy per common_root based on sibling count
    // =========================================================================
    let mut cluster_count = 0;
    let mut aggregated_count = 0;
    let mut skipped_within_dir = 0;

    for (common_root, builder) in root_map {
        let num_diverging_keys = builder.directories.len();

        // Skip if only one directory
        if num_diverging_keys < 2 {
            continue;
        }

        // Count sibling directories at this divergence point
        let sibling_count = read_only_db
            .count_child_directories(&common_root, FileSource::Corpus)
            .unwrap_or(0);

        if sibling_count > many_siblings_threshold {
            // MANY SIBLINGS: Aggregate all into ONE cluster keyed by last component of common_root
            // e.g., "corpus/web/releases/monstercat" -> key = "monstercat"
            let cluster_key = common_root
                .rsplit('/')
                .next()
                .unwrap_or(&common_root)
                .to_string();

            // Build directories array with all divergent subdirectories
            let mut directories: Vec<serde_json::Value> = builder.directories
                .iter()
                .map(|(key, track_ids)| {
                    serde_json::json!({
                        "path_suffix": key,
                        "track_ids": track_ids,
                    })
                })
                .collect();
            directories.sort_by(|a, b| {
                let a_suffix = a.get("path_suffix").and_then(|v| v.as_str()).unwrap_or("");
                let b_suffix = b.get("path_suffix").and_then(|v| v.as_str()).unwrap_or("");
                a_suffix.cmp(b_suffix)
            });

            let metadata = serde_json::json!({
                "common_root": builder.common_root,
                "directories": directories,
                "fingerprint_count": builder.fingerprint_keys.len(),
                "fingerprint_overlap_keys": builder.fingerprint_keys,
                "aggregated": true,
                "sibling_count": sibling_count,
            });

            sender.ensure_aggregate_signal(
                AggregateSignalType::DirectoryOverlapCluster,
                &cluster_key,
                Some(&metadata.to_string()),
                witness,
            );

            aggregated_count += 1;
        } else {
            // FEW SIBLINGS: Use traditional component-pair keying
            // Skip if too many diverging keys (within-directory variants)
            if num_diverging_keys >= within_directory_min_keys {
                skipped_within_dir += 1;
                continue;
            }

            if num_diverging_keys > cross_directory_max_keys {
                // In the gap between thresholds - skip
                continue;
            }

            // Build cluster key from sorted component pairs
            let mut sorted_comps: Vec<_> = builder.directories.keys().collect();
            sorted_comps.sort();
            let cluster_key = sorted_comps
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("|");

            let mut directories: Vec<serde_json::Value> = builder.directories
                .iter()
                .map(|(key, track_ids)| {
                    serde_json::json!({
                        "path_suffix": key,
                        "track_ids": track_ids,
                    })
                })
                .collect();
            directories.sort_by(|a, b| {
                let a_suffix = a.get("path_suffix").and_then(|v| v.as_str()).unwrap_or("");
                let b_suffix = b.get("path_suffix").and_then(|v| v.as_str()).unwrap_or("");
                a_suffix.cmp(b_suffix)
            });

            let metadata = serde_json::json!({
                "common_root": builder.common_root,
                "directories": directories,
                "fingerprint_count": builder.fingerprint_keys.len(),
                "fingerprint_overlap_keys": builder.fingerprint_keys,
            });

            sender.ensure_aggregate_signal(
                AggregateSignalType::DirectoryOverlapCluster,
                &cluster_key,
                Some(&metadata.to_string()),
                witness,
            );

            cluster_count += 1;
        }
    }

    log_general(format!(
        "[COMPUTE] ClusterDirectoryOverlaps: emitted {} clusters ({} aggregated, {} fine-grained), skipped {} within-directory (from {} fingerprint overlaps)",
        aggregated_count + cluster_count, aggregated_count, cluster_count, skipped_within_dir, fp_overlap_signals.len()
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Builder for accumulating overlaps under a common root directory.
///
/// Used in sibling-aware clustering to group all overlaps that share the same
/// common_root before deciding on clustering strategy based on sibling count.
struct RootBuilder {
    /// Common path root before divergence point
    common_root: String,
    /// Map of first diverging path component -> track_ids
    directories: HashMap<String, Vec<i64>>,
    /// Source fingerprint overlap keys
    fingerprint_keys: Vec<String>,
}

/// Find directory divergence point for a set of (path, track_id) pairs.
///
/// Returns (common_root, divergence_map) where:
/// - common_root: The shared path prefix before divergence
/// - divergence_map: divergent_suffix -> track_ids for that suffix
///
/// Empty divergence_map if paths don't diverge or are invalid.
fn find_directory_divergence_with_root(path_track_pairs: &[(&str, i64)]) -> (String, Vec<(String, Vec<i64>)>) {
    if path_track_pairs.len() < 2 {
        return (String::new(), Vec::new());
    }

    // Split paths into components
    let components: Vec<Vec<&str>> = path_track_pairs
        .iter()
        .map(|(p, _)| p.split('/').collect())
        .collect();

    // Find common prefix length
    let min_len = components.iter().map(|c| c.len()).min().unwrap_or(0);
    let mut common_prefix_len = 0;

    for i in 0..min_len {
        let first = components[0].get(i);
        if components.iter().all(|c| c.get(i) == first) {
            common_prefix_len = i + 1;
        } else {
            break;
        }
    }

    // If no divergence found (all identical paths), return empty
    if common_prefix_len >= min_len {
        return (String::new(), Vec::new());
    }

    // Build common root from prefix
    let common_root = components[0][..common_prefix_len].join("/");

    // Group paths by their divergent suffix (from divergence point to one level up from file)
    let mut suffix_map: HashMap<String, Vec<i64>> = HashMap::new();

    for (idx, comps) in components.iter().enumerate() {
        // Divergent suffix: from common_prefix_len to (len - 1) to exclude filename
        let suffix_end = comps.len().saturating_sub(1);
        if common_prefix_len >= suffix_end {
            // Path is too short to have meaningful divergence
            continue;
        }

        let suffix_parts: Vec<&str> = comps[common_prefix_len..suffix_end].to_vec();
        let suffix = suffix_parts.join("/");

        // Use actual track_id from the pair
        let track_id = path_track_pairs[idx].1;
        suffix_map
            .entry(suffix)
            .or_default()
            .push(track_id);
    }

    // Convert to vec and return
    (common_root, suffix_map.into_iter().collect())
}

/// Extract the first path component from a suffix.
fn extract_first_component(suffix: &str) -> &str {
    suffix.split('/').next().unwrap_or(suffix)
}
