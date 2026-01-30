//! Awake-phase computation executors.
//!
//! These functions implement the actual logic for Awake computations.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use rusqlite::params;

use crate::logging::log_general;
use crate::corpus::computations::helpers::{
    drop_stale_file_signal, ensure_file_signal_if_missing,
    ensure_file_signal_with_metadata_if_missing, get_configured_library_names,
    parse_track_ids_csv, reconcile_aggregate_signals, ComputedAggregateSignal,
};
use crate::corpus::computations::types::ComputationWitness;
use crate::corpus::db::types::{AggregateSignal, AggregateSignalType, LibraryFileSignalType, SignalType};
use crate::corpus::deploy::compute_deployment_path_with_tags;
use crate::corpus::db::Database;
use crate::db_thread;

use super::{Computation, Result};

// ============================================================================
// Schedule Content Analysis
// ============================================================================

/// Execute ScheduleContentAnalysis - spawns all content detection computations.
pub fn execute_schedule_content_analysis(
    read_only_db: &Database,
    start: Instant,
) -> Result {
    log_general("[COMPUTE] ScheduleContentAnalysis: spawning all detection computations");

    // Note: OOB tag change classification is now handled in Asleep phase by VerifyTags
    let mut spawn = vec![
        Computation::DetectFingerprintDuplicates,
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
    // The scan data was stored in library_scan_state during Awakening phase
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

    // Suppress unused read_only_db warning - will be used once library_scan_state is implemented
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

/// Execute DetectFingerprintDuplicates - bulk detection of fingerprint duplicates.
pub fn execute_detect_fingerprint_duplicates(
    read_only_db: &Database,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::db::queries::tracks::fingerprint_to_text;

    let computation = Computation::DetectFingerprintDuplicates;

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
        AggregateSignalType::FingerprintDuplicate,
        computed,
        witness,
    );

    let total_groups = new_count + updated + unchanged;
    log_general(format!(
        "[COMPUTE] DetectFingerprintDuplicates: {} groups ({} tracks), cleared={}, new={}, updated={}, unchanged={}",
        total_groups, total_tracks, cleared, new_count, updated, unchanged
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Duplicate Inode Detection
// ============================================================================

/// Execute DetectDuplicateInodes - bulk detection of duplicate inodes.
pub fn execute_detect_duplicate_inodes(
    read_only_db: &Database,
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
    read_only_db: &Database,
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

    let tracks_with_tags = match read_only_db.get_tracks_with_tag_presence() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query tracks with tag presence: {}", e),
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
    read_only_db: &Database,
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

    let all_tags = match read_only_db.get_all_track_tags_ordered() {
        Ok(rows) => rows,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query track tags: {}", e),
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
    read_only_db: &Database,
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
                .get_track_ids_for_tag_values(&collision.tag_name, &variant_refs)
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
    read_only_db: &Database,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::health::compound::{detect_all_compound_values, get_track_ids_for_compound_value};

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
        // Get track IDs for this compound value
        let track_ids = get_track_ids_for_compound_value(read_only_db, &cv.tag_name, &cv.compound_value)
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
    read_only_db: &Database,
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

    // Query all tracks and filter for shit formats
    let tracks = match read_only_db.get_all_tracks(None) {
        Ok(t) => t,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query tracks: {}", e),
            );
        }
    };

    let mut signal_count = 0;

    for track in tracks {
        let file_type_lower = track.file_type.to_lowercase();
        if SHIT_FORMAT_TYPES.contains(&file_type_lower.as_str()) {
            let metadata_json = serde_json::json!({
                "file_type": track.file_type
            }).to_string();

            sender.ensure_file_signal_with_metadata(
                CorpusFileSignalType::ShitFormat.into(),
                &track.path,
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
    read_only_db: &Database,
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
        let path = &signal.issue_key;
        if let Ok(Some(track)) = read_only_db.get_track_by_path(path) {
            if let Some(track_id) = track.id {
                let tags = read_only_db.get_track_tags(track_id).unwrap_or_default();
                let tag_map: HashMap<String, String> = tags
                    .into_iter()
                    .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                    .collect();

                let deploy_path = compute_deployment_path_with_tags(&track, &tag_map)
                    .to_string_lossy()
                    .to_string();

                deploy_path_to_tracks
                    .entry(deploy_path)
                    .or_default()
                    .push(track_id);
            }
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
/// Reads library scan data from library_scan_state table and compares against
/// corpus index to identify leftovers and stale deployments.
pub fn execute_derive_deploy_health_signals(
    read_only_db: &Database,
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

    // Query library scan data from Awakening phase
    let library_scan_entries = match read_only_db.get_library_scan_files(library_name) {
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

    // Get all corpus track inodes
    let corpus_inodes = read_only_db.get_all_track_inodes().unwrap_or_default();

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
            let stale_metadata = if let Ok(Some(track)) = read_only_db.get_track_by_path(corpus_path) {
                if let Some(track_id) = track.id {
                    let tags = read_only_db.get_track_tags(track_id).unwrap_or_default();
                    let tag_map: std::collections::HashMap<String, String> = tags
                        .into_iter()
                        .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                        .collect();

                    // Compute expected relative path within the library
                    let expected_relative = compute_deployment_path_with_tags(&track, &tag_map);

                    // library_path is domain-prefixed relative to archive root,
                    // e.g., "libraries/music/Artist/Album/track.mp3"
                    // expected_relative is just "Artist/Album/track.mp3" (no library prefix)
                    // So we strip the "libraries/{name}" prefix for comparison
                    let library_domain_prefix = std::path::Path::new("libraries").join(library_name);
                    let library_path_suffix = library_path
                        .strip_prefix(&library_domain_prefix)
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|_| library_path.clone());

                    if library_path_suffix != expected_relative {
                        // Stale: store relative paths in metadata
                        Some(serde_json::json!({
                            "library_path": library_path.to_string_lossy(),
                            "expected_path": expected_relative.to_string_lossy(),
                            "corpus_path": corpus_path,
                            "track_id": track_id
                        }))
                    } else {
                        None
                    }
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
/// (via library_scan_state) and emits:
/// - DeployReady: healthy file not in any library
/// - DeployedHealthy: healthy file correctly deployed (in library, not stale)
pub fn execute_derive_corpus_deploy_status(
    read_only_db: &Database,
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

    // Build set of deployed inodes from library_scan_state
    let deployed_inodes = read_only_db.get_all_library_scan_inodes().unwrap_or_default();

    // Get set of stale library paths (files in library but at wrong path)
    // LibraryStale keys have format "library_stale:{library_name}:{library_path}"
    let stale_signals = read_only_db
        .get_signals(Some(SignalType::LibraryStale))
        .unwrap_or_default();

    // Build set of inodes that are deployed but stale
    let mut stale_inodes: std::collections::HashSet<i64> = std::collections::HashSet::new();
    if let Ok(all_library_files) = read_only_db.get_library_scan_files_all() {
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

        // Get the track (we need inode and track_id for tags lookup)
        let track = match read_only_db.get_track_by_path(corpus_path) {
            Ok(Some(t)) => t,
            _ => continue, // Skip if track not found
        };
        let inode = track.inode;

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

            // Compute the deploy path for this track (relative)
            let deploy_path = if let Some(track_id) = track.id {
                // Get track tags and build tag_map
                let tags = read_only_db.get_track_tags(track_id).unwrap_or_default();
                let tag_map: HashMap<String, String> = tags
                    .into_iter()
                    .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                    .collect();
                // Compute relative deploy path (relative to library root)
                compute_deployment_path_with_tags(&track, &tag_map)
            } else {
                // Fallback: no track_id, use empty path (shouldn't happen)
                std::path::PathBuf::new()
            };

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
    read_only_db: &Database,
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
