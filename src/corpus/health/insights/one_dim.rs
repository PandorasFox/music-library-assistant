//! One-Dimensional Insight Computations
//!
//! These insights aggregate a single signal type and can be computed
//! immediately (synchronously) from the database and heartbeat result.
//!
//! ## Processing Order
//!
//! HeartbeatResult is processed in a specific order:
//! 1. Relocations first (same inode at new path)
//! 2. Then missing_from_disk / missing_from_index
//!
//! This ensures relocated files don't appear as both "missing" and "new".

use crate::corpus::db::types::HealthIssueType;
use crate::corpus::db::Database;
use crate::corpus::health::HeartbeatResult;

use super::Insight;

/// Compute all one-dimensional insights from current state.
///
/// This is called synchronously when the insights view opens.
/// HeartbeatResult should be cloned from the most recent heartbeat.
///
/// # Arguments
///
/// * `db` - Database connection for querying signals
/// * `heartbeat` - Most recent heartbeat result (cloned)
///
/// # Returns
///
/// Vector of computed insights, unsorted (caller should sort by priority).
pub fn compute_one_dim_insights(db: &Database, heartbeat: &HeartbeatResult) -> Vec<Insight> {
    let mut insights = Vec::new();

    // =========================================================================
    // Corpus Health (always included, shown at bottom)
    // =========================================================================
    insights.push(compute_corpus_health(db, heartbeat));

    // =========================================================================
    // Index Desync (from HeartbeatResult - already processed relocations first)
    // =========================================================================
    if let Some(insight) = compute_index_desync(heartbeat) {
        insights.push(insight);
    }

    // =========================================================================
    // Deployment Conflicts (from health_issues table)
    // =========================================================================
    if let Some(insight) = compute_deployment_conflicts(db) {
        insights.push(insight);
    }

    // =========================================================================
    // Out-of-Band Changes (from health_issues table)
    // =========================================================================
    if let Some(insight) = compute_out_of_band_changes(db) {
        insights.push(insight);
    }

    // =========================================================================
    // Tag Canonicalization (per field, from tag_canonicalization table)
    // =========================================================================
    insights.extend(compute_tag_canonicalizations(db));

    // =========================================================================
    // Missing Tags (from health_issues table)
    // =========================================================================
    insights.extend(compute_missing_tags(db));

    insights
}

/// Compute corpus health summary from heartbeat result.
fn compute_corpus_health(db: &Database, heartbeat: &HeartbeatResult) -> Insight {
    let total_tracks = db.get_track_count(Some("corpus")).unwrap_or(0);

    Insight::CorpusHealth {
        total_tracks,
        indexed_healthy: heartbeat.is_corpus_healthy(),
        libraries_healthy: heartbeat.are_libraries_healthy(),
        tags_synced: heartbeat.are_tags_synced(),
    }
}

/// Compute index desync insight from heartbeat result.
///
/// Note: HeartbeatResult has already processed relocations, so:
/// - `missing_from_disk` excludes files that were just relocated
/// - `new_on_disk` excludes files that were just relocated
fn compute_index_desync(heartbeat: &HeartbeatResult) -> Option<Insight> {
    let missing_from_disk = heartbeat.missing_from_disk;
    let missing_from_index = heartbeat.new_on_disk; // Files on disk not in index
    let relocated = heartbeat.files_relocated;

    // Only include if there's something to report
    if missing_from_disk == 0 && missing_from_index == 0 && relocated == 0 {
        return None;
    }

    Some(Insight::IndexDesync {
        missing_from_disk,
        missing_from_index,
        relocated,
    })
}

/// Compute deployment conflicts from health_issues table.
fn compute_deployment_conflicts(db: &Database) -> Option<Insight> {
    let issues = db
        .get_unresolved_health_issues(Some(HealthIssueType::DeployConflict))
        .unwrap_or_default();

    if issues.is_empty() {
        return None;
    }

    Some(Insight::DeploymentConflicts {
        count: issues.len(),
    })
}

/// Compute out-of-band changes from health_issues table.
fn compute_out_of_band_changes(db: &Database) -> Option<Insight> {
    let tag_changes = db
        .get_unresolved_health_issues(Some(HealthIssueType::OutOfBandTagChange))
        .unwrap_or_default()
        .len();

    let file_changes = db
        .get_unresolved_health_issues(Some(HealthIssueType::OutOfBandFileChange))
        .unwrap_or_default()
        .len();

    if tag_changes == 0 && file_changes == 0 {
        return None;
    }

    Some(Insight::OutOfBandChanges {
        tag_changes,
        file_changes,
    })
}

/// Compute tag canonicalization insights per field.
///
/// Queries the tag_canonicalization table for unconfirmed variants.
fn compute_tag_canonicalizations(db: &Database) -> Vec<Insight> {
    let mut insights = Vec::new();

    // Query unconfirmed canonicalizations grouped by tag_name
    // Each row: (tag_name, variant_count, affected_tracks_estimate)
    let results = db.conn.prepare(
        r#"
        SELECT tag_name, COUNT(DISTINCT variant_value), COUNT(*)
        FROM tag_canonicalization
        WHERE confirmed_at IS NULL
        GROUP BY tag_name
        "#,
    );

    if let Ok(mut stmt) = results {
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, usize>(1)?,
                row.get::<_, usize>(2)?,
            ))
        });

        if let Ok(rows) = rows {
            for row in rows.flatten() {
                let (field, variant_count, affected_tracks) = row;
                if variant_count > 0 {
                    insights.push(Insight::TagCanonicalization {
                        field,
                        variant_count,
                        affected_tracks,
                    });
                }
            }
        }
    }

    insights
}

/// Compute missing tags insights.
///
/// Groups missing tag issues by tag name.
fn compute_missing_tags(db: &Database) -> Vec<Insight> {
    let mut insights = Vec::new();

    let issues = db
        .get_unresolved_health_issues(Some(HealthIssueType::MissingTag))
        .unwrap_or_default();

    // Group by tag name (stored in metadata_json)
    let mut by_tag: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for issue in issues {
        // metadata_json contains {"tag_name": "album_artist"} or similar
        if let Some(ref json) = issue.metadata_json {
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json) {
                if let Some(tag_name) = parsed.get("tag_name").and_then(|v| v.as_str()) {
                    *by_tag.entry(tag_name.to_string()).or_default() += 1;
                }
            }
        }
    }

    for (tag_name, count) in by_tag {
        insights.push(Insight::MissingTags { tag_name, count });
    }

    insights
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn mock_healthy_heartbeat() -> HeartbeatResult {
        HeartbeatResult {
            indexed_count: 1000,
            disk_count: 1000,
            missing_from_disk: 0,
            new_on_disk: 0,
            files_scanned: 0,
            files_relocated: 0,
            duplicate_inodes: 0,
            pending_tag_flushes: 0,
            library_health: vec![],
            deployment_conflicts: 0,
            duration: Duration::from_millis(100),
        }
    }

    fn mock_unhealthy_heartbeat() -> HeartbeatResult {
        HeartbeatResult {
            indexed_count: 1000,
            disk_count: 995,
            missing_from_disk: 5,
            new_on_disk: 3,
            files_scanned: 0,
            files_relocated: 2,
            duplicate_inodes: 0,
            pending_tag_flushes: 1,
            library_health: vec![],
            deployment_conflicts: 2,
            duration: Duration::from_millis(150),
        }
    }

    #[test]
    fn test_compute_index_desync_healthy() {
        let heartbeat = mock_healthy_heartbeat();
        let insight = compute_index_desync(&heartbeat);
        assert!(insight.is_none());
    }

    #[test]
    fn test_compute_index_desync_unhealthy() {
        let heartbeat = mock_unhealthy_heartbeat();
        let insight = compute_index_desync(&heartbeat);
        assert!(insight.is_some());

        if let Some(Insight::IndexDesync {
            missing_from_disk,
            missing_from_index,
            relocated,
        }) = insight
        {
            assert_eq!(missing_from_disk, 5);
            assert_eq!(missing_from_index, 3);
            assert_eq!(relocated, 2);
        } else {
            panic!("Expected IndexDesync insight");
        }
    }
}
