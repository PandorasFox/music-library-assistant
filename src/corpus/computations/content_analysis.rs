//! Content analysis computations - duplicate detection and tag analysis.
//!
//! These computations run during the second awakening stage to identify:
//! - Fingerprint duplicates (identical acoustic fingerprints)
//! - Duplicate inodes (hard links or duplicate index entries)
//! - Missing required tags
//! - Metadata duplicates (tracks with identical tag sets)
//! - Tag canonicalization opportunities (similar tag values)
//! - Out-of-band changes (files modified outside of MLA)
//! - Deploy conflicts (tracks that would deploy to the same path)

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use rusqlite::params;

use crate::config::log_message;
use crate::corpus::db::types::{AggregateSignal, AggregateSignalType, FileSignalType, HealthIssueType};
use crate::corpus::db::Database;

use super::helpers::{ensure_file_signal_if_missing, get_signal_sender_or_fail, parse_track_ids_csv};
use super::types::{Computation, ComputationResult, ComputationWitness};

// ============================================================================
// Schedule Content Analysis
// ============================================================================

/// Execute ScheduleContentAnalysis - spawns all content detection computations in parallel.
pub(super) fn execute_schedule_content_analysis(
    start: Instant,
) -> ComputationResult {
    let _ = log_message("[COMPUTE] ScheduleContentAnalysis: spawning all detection computations");

    let spawn = vec![
        Computation::DetectFingerprintDuplicates,
        Computation::DetectDuplicateInodes,
        Computation::DetectMissingTags,
        Computation::DetectMetadataDuplicates,
        Computation::DetectTagCanonicalizations,
        Computation::VerifyOutOfBandChanges,
        Computation::DetectDeployConflicts,
    ];

    ComputationResult::success(
        Computation::ScheduleContentAnalysis,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}

// ============================================================================
// Fingerprint Duplicate Detection
// ============================================================================

/// Execute DetectFingerprintDuplicates - bulk detection of fingerprint duplicates.
pub(super) fn execute_detect_fingerprint_duplicates(
    db: &Database,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::DetectFingerprintDuplicates;

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // First, clear stale FingerprintDuplicate signals
    // (fingerprints that no longer have duplicates)
    let clear_result = db.conn.execute(
        "DELETE FROM health_issues WHERE issue_type = 'fingerprint_dup'
         AND issue_key NOT IN (
             SELECT fingerprint FROM tracks
             WHERE fingerprint IS NOT NULL
             GROUP BY fingerprint HAVING COUNT(*) > 1
         )",
        params![],
    );
    if let Err(e) = clear_result {
        let _ = log_message(&format!(
            "[COMPUTE] DetectFingerprintDuplicates: error clearing stale signals: {}",
            e
        ));
    }

    // Find all fingerprints with duplicates, including track IDs
    let query = "SELECT fingerprint, GROUP_CONCAT(id) as track_ids
                 FROM tracks
                 WHERE fingerprint IS NOT NULL
                 GROUP BY fingerprint
                 HAVING COUNT(*) > 1";

    let mut stmt = match db.conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            return ComputationResult::failure(
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
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to execute query: {}", e),
            );
        }
    };

    let mut total_groups = 0;
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

        // Parse track IDs from comma-separated string
        let track_ids = parse_track_ids_csv(&track_ids_str);

        total_groups += 1;
        total_tracks += track_ids.len();

        // Create/replace signal with embedded track IDs
        let signal = AggregateSignal {
            id: None,
            signal_type: AggregateSignalType::FingerprintDuplicate,
            key: fingerprint.clone(),
            discovered_at: None,
            metadata_json: Some(serde_json::json!({
                "fingerprint": fingerprint,
            }).to_string()),
        }
        .with_track_ids(&track_ids);

        sender.replace_aggregate_signal(signal, witness);
    }

    let _ = log_message(&format!(
        "[COMPUTE] DetectFingerprintDuplicates: {} groups, {} tracks total",
        total_groups, total_tracks
    ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Duplicate Inode Detection
// ============================================================================

/// Execute DetectDuplicateInodes - bulk detection of duplicate inodes.
pub(super) fn execute_detect_duplicate_inodes(
    db: &Database,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::DetectDuplicateInodes;

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // First, clear stale DuplicateInode signals
    let clear_result = db.conn.execute(
        "DELETE FROM health_issues WHERE issue_type = 'duplicate_inode'
         AND issue_key NOT IN (
             SELECT CAST(inode AS TEXT) FROM tracks
             GROUP BY inode HAVING COUNT(*) > 1
         )",
        params![],
    );
    if let Err(e) = clear_result {
        let _ = log_message(&format!(
            "[COMPUTE] DetectDuplicateInodes: error clearing stale signals: {}",
            e
        ));
    }

    // Find all inodes with duplicates, including track IDs
    let query = "SELECT inode, GROUP_CONCAT(id) as track_ids
                 FROM tracks
                 GROUP BY inode
                 HAVING COUNT(*) > 1";

    let mut stmt = match db.conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            return ComputationResult::failure(
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
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to execute query: {}", e),
            );
        }
    };

    let mut total_groups = 0;

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

        // Parse track IDs from comma-separated string
        let track_ids = parse_track_ids_csv(&track_ids_str);

        total_groups += 1;

        // Create/replace signal with embedded track IDs
        let signal = AggregateSignal {
            id: None,
            signal_type: AggregateSignalType::DuplicateInode,
            key: inode.to_string(),
            discovered_at: None,
            metadata_json: Some(serde_json::json!({
                "inode": inode,
            }).to_string()),
        }
        .with_track_ids(&track_ids);

        sender.replace_aggregate_signal(signal, witness);
    }

    let _ = log_message(&format!(
        "[COMPUTE] DetectDuplicateInodes: {} duplicate inode groups",
        total_groups
    ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Missing Tags Detection
// ============================================================================

/// Execute DetectMissingTags - detect tracks missing required tags.
///
/// Groups tracks by album (or directory if no album tag), listing which tags
/// are missing in the metadata. Key format: `album=AlbumName` or `dir=/path`.
pub(super) fn execute_detect_missing_tags(
    db: &Database,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    use std::collections::HashSet;

    let computation = Computation::DetectMissingTags;

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // Load required tags from config
    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(e) => {
            return ComputationResult::failure(
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

    // Clear all existing MissingTag signals (we'll rebuild)
    let _ = db.conn.execute(
        "DELETE FROM health_issues WHERE issue_type = 'missing_tag'",
        params![],
    );

    // Get all tracks with their tags and paths
    // We need: track_id, path, album (if present), all tag names present
    let query = "
        SELECT t.id, t.path,
               (SELECT tag_value FROM track_tags WHERE track_id = t.id AND LOWER(tag_name) = 'album' LIMIT 1) as album,
               GROUP_CONCAT(LOWER(tt.tag_name), ',') as present_tags
        FROM tracks t
        LEFT JOIN track_tags tt ON t.id = tt.track_id
        GROUP BY t.id
    ";

    let mut stmt = match db.conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to prepare query: {}", e),
            );
        }
    };

    // Group by album (or directory), accumulating which tags are missing
    // Key: album=value or dir=/path
    // Value: (missing_tags set, track_ids)
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
            return ComputationResult::failure(
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

        // Parse present tags
        let present_tags: HashSet<String> = present_tags_str
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        // Find missing required tags
        let missing: HashSet<String> = required_tags
            .difference(&present_tags)
            .cloned()
            .collect();

        if missing.is_empty() {
            continue;
        }

        // Determine grouping key: album=value or dir=/path
        let key = if let Some(album_name) = album {
            format!("album={}", album_name)
        } else {
            // Use parent directory as fallback
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

    // Create signals for each group
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

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Metadata Duplicate Detection
// ============================================================================

/// Execute DetectMetadataDuplicates - detect exact metadata duplicates.
///
/// Groups tracks by their FULL tag signature (all tags, sorted by name).
/// Two tracks are duplicates if ALL their tags match exactly.
pub(super) fn execute_detect_metadata_duplicates(
    db: &Database,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    let computation = Computation::DetectMetadataDuplicates;

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // First, clear all MetadataDuplicate signals (we'll rebuild)
    let _ = db.conn.execute(
        "DELETE FROM health_issues WHERE issue_type = 'metadata_dup'",
        params![],
    );

    // Build tag signature for each track
    // GROUP_CONCAT with ORDER BY ensures consistent ordering
    let query = "
        SELECT t.id, tt.tag_name, tt.tag_value
        FROM tracks t
        JOIN track_tags tt ON t.id = tt.track_id
        ORDER BY t.id, LOWER(tt.tag_name)
    ";

    let mut stmt = match db.conn.prepare(query) {
        Ok(s) => s,
        Err(e) => {
            return ComputationResult::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to prepare query: {}", e),
            );
        }
    };

    // Build track_id -> sorted tag signature map
    let mut track_tags: HashMap<i64, Vec<(String, String)>> = HashMap::new();

    let rows = match stmt.query_map(params![], |row| {
        let track_id: i64 = row.get(0)?;
        let tag_name: String = row.get(1)?;
        let tag_value: String = row.get(2)?;
        Ok((track_id, tag_name, tag_value))
    }) {
        Ok(r) => r,
        Err(e) => {
            return ComputationResult::failure(
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

    // Build signature -> track_ids map
    let mut sig_to_tracks: HashMap<String, Vec<i64>> = HashMap::new();

    for (track_id, mut tags) in track_tags {
        // Sort by tag name for consistent signature
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

    // Create signals for duplicates
    let mut total_groups = 0;

    for (signature, track_ids) in sig_to_tracks {
        if track_ids.len() < 2 {
            continue;
        }

        total_groups += 1;

        // Create/replace signal with embedded track IDs
        // The key is a hash of the signature to keep it manageable
        // (full signatures can be very long)
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

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

/// Simple hash function for signature strings.
pub(super) fn md5_hash(s: &str) -> u64 {
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
pub(super) fn execute_detect_tag_canonicalizations(
    db: &Database,
    start: Instant,
) -> ComputationResult {
    use crate::corpus::health::canonicalization::detect_and_store_canonicalizations;

    let computation = Computation::DetectTagCanonicalizations;

    match detect_and_store_canonicalizations(db) {
        Ok(count) => {
            let _ = log_message(&format!(
                "[COMPUTE] DetectTagCanonicalizations: {} new canonicalization entries",
                count
            ));
            ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
        }
        Err(e) => ComputationResult::failure(
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
pub(super) fn execute_verify_out_of_band_changes(
    db: &Database,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    use crate::corpus::mutations::indexing::execute_verify_tags;
    use std::os::unix::fs::MetadataExt;

    let computation = Computation::VerifyOutOfBandChanges;

    // Get signal sender for async writes
    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // Get all CorpusFileModifiedOutOfBand signals
    let oob_signals = match db.get_health_signals(Some(HealthIssueType::CorpusFileModifiedOutOfBand)) {
        Ok(signals) => signals,
        Err(e) => {
            return ComputationResult::failure(
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
        // The issue_key is the file path
        let path = &signal.issue_key;

        // Find the track for this path
        let track = match db.get_track_by_path(path) {
            Ok(Some(t)) => t,
            Ok(None) => {
                // Track no longer exists - clear the signal
                sender.clear_file_signal(FileSignalType::CorpusFileModifiedOutOfBand, path, witness);
                continue;
            }
            Err(_) => continue,
        };

        let track_id = match track.id {
            Some(id) => id,
            None => continue,
        };

        // Run tag verification
        if let Err(e) = execute_verify_tags(db, track_id, Path::new(path)) {
            let _ = log_message(&format!(
                "[COMPUTE] VerifyOutOfBandChanges: error verifying {}: {}",
                path, e
            ));
            continue;
        }

        verified_count += 1;

        // Check if there are any tag mismatches for this track
        let mismatch_count: i64 = db.conn
            .query_row(
                "SELECT COUNT(*) FROM tag_mismatches WHERE track_id = ?1",
                params![track_id],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if mismatch_count > 0 {
            // Tags differ - create OutOfBandTagChange signal
            tag_change_count += 1;
            ensure_file_signal_if_missing(db, &sender, FileSignalType::OutOfBandTagChange, path, witness);
        } else {
            // Tags match but mtime changed - file was touched but unchanged
            mtime_only_count += 1;
            // Clear the CorpusFileModifiedOutOfBand signal
            sender.clear_file_signal(FileSignalType::CorpusFileModifiedOutOfBand, path, witness);

            // Update scan_state to current mtime to avoid future toil
            // (This is a minor mutation but acceptable in computation context for bookkeeping)
            let file_path = Path::new(path);
            if let Ok(metadata) = std::fs::metadata(file_path) {
                let mtime_secs = metadata.mtime();
                let mtime_nanos = metadata.mtime_nsec() as i64;

                let _ = db.conn.execute(
                    "UPDATE scan_state SET mtime_secs = ?1, mtime_nanos = ?2 WHERE path = ?3",
                    params![mtime_secs, mtime_nanos, path],
                );
            }
        }
    }

    let _ = log_message(&format!(
        "[COMPUTE] VerifyOutOfBandChanges: verified {} files, {} tag changes, {} mtime-only",
        verified_count, tag_change_count, mtime_only_count
    ));

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Deploy Conflict Detection
// ============================================================================

/// Execute DetectDeployConflicts - bulk detection of deploy path collisions.
///
/// Groups healthy tracks by their computed deployment path. Tracks that would
/// deploy to the same path are marked as DeployConflict signals.
pub(super) fn execute_detect_deploy_conflicts(
    db: &Database,
    witness: &ComputationWitness,
    start: Instant,
) -> ComputationResult {
    use crate::corpus::deploy::compute_deployment_path_with_tags;

    let computation = Computation::DetectDeployConflicts;

    let sender = match get_signal_sender_or_fail(computation.clone(), start) {
        Ok(s) => s,
        Err(result) => return result,
    };

    // Clear all existing DeployConflict signals (we rebuild from scratch)
    let _ = db.conn.execute(
        "DELETE FROM health_issues WHERE issue_type = 'deploy_conflict'",
        params![],
    );

    // Get all HealthyFile signals
    let healthy_signals = db
        .get_health_signals(Some(HealthIssueType::HealthyFile))
        .unwrap_or_default();

    // Compute deployment paths and group by path
    let mut deploy_path_to_tracks: HashMap<String, Vec<i64>> = HashMap::new();

    for signal in &healthy_signals {
        let path = &signal.issue_key;
        if let Ok(Some(track)) = db.get_track_by_path(path) {
            if let Some(track_id) = track.id {
                let tags = db.get_track_tags(track_id).unwrap_or_default();
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

    // Create signals for conflicts (paths with multiple tracks)
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

    ComputationResult::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
