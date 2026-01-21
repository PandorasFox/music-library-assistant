//! Awake-phase computation executors.
//!
//! These functions implement the actual logic for Awake computations.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use rusqlite::params;

use crate::config::log_message;
use crate::corpus::computations::helpers::{
    clear_file_signal_if_present, ensure_file_signal_if_missing,
    ensure_file_signal_with_metadata_if_missing, get_configured_library_names,
    parse_track_ids_csv, reconcile_aggregate_signals, ComputedAggregateSignal,
};
use crate::corpus::computations::types::ComputationWitness;
use crate::corpus::db::types::{AggregateSignal, AggregateSignalType, CorpusFileSignalType, LibraryFileSignalType, HealthIssueType};
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
    let _ = log_message("[COMPUTE] ScheduleContentAnalysis: spawning all detection computations");

    let mut spawn = vec![
        Computation::DetectFingerprintDuplicates,
        Computation::DetectDuplicateInodes,
        Computation::DetectMissingTags,
        Computation::DetectMetadataDuplicates,
        Computation::DetectTagCanonicalizations,
        Computation::VerifyOutOfBandChanges,
        Computation::DetectDeployConflicts,
        Computation::DeriveCorpusDeployStatus,
    ];

    // Also spawn DeriveDeployHealthSignals for each configured library
    // The scan data was stored in library_scan_state during Awakening phase
    if let Ok(config) = crate::config::load_config() {
        let library_names = get_configured_library_names(&config);
        for library_name in library_names {
            let library_root = config.libraries_root.join(&library_name);
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
    let query = "SELECT fingerprint, GROUP_CONCAT(id) as track_ids
                 FROM tracks
                 WHERE fingerprint IS NOT NULL
                 GROUP BY fingerprint
                 HAVING COUNT(*) > 1";

    let mut stmt = match read_only_db.conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to prepare query: {}", e),
            );
        }
    };

    let rows = match stmt.query_map(params![], |row| {
        let fingerprint: String = row.get(0)?;
        let track_ids_str: String = row.get(1)?;
        Ok((fingerprint, track_ids_str))
    }) {
        Ok(r) => r,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to execute query: {}", e),
            );
        }
    };

    // Build computed signals
    let mut computed = Vec::new();
    let mut total_tracks = 0;

    for row_result in rows {
        let (fingerprint, track_ids_str) = match row_result {
            Ok(r) => r,
            Err(e) => {
                let _ = log_message(&format!(
                    "[COMPUTE] DetectFingerprintDuplicates: row error: {}",
                    e
                ));
                continue;
            }
        };

        let track_ids = parse_track_ids_csv(&track_ids_str);
        total_tracks += track_ids.len();

        let metadata = serde_json::json!({
            "fingerprint": &fingerprint,
            "track_ids": &track_ids,
        })
        .to_string();

        computed.push(ComputedAggregateSignal {
            key: fingerprint,
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
    let _ = log_message(&format!(
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

    let query = "SELECT inode, GROUP_CONCAT(id) as track_ids
                 FROM tracks
                 GROUP BY inode
                 HAVING COUNT(*) > 1";

    let mut stmt = match read_only_db.conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to prepare query: {}", e),
            );
        }
    };

    let rows = match stmt.query_map(params![], |row| {
        let inode: i64 = row.get(0)?;
        let track_ids_str: String = row.get(1)?;
        Ok((inode, track_ids_str))
    }) {
        Ok(r) => r,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to execute query: {}", e),
            );
        }
    };

    // Build computed signals
    let mut computed = Vec::new();

    for row_result in rows {
        let (inode, track_ids_str) = match row_result {
            Ok(r) => r,
            Err(e) => {
                let _ = log_message(&format!(
                    "[COMPUTE] DetectDuplicateInodes: row error: {}",
                    e
                ));
                continue;
            }
        };

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
    let _ = log_message(&format!(
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
    sender.clear_health_issues_by_type(HealthIssueType::MissingTag, witness);

    let query = "
        SELECT t.id, t.path,
               (SELECT tag_value FROM track_tags WHERE track_id = t.id AND LOWER(tag_name) = 'album' LIMIT 1) as album,
               GROUP_CONCAT(LOWER(tt.tag_name), ',') as present_tags
        FROM tracks t
        LEFT JOIN track_tags tt ON t.id = tt.track_id
        GROUP BY t.id
    ";

    let mut stmt = match read_only_db.conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to prepare query: {}", e),
            );
        }
    };

    let mut groups: HashMap<String, (HashSet<String>, Vec<i64>)> = HashMap::new();

    let rows = match stmt.query_map(params![], |row| {
        let track_id: i64 = row.get(0)?;
        let path: String = row.get(1)?;
        let album: Option<String> = row.get(2)?;
        let present_tags: Option<String> = row.get(3)?;
        Ok((track_id, path, album, present_tags))
    }) {
        Ok(r) => r,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to execute query: {}", e),
            );
        }
    };

    for row_result in rows {
        let (track_id, path, album, present_tags_str) = match row_result {
            Ok(r) => r,
            Err(_) => continue,
        };

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

    let _ = log_message(&format!(
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
    sender.clear_health_issues_by_type(HealthIssueType::MetadataDuplicate, witness);

    let query = "
        SELECT t.id, tt.tag_name, tt.tag_value
        FROM tracks t
        JOIN track_tags tt ON t.id = tt.track_id
        ORDER BY t.id, LOWER(tt.tag_name)
    ";

    let mut stmt = match read_only_db.conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to prepare query: {}", e),
            );
        }
    };

    let mut track_tags: HashMap<i64, Vec<(String, String)>> = HashMap::new();

    let rows = match stmt.query_map(params![], |row| {
        let track_id: i64 = row.get(0)?;
        let tag_name: String = row.get(1)?;
        let tag_value: String = row.get(2)?;
        Ok((track_id, tag_name, tag_value))
    }) {
        Ok(r) => r,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to execute query: {}", e),
            );
        }
    };

    for row_result in rows {
        let (track_id, tag_name, tag_value) = match row_result {
            Ok(r) => r,
            Err(_) => continue,
        };
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

    let _ = log_message(&format!(
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
pub fn execute_detect_tag_canonicalizations(
    read_only_db: &Database,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::health::canonicalization::detect_canonicalizations;

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

    match detect_canonicalizations(read_only_db) {
        Ok(canonicalizations) => {
            let count = canonicalizations.len();
            // Store canonicalizations via db_thread
            for canon in canonicalizations {
                sender.upsert_tag_canonicalization(
                    &canon.tag_name,
                    &canon.canonical_value,
                    &canon.variant_value,
                    canon.confidence,
                    witness,
                );
            }
            let _ = log_message(&format!(
                "[COMPUTE] DetectTagCanonicalizations: {} new canonicalization entries",
                count
            ));
            Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
        }
        Err(e) => Result::failure(
            computation,
            start.elapsed().as_millis() as u64,
            format!("Failed to detect canonicalizations: {}", e),
        ),
    }
}

// ============================================================================
// Out-of-Band Change Verification
// ============================================================================

/// Execute VerifyOutOfBandChanges - verify files with modified mtime.
pub fn execute_verify_out_of_band_changes(
    read_only_db: &Database,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    use crate::corpus::mutations::indexing::execute_verify_tags;
    use std::os::unix::fs::MetadataExt;

    let computation = Computation::VerifyOutOfBandChanges;

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

    let oob_signals = match read_only_db.get_health_signals(Some(HealthIssueType::CorpusFileModifiedOutOfBand)) {
        Ok(signals) => signals,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to get OOB signals: {}", e),
            );
        }
    };

    let mut verified_count = 0;
    let mut tag_change_count = 0;
    let mut mtime_only_count = 0;

    for signal in oob_signals {
        let path = &signal.issue_key;

        let track = match read_only_db.get_track_by_path(path) {
            Ok(Some(t)) => t,
            Ok(None) => {
                sender.clear_file_signal(CorpusFileSignalType::CorpusFileModifiedOutOfBand.into(), path, witness);
                continue;
            }
            Err(_) => continue,
        };

        let track_id = match track.id {
            Some(id) => id,
            None => continue,
        };

        if let Err(e) = execute_verify_tags(read_only_db, track_id, Path::new(path)) {
            let _ = log_message(&format!(
                "[COMPUTE] VerifyOutOfBandChanges: error verifying {}: {}",
                path, e
            ));
            continue;
        }

        verified_count += 1;

        let mismatch_count: i64 = read_only_db.conn
            .query_row(
                "SELECT COUNT(*) FROM tag_mismatches WHERE track_id = ?1",
                params![track_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if mismatch_count > 0 {
            tag_change_count += 1;
            ensure_file_signal_if_missing(read_only_db, &sender, CorpusFileSignalType::OutOfBandTagChange.into(), path, witness);
        } else {
            mtime_only_count += 1;
            sender.clear_file_signal(CorpusFileSignalType::CorpusFileModifiedOutOfBand.into(), path, witness);

            let file_path = Path::new(path);
            if let Ok(metadata) = std::fs::metadata(file_path) {
                let mtime_secs = metadata.mtime();
                let mtime_nanos = metadata.mtime_nsec() as i64;

                // Routes through db_thread which has write access
                sender.update_scan_state_mtime(path, mtime_secs, mtime_nanos, witness);
            }
        }
    }

    let _ = log_message(&format!(
        "[COMPUTE] VerifyOutOfBandChanges: verified {} files, {} tag changes, {} mtime-only",
        verified_count, tag_change_count, mtime_only_count
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
    sender.clear_health_issues_by_type(HealthIssueType::DeployConflict, witness);

    let healthy_signals = read_only_db
        .get_health_signals(Some(HealthIssueType::HealthyFile))
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

    let _ = log_message(&format!(
        "[COMPUTE] DetectDeployConflicts: {} conflicts among {} healthy files",
        conflict_count,
        healthy_signals.len()
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Execute CheckDeployConflicts - per-track deploy conflict check.
///
/// Placeholder - actual implementation would check a single track's deploy path
/// against other tracks.
pub fn execute_check_deploy_conflicts(
    _read_only_db: &Database,
    track_id: i64,
    start: Instant,
) -> Result {
    let _ = log_message(&format!(
        "CheckDeployConflicts: checking track {} (TODO: implement per-track check)",
        track_id
    ));

    Result::success(
        Computation::CheckDeployConflicts { track_id },
        start.elapsed().as_millis() as u64,
        Vec::new(),
    )
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
            let _ = log_message(&format!(
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
            clear_file_signal_if_present(read_only_db, &sender, LibraryFileSignalType::LibraryLeftover.into(), &leftover_key, witness);

            // Check if stale and capture metadata for the signal
            let stale_metadata = if let Ok(Some(track)) = read_only_db.get_track_by_path(corpus_path) {
                if let Some(track_id) = track.id {
                    let tags = read_only_db.get_track_tags(track_id).unwrap_or_default();
                    let tag_map: std::collections::HashMap<String, String> = tags
                        .into_iter()
                        .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                        .collect();

                    let expected_relative = compute_deployment_path_with_tags(&track, &tag_map);
                    let expected_path = library_root.join(&expected_relative);

                    if library_path != &expected_path {
                        // Stale: store all needed info in metadata
                        Some(serde_json::json!({
                            "library_path": library_path.to_string_lossy(),
                            "expected_path": expected_path.to_string_lossy(),
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
                clear_file_signal_if_present(read_only_db, &sender, LibraryFileSignalType::LibraryStale.into(), &stale_key, witness);
            }
        } else {
            leftover_count += 1;
            ensure_file_signal_if_missing(read_only_db, &sender, LibraryFileSignalType::LibraryLeftover.into(), &leftover_key, witness);
            clear_file_signal_if_present(read_only_db, &sender, LibraryFileSignalType::LibraryStale.into(), &stale_key, witness);
        }
    }

    let _ = log_message(&format!(
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
        .get_health_signals(Some(HealthIssueType::HealthyFile))
        .unwrap_or_default();

    // Build set of deployed inodes from library_scan_state
    let deployed_inodes = read_only_db.get_all_library_scan_inodes().unwrap_or_default();

    // Get set of stale library paths (files in library but at wrong path)
    // LibraryStale keys have format "library_stale:{library_name}:{library_path}"
    let stale_signals = read_only_db
        .get_health_signals(Some(HealthIssueType::LibraryStale))
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
            clear_file_signal_if_present(
                read_only_db,
                &sender,
                LibraryFileSignalType::DeployReady.into(),
                corpus_path,
                witness,
            );
            clear_file_signal_if_present(
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
            clear_file_signal_if_present(
                read_only_db,
                &sender,
                LibraryFileSignalType::DeployReady.into(),
                corpus_path,
                witness,
            );
        } else {
            // File is not deployed (or deployed but stale)
            deploy_ready_count += 1;

            // Compute the deploy path for this track
            let deploy_path = if let Some(track_id) = track.id {
                // Get track tags and build tag_map
                let tags = read_only_db.get_track_tags(track_id).unwrap_or_default();
                let tag_map: HashMap<String, String> = tags
                    .into_iter()
                    .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                    .collect();
                // Compute relative deploy path and make absolute
                let relative_path = compute_deployment_path_with_tags(&track, &tag_map);
                config.libraries_root.join(&relative_path)
            } else {
                // Fallback: no track_id, use empty path (shouldn't happen)
                std::path::PathBuf::new()
            };

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
            clear_file_signal_if_present(
                read_only_db,
                &sender,
                LibraryFileSignalType::DeployedHealthy.into(),
                corpus_path,
                witness,
            );
        }
    }

    let _ = log_message(&format!(
        "[COMPUTE] DeriveCorpusDeployStatus: {} healthy files, {} deploy-ready, {} deployed-healthy, {} not configured",
        healthy_signals.len(),
        deploy_ready_count,
        deployed_healthy_count,
        skipped_not_configured,
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
