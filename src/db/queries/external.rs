//! External matching queries.
//!
//! Read-only queries for external matching tables (external_matches,
//! external_no_match, external_retry). Used by the fetch thread to
//! determine which inodes need lookup.

use anyhow::Result;
use rusqlite::params;
use std::path::Path;

use super::Database;

/// A row from external_matches joined with files, for signal derivation.
pub struct ExternalMatchRow {
    pub inode: i64,
    pub recording_id: String,
    pub confidence: f64,
    pub raw_response: Option<Vec<u8>>,
    pub path: String,
}

/// A candidate inode for external lookup.
pub struct ExternalLookupCandidate {
    pub inode: i64,
    pub fingerprint: Vec<u32>,
    pub duration_secs: u32,
}

impl Database {
    /// Get external matches for corpus files, for signal derivation.
    ///
    /// Returns rows ordered by (inode, confidence DESC) so the caller can
    /// group by inode and take the first per group (highest confidence).
    pub fn get_external_matches_for_derivation(
        &self,
        source_key: i64,
    ) -> Result<Vec<ExternalMatchRow>> {
        let mut stmt = self.conn().prepare(
            r#"SELECT em.inode, em.recording_id, em.confidence, em.raw_response, f.path
               FROM external_matches em
               JOIN files f ON em.inode = f.inode
               WHERE f.zone = 'corpus' AND em.source = ?1
               ORDER BY em.inode, em.confidence DESC"#,
        )?;

        let rows = stmt.query_map(params![source_key], |row| {
            Ok(ExternalMatchRow {
                inode: row.get(0)?,
                recording_id: row.get(1)?,
                confidence: row.get(2)?,
                raw_response: row.get(3)?,
                path: row.get(4)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            if let Ok(r) = row {
                results.push(r);
            }
        }
        Ok(results)
    }

    /// Get corpus inodes needing external lookup for a given source.
    ///
    /// Returns inodes that:
    /// - Are corpus audio files with fingerprints
    /// - Have no existing match in external_matches for this source
    /// - Have no existing no-match in external_no_match for this source
    /// - Are in one of the eligible source directories
    ///
    /// Results are limited to `limit` rows for batch sizing.
    pub fn get_inodes_needing_lookup(
        &self,
        source_key: i64,
        eligible_dirs: &[&Path],
        limit: usize,
    ) -> Result<Vec<ExternalLookupCandidate>> {
        if eligible_dirs.is_empty() {
            return Ok(Vec::new());
        }

        // Build LIKE patterns for eligible directories
        let patterns: Vec<String> = eligible_dirs
            .iter()
            .map(|d| {
                let dir_str = format!("corpus/{}", d.display());
                let escaped = super::escape_like_wildcards(&dir_str);
                format!("{}/%", escaped)
            })
            .collect();

        // Build OR clause for directory matching
        let dir_conditions: Vec<String> = patterns
            .iter()
            .enumerate()
            .map(|(i, _)| format!("f.path LIKE ?{} ESCAPE '\\'", i + 3))
            .collect();
        let dir_clause = dir_conditions.join(" OR ");

        let sql = format!(
            r#"SELECT a.inode, a.fingerprint, a.duration_ms
               FROM audio_info a
               JOIN files f ON a.inode = f.inode
               WHERE f.zone = 'corpus'
                 AND a.fingerprint IS NOT NULL
                 AND a.duration_ms IS NOT NULL
                 AND ({dir_clause})
                 AND NOT EXISTS (
                     SELECT 1 FROM external_matches em
                     WHERE em.inode = a.inode AND em.source = ?1
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM external_no_match enm
                     WHERE enm.fingerprint = a.fingerprint AND enm.source = ?1
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM external_retry er
                     WHERE er.inode = a.inode AND er.source = ?1
                 )
               LIMIT ?2"#
        );

        let mut stmt = self.conn().prepare(&sql)?;

        // Bind source key and limit first, then directory patterns
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        param_values.push(Box::new(source_key));
        param_values.push(Box::new(limit as i64));
        for pattern in &patterns {
            param_values.push(Box::new(pattern.clone()));
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            let inode: i64 = row.get(0)?;
            let fp_blob: Vec<u8> = row.get(1)?;
            let duration_ms: i64 = row.get(2)?;

            // Convert BLOB back to Vec<u32> (little-endian)
            let fingerprint: Vec<u32> = fp_blob
                .chunks_exact(4)
                .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();

            Ok(ExternalLookupCandidate {
                inode,
                fingerprint,
                duration_secs: (duration_ms / 1000) as u32,
            })
        })?;

        let mut candidates = Vec::new();
        for row in rows {
            if let Ok(c) = row {
                candidates.push(c);
            }
        }

        Ok(candidates)
    }

    /// Get retry candidates for a given source.
    ///
    /// Returns inodes that previously failed lookup and need retry.
    pub fn get_retry_candidates(
        &self,
        source_key: i64,
        limit: usize,
    ) -> Result<Vec<ExternalLookupCandidate>> {
        let mut stmt = self.conn().prepare(
            r#"SELECT er.inode, er.fingerprint, a.duration_ms
               FROM external_retry er
               JOIN audio_info a ON er.inode = a.inode
               WHERE er.source = ?1
                 AND a.duration_ms IS NOT NULL
               ORDER BY er.retry_count ASC, er.failed_at ASC
               LIMIT ?2"#,
        )?;

        let rows = stmt.query_map(params![source_key, limit as i64], |row| {
            let inode: i64 = row.get(0)?;
            let fp_blob: Vec<u8> = row.get(1)?;
            let duration_ms: i64 = row.get(2)?;

            let fingerprint: Vec<u32> = fp_blob
                .chunks_exact(4)
                .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();

            Ok(ExternalLookupCandidate {
                inode,
                fingerprint,
                duration_secs: (duration_ms / 1000) as u32,
            })
        })?;

        let mut candidates = Vec::new();
        for row in rows {
            if let Ok(c) = row {
                candidates.push(c);
            }
        }

        Ok(candidates)
    }
}
