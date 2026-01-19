//! One-Dimensional Insight Computations
//!
//! These insights aggregate a single signal type and can be computed
//! immediately (synchronously) from the database and health_issues table.
//!
//! ## Data Sources
//!
//! Insights are derived from:
//! - `health_issues` table: MissingFromDisk, MissingFromIndex, FileRelocated, etc.
//! - `tag_canonicalization` table: Unconfirmed tag variants
//! - Direct database queries: Track counts, pending operations

use crate::corpus::db::types::HealthIssueType;
use crate::corpus::db::Database;

use super::Insight;

/// Compute all one-dimensional insights from current state.
///
/// This is called synchronously when the insights view opens.
/// Queries health_issues table and other database state directly.
///
/// # Arguments
///
/// * `db` - Database connection for querying signals
///
/// # Returns
///
/// Vector of computed insights, unsorted (caller should sort by priority).
pub fn compute_one_dim_insights(db: &Database) -> Vec<Insight> {
    let mut insights = Vec::new();

    // =========================================================================
    // Corpus Health (always included, shown at bottom)
    // =========================================================================
    insights.push(compute_corpus_health(db));

    // =========================================================================
    // Index Desync (from health_issues table)
    // =========================================================================
    if let Some(insight) = compute_index_desync(db) {
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

/// Compute corpus health summary from health_issues table.
fn compute_corpus_health(db: &Database) -> Insight {
    let total_tracks = db.get_track_count(Some("corpus")).unwrap_or(0);

    // Check health_issues for various problem types
    let missing_from_disk = db
        .get_unresolved_health_issues(Some(HealthIssueType::MissingFromDisk))
        .unwrap_or_default()
        .len();
    let relocated = db
        .get_unresolved_health_issues(Some(HealthIssueType::FileRelocated))
        .unwrap_or_default()
        .len();
    let duplicate_inodes = db
        .get_unresolved_health_issues(Some(HealthIssueType::DuplicateInode))
        .unwrap_or_default()
        .len();

    // Corpus is healthy if no structural issues
    let indexed_healthy = missing_from_disk == 0 && relocated == 0 && duplicate_inodes == 0;

    // Check for out-of-band tag changes
    let tag_changes = db
        .get_unresolved_health_issues(Some(HealthIssueType::OutOfBandTagChange))
        .unwrap_or_default()
        .len();
    let tags_synced = tag_changes == 0;

    // Libraries health: check for deployment conflicts and stale deployments
    let deploy_conflicts = db
        .get_unresolved_health_issues(Some(HealthIssueType::DeployConflict))
        .unwrap_or_default()
        .len();
    let libraries_healthy = deploy_conflicts == 0;

    Insight::CorpusHealth {
        total_tracks,
        indexed_healthy,
        libraries_healthy,
        tags_synced,
    }
}

/// Compute index desync insight from health_issues table.
///
/// Counts files (not issues) for:
/// - MissingFromDisk: Indexed files not found on disk
/// - MissingFromIndex: Disk files not in index
/// - FileRelocated: Files moved to new paths (same inode)
///
/// Each health issue groups files by directory and stores file_count in metadata.
fn compute_index_desync(db: &Database) -> Option<Insight> {
    // Sum file_count from each issue's metadata (issues are grouped by directory)
    let missing_from_disk = sum_file_count_from_issues(
        &db.get_unresolved_health_issues(Some(HealthIssueType::MissingFromDisk))
            .unwrap_or_default(),
    );
    let missing_from_index = sum_file_count_from_issues(
        &db.get_unresolved_health_issues(Some(HealthIssueType::MissingFromIndex))
            .unwrap_or_default(),
    );
    let relocated = sum_file_count_from_issues(
        &db.get_unresolved_health_issues(Some(HealthIssueType::FileRelocated))
            .unwrap_or_default(),
    );

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

/// Sum file_count from health issue metadata.
///
/// Each issue stores metadata_json with {"file_count": N, ...}.
/// Returns the sum of all file_count values.
fn sum_file_count_from_issues(issues: &[crate::corpus::db::types::HealthIssue]) -> usize {
    issues
        .iter()
        .filter_map(|issue| {
            issue
                .metadata_json
                .as_ref()
                .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
                .and_then(|v| v.get("file_count")?.as_u64())
        })
        .sum::<u64>() as usize
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

    // Note: These tests now require a database connection since one_dim insights
    // query health_issues directly. Integration tests should be added that:
    // 1. Create in-memory database
    // 2. Insert health_issues records
    // 3. Verify compute_one_dim_insights returns expected insights

    #[test]
    fn test_insight_priority_ordering() {
        // Test that insights sort correctly by priority
        let mut insights = vec![
            Insight::CorpusHealth {
                total_tracks: 100,
                indexed_healthy: true,
                libraries_healthy: true,
                tags_synced: true,
            },
            Insight::IndexDesync {
                missing_from_disk: 5,
                missing_from_index: 3,
                relocated: 0,
            },
        ];

        insights.sort_by(|a, b| b.priority().cmp(&a.priority()));

        // IndexDesync (priority 100) should come before CorpusHealth (priority 0)
        assert!(matches!(insights[0], Insight::IndexDesync { .. }));
        assert!(matches!(insights[1], Insight::CorpusHealth { .. }));
    }
}
