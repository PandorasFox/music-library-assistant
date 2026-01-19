//! Health signal and known variant operations.
//!
//! Signals are facts about corpus state. They are created by computations and
//! deleted when they become stale. There is no "resolution" concept - signals
//! simply exist or don't exist based on current corpus state.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use super::Database;
use crate::corpus::db::types::{
    CorpusSummary, HealthIssue, HealthIssueSeverity, HealthIssueType, HealthSummary,
    KnownVariant, Track, TrackRole, VariantType,
};

impl Database {
    // ========================================================================
    // Corpus Summary
    // ========================================================================

    /// Get aggregated corpus summary for UI display.
    pub fn get_corpus_summary(&self) -> Result<CorpusSummary> {
        let track_count = self.get_track_count(Some("corpus")).unwrap_or(0);

        // Count deploy conflict signals
        let deploy_conflicts: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM health_issues WHERE issue_type = 'deploy_conflict'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let health_summary = self.get_health_summary().unwrap_or_default();
        let deployment_stats = self.get_deployment_stats().ok().flatten();
        let pending_changes = std::collections::HashMap::new(); // Decisions now in-memory only

        // Get last scan time from scan_history
        let last_scan: Option<String> = self
            .conn
            .query_row(
                "SELECT completed_at FROM scan_history WHERE source = 'corpus' ORDER BY id DESC LIMIT 1",
                params![],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();

        Ok(CorpusSummary {
            track_count,
            deploy_conflicts,
            health_summary,
            deployment_stats,
            pending_changes,
            last_scan,
        })
    }

    // ========================================================================
    // Health Issue Operations
    // ========================================================================

    /// Insert a new health signal.
    pub fn insert_health_issue(&self, issue: &HealthIssue) -> Result<i64> {
        self.conn
            .execute(
                r#"
                INSERT INTO health_issues
                (issue_type, issue_key, severity, discovered_at, metadata_json)
                VALUES (?1, ?2, ?3, COALESCE(?4, CURRENT_TIMESTAMP), ?5)
                "#,
                params![
                    issue.issue_type.as_str(),
                    &issue.issue_key,
                    issue.severity.as_str(),
                    &issue.discovered_at,
                    &issue.metadata_json,
                ],
            )
            .context("Failed to insert health signal")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get health signals, optionally filtered by type.
    pub fn get_health_signals(
        &self,
        issue_type: Option<HealthIssueType>,
    ) -> Result<Vec<HealthIssue>> {
        let sql = match issue_type {
            Some(_) => {
                r#"SELECT id, issue_type, issue_key, severity, discovered_at, metadata_json
                   FROM health_issues
                   WHERE issue_type = ?1
                   ORDER BY discovered_at DESC"#
            }
            None => {
                r#"SELECT id, issue_type, issue_key, severity, discovered_at, metadata_json
                   FROM health_issues
                   ORDER BY discovered_at DESC"#
            }
        };

        let mut stmt = self.conn.prepare(sql)?;

        let rows = if let Some(it) = issue_type {
            stmt.query_map(params![it.as_str()], Self::row_to_health_issue)?
        } else {
            stmt.query_map(params![], Self::row_to_health_issue)?
        };

        let mut issues = Vec::new();
        for row in rows {
            issues.push(row?);
        }
        Ok(issues)
    }

    /// Backwards compatibility alias
    #[deprecated(note = "Use get_health_signals instead")]
    pub fn get_unresolved_health_issues(
        &self,
        issue_type: Option<HealthIssueType>,
    ) -> Result<Vec<HealthIssue>> {
        self.get_health_signals(issue_type)
    }

    /// Get health signal by type and key (e.g., path or fingerprint).
    pub fn get_health_issue_by_key(
        &self,
        issue_type: HealthIssueType,
        issue_key: &str,
    ) -> Result<Option<HealthIssue>> {
        self.conn
            .query_row(
                r#"SELECT id, issue_type, issue_key, severity, discovered_at, metadata_json
                   FROM health_issues
                   WHERE issue_type = ?1 AND issue_key = ?2"#,
                params![issue_type.as_str(), issue_key],
                Self::row_to_health_issue,
            )
            .optional()
            .context("Failed to query health signal by key")
    }

    /// Delete a health signal by ID.
    pub fn delete_health_signal(&self, signal_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM health_issues WHERE id = ?1", params![signal_id])
            .context("Failed to delete health signal")?;
        Ok(())
    }

    /// Delete health signals by type and key.
    pub fn delete_health_signals_by_key(
        &self,
        issue_type: HealthIssueType,
        issue_key: &str,
    ) -> Result<usize> {
        let deleted = self.conn
            .execute(
                "DELETE FROM health_issues WHERE issue_type = ?1 AND issue_key = ?2",
                params![issue_type.as_str(), issue_key],
            )
            .context("Failed to delete health signals")?;
        Ok(deleted)
    }

    /// Delete all signals of a given type for a specific path.
    pub fn delete_signals_for_path(&self, path: &str) -> Result<usize> {
        let deleted = self.conn
            .execute(
                "DELETE FROM health_issues WHERE issue_key = ?1",
                params![path],
            )
            .context("Failed to delete signals for path")?;
        Ok(deleted)
    }

    // ========================================================================
    // Directory-Level Queries (for chunked computations)
    // ========================================================================

    /// Get distinct parent directories from tracks table.
    pub fn get_distinct_track_directories(&self) -> Result<Vec<std::path::PathBuf>> {
        use std::path::PathBuf;

        // Fetch all paths and compute parent directories in Rust
        let mut stmt = self.conn.prepare("SELECT DISTINCT path FROM tracks")?;
        let rows = stmt.query_map(params![], |row| {
            let path: String = row.get(0)?;
            Ok(path)
        })?;

        let mut directories = std::collections::HashSet::new();
        for row in rows {
            let path = row?;
            if let Some(parent) = PathBuf::from(&path).parent() {
                directories.insert(parent.to_path_buf());
            }
        }

        Ok(directories.into_iter().collect())
    }

    /// Get distinct parent directories from FileInCorpus signals.
    pub fn get_distinct_corpus_directories(&self) -> Result<Vec<std::path::PathBuf>> {
        use std::path::PathBuf;

        // FileInCorpus signals use issue_key format "file_in_corpus:{directory}"
        let mut stmt = self.conn.prepare(
            r#"SELECT DISTINCT issue_key FROM health_issues
               WHERE issue_type = 'file_in_corpus'"#
        )?;

        let rows = stmt.query_map(params![], |row| {
            let key: String = row.get(0)?;
            Ok(key)
        })?;

        let mut directories = Vec::new();
        for row in rows {
            let key = row?;
            // Parse "file_in_corpus:{directory}" format
            if let Some(dir) = key.strip_prefix("file_in_corpus:") {
                directories.push(PathBuf::from(dir));
            }
        }

        Ok(directories)
    }

    /// Get signals in a specific directory.
    pub fn get_signals_in_directory(
        &self,
        dir: &std::path::Path,
        signal_type: HealthIssueType,
    ) -> Result<Vec<HealthIssue>> {
        let dir_str = dir.to_string_lossy();
        let pattern = format!("{}%", dir_str);

        let mut stmt = self.conn.prepare(
            r#"SELECT id, issue_type, issue_key, severity, discovered_at, metadata_json
               FROM health_issues
               WHERE issue_type = ?1 AND issue_key LIKE ?2"#
        )?;

        let rows = stmt.query_map(
            params![signal_type.as_str(), pattern],
            Self::row_to_health_issue
        )?;

        let mut issues = Vec::new();
        for row in rows {
            issues.push(row?);
        }
        Ok(issues)
    }

    // Note: get_tracks_in_directory is defined in tracks.rs

    /// Get signals discovered since a given timestamp.
    pub fn get_signals_since(
        &self,
        signal_type: HealthIssueType,
        since: &str,
    ) -> Result<Vec<HealthIssue>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, issue_type, issue_key, severity, discovered_at, metadata_json
               FROM health_issues
               WHERE issue_type = ?1 AND discovered_at > ?2
               ORDER BY discovered_at DESC"#
        )?;

        let rows = stmt.query_map(
            params![signal_type.as_str(), since],
            Self::row_to_health_issue
        )?;

        let mut issues = Vec::new();
        for row in rows {
            issues.push(row?);
        }
        Ok(issues)
    }

    /// Add a track to a health issue.
    pub fn add_health_issue_track(
        &self,
        issue_id: i64,
        track_id: i64,
        role: TrackRole,
    ) -> Result<i64> {
        self.conn
            .execute(
                r#"INSERT INTO health_issue_tracks (issue_id, track_id, role)
                   VALUES (?1, ?2, ?3)"#,
                params![issue_id, track_id, role.as_str()],
            )
            .context("Failed to add health issue track")?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Get tracks for a health issue.
    pub fn get_health_issue_tracks(&self, issue_id: i64) -> Result<Vec<(Track, TrackRole)>> {
        // TODO: This query pattern (17-column SELECT for row_to_track) is duplicated across
        // multiple files. Consider extracting a constant or helper for the column list.
        let mut stmt = self.conn.prepare(
            r#"SELECT t.id, t.path, t.source, t.inode, t.file_size, t.file_type,
                      t.artist, t.album, t.album_artist, t.title, t.track_number, t.genre,
                      t.duration_ms, t.bitrate_kbps, t.sample_rate, t.fingerprint, t.isrc,
                      hit.role
               FROM health_issue_tracks hit
               JOIN tracks t ON t.id = hit.track_id
               WHERE hit.issue_id = ?1"#,
        )?;

        let rows = stmt.query_map(params![issue_id], |row| {
            let track = Self::row_to_track(row)?;
            let role_str: String = row.get(17)?;
            let role = TrackRole::from_str(&role_str).unwrap_or(TrackRole::Member);
            Ok((track, role))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get health signals for a specific track.
    pub fn get_health_issues_for_track(&self, track_id: i64) -> Result<Vec<HealthIssue>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT hi.id, hi.issue_type, hi.issue_key, hi.severity, hi.discovered_at, hi.metadata_json
               FROM health_issues hi
               JOIN health_issue_tracks hit ON hi.id = hit.issue_id
               WHERE hit.track_id = ?1"#,
        )?;

        let rows = stmt.query_map(params![track_id], Self::row_to_health_issue)?;

        let mut issues = Vec::new();
        for row in rows {
            issues.push(row?);
        }
        Ok(issues)
    }

    /// Get health summary statistics.
    pub fn get_health_summary(&self) -> Result<HealthSummary> {
        let mut summary = HealthSummary::default();

        // Count by issue type
        let mut stmt = self.conn.prepare(
            r#"SELECT issue_type, severity, COUNT(*)
               FROM health_issues
               GROUP BY issue_type, severity"#,
        )?;

        let rows = stmt.query_map(params![], |row| {
            let issue_type: String = row.get(0)?;
            let severity: String = row.get(1)?;
            let count: i64 = row.get(2)?;
            Ok((issue_type, severity, count as usize))
        })?;

        for row in rows {
            let (issue_type, severity, count) = row?;

            match issue_type.as_str() {
                "fingerprint_dup" => summary.fingerprint_duplicates += count,
                "metadata_dup" => summary.metadata_duplicates += count,
                // Legacy and unified type both map to canonicalization_issues
                "canon" | "tag_canon" | "genre_canon" => summary.canonicalization_issues += count,
                "missing_tag" => summary.missing_tag_issues += count,
                "quality" => summary.quality_variants += count,
                _ => {}
            }

            match severity.as_str() {
                "auto_resolvable" => summary.auto_resolvable += count,
                "manual_review" => summary.manual_review += count,
                _ => {}
            }
        }

        // Count known variants
        summary.known_variants = self.conn.query_row(
            "SELECT COUNT(*) FROM known_variants",
            params![],
            |row| row.get(0),
        )?;

        // Count unconfirmed tag canonicalizations as canonicalization issues
        // These are stored in tag_canonicalization, not health_issues
        let unconfirmed_canons: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM tag_canonicalization WHERE confirmed_at IS NULL",
            params![],
            |row| row.get(0),
        )?;
        summary.canonicalization_issues += unconfirmed_canons;

        Ok(summary)
    }

    // ========================================================================
    // Known Variant Operations
    // ========================================================================

    /// Insert a known variant relationship.
    pub fn insert_known_variant(&self, variant: &KnownVariant) -> Result<i64> {
        self.conn
            .execute(
                r#"INSERT INTO known_variants
                   (variant_type, canonical_fingerprint, variant_fingerprint,
                    canonical_track_id, variant_track_id, marked_at, notes)
                   VALUES (?1, ?2, ?3, ?4, ?5, COALESCE(?6, CURRENT_TIMESTAMP), ?7)"#,
                params![
                    variant.variant_type.as_str(),
                    &variant.canonical_fingerprint,
                    &variant.variant_fingerprint,
                    variant.canonical_track_id,
                    variant.variant_track_id,
                    &variant.marked_at,
                    &variant.notes,
                ],
            )
            .context("Failed to insert known variant")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Check if a fingerprint is a known variant.
    pub fn is_known_variant(&self, fingerprint: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            r#"SELECT COUNT(*) FROM known_variants
               WHERE canonical_fingerprint = ?1 OR variant_fingerprint = ?1"#,
            params![fingerprint],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Get known variants for a fingerprint.
    pub fn get_known_variants_for_fingerprint(&self, fingerprint: &str) -> Result<Vec<KnownVariant>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, variant_type, canonical_fingerprint, variant_fingerprint,
                      canonical_track_id, variant_track_id, marked_at, notes
               FROM known_variants
               WHERE canonical_fingerprint = ?1 OR variant_fingerprint = ?1"#,
        )?;

        let rows = stmt.query_map(params![fingerprint], Self::row_to_known_variant)?;

        let mut variants = Vec::new();
        for row in rows {
            variants.push(row?);
        }
        Ok(variants)
    }

    /// Get all known variants.
    pub fn get_all_known_variants(&self) -> Result<Vec<KnownVariant>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, variant_type, canonical_fingerprint, variant_fingerprint,
                      canonical_track_id, variant_track_id, marked_at, notes
               FROM known_variants
               ORDER BY variant_type, canonical_fingerprint"#,
        )?;

        let rows = stmt.query_map(params![], Self::row_to_known_variant)?;

        let mut variants = Vec::new();
        for row in rows {
            variants.push(row?);
        }
        Ok(variants)
    }

    // ========================================================================
    // Row Conversion Helpers
    // ========================================================================

    /// Convert a row to HealthIssue.
    /// Expected columns: id, issue_type, issue_key, severity, discovered_at, metadata_json
    pub(super) fn row_to_health_issue(row: &rusqlite::Row) -> rusqlite::Result<HealthIssue> {
        let issue_type_str: String = row.get(1)?;
        let severity_str: String = row.get(3)?;

        Ok(HealthIssue {
            id: Some(row.get(0)?),
            issue_type: HealthIssueType::from_str(&issue_type_str)
                .unwrap_or(HealthIssueType::FingerprintDuplicate),
            issue_key: row.get(2)?,
            severity: HealthIssueSeverity::from_str(&severity_str)
                .unwrap_or(HealthIssueSeverity::ManualReview),
            discovered_at: row.get(4)?,
            metadata_json: row.get(5)?,
        })
    }

    pub(super) fn row_to_known_variant(row: &rusqlite::Row) -> rusqlite::Result<KnownVariant> {
        let variant_type_str: String = row.get(1)?;

        Ok(KnownVariant {
            id: Some(row.get(0)?),
            variant_type: VariantType::from_str(&variant_type_str)
                .unwrap_or(VariantType::Rerelease),
            canonical_fingerprint: row.get(2)?,
            variant_fingerprint: row.get(3)?,
            canonical_track_id: row.get(4)?,
            variant_track_id: row.get(5)?,
            marked_at: row.get(6)?,
            notes: row.get(7)?,
        })
    }
}
