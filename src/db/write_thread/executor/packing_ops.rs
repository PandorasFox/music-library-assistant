//! Release packing pipeline operations (intermediate tables).

use rusqlite::params;

use crate::db::Database;

use super::super::types::*;

// ============================================================================
// Packing Pipeline Operations
// ============================================================================

/// Execute TruncatePackingTables: clear all packing intermediate tables.
pub(super) fn execute_truncate_packing_tables(db: &Database) -> anyhow::Result<()> {
    db.conn().execute_batch(
        "DELETE FROM release_packing_manifest; DELETE FROM release_packing_scores; DELETE FROM release_packing_candidates; DELETE FROM pending_acoustid_submissions; DELETE FROM signal_packed_release;"
    )?;
    Ok(())
}

/// Execute WritePackingManifest: write a batch of rows to release_packing_manifest.
pub(super) fn execute_write_packing_manifest(
    db: &Database,
    rows: &[(String, i32, String, String, i32)],
) -> anyhow::Result<()> {
    let mut stmt = db.conn().prepare(
        "INSERT OR REPLACE INTO release_packing_manifest (release_id, total_tracks, release_title, release_artist, media_count) VALUES (?1, ?2, ?3, ?4, ?5)"
    )?;
    for (release_id, total_tracks, title, artist, media_count) in rows {
        stmt.execute(params![release_id, total_tracks, title, artist, media_count])?;
    }
    Ok(())
}

/// Execute WritePackingScores: write a batch of scored candidates.
pub(super) fn execute_write_packing_scores(
    db: &Database,
    rows: &[PackingScoreRow],
) -> anyhow::Result<()> {
    let mut stmt = db.conn().prepare(
        "INSERT OR REPLACE INTO release_packing_scores \
         (release_id, inode, recording_id, medium_pos, track_pos, track_title, medium_format, track_number, score, score_breakdown, is_optimal, match_method, fingerprint_hex, raw_duration_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
    )?;
    for row in rows {
        stmt.execute(params![
            row.release_id,
            row.inode,
            row.recording_id,
            row.medium_pos,
            row.track_pos,
            row.track_title,
            row.medium_format,
            row.track_number,
            row.score,
            row.score_breakdown,
            row.is_optimal as i32,
            row.match_method,
            row.fingerprint_hex,
            row.raw_duration_ms,
        ])?;
    }
    Ok(())
}

/// Execute WritePackingCandidates: write a batch of candidate rows.
pub(super) fn execute_write_packing_candidates(
    db: &Database,
    rows: &[PackingCandidateRow],
) -> anyhow::Result<()> {
    let mut stmt = db.conn().prepare(
        "INSERT OR REPLACE INTO release_packing_candidates \
         (release_id, inode, recording_id, confidence, path, parent_dir, duration_ms, \
          tag_title, tag_artist, tag_album, tag_tracknumber, dir_file_count) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    for row in rows {
        stmt.execute(params![
            row.release_id,
            row.inode,
            row.recording_id,
            row.confidence,
            row.path,
            row.parent_dir,
            row.duration_ms,
            row.tag_title,
            row.tag_artist,
            row.tag_album,
            row.tag_tracknumber,
            row.dir_file_count,
        ])?;
    }
    Ok(())
}

/// Execute DeletePackingDataForRelease: remove candidates and scores for one release.
/// Used by the pinned warm path to clean stale data before writing fresh candidates.
pub(super) fn execute_delete_packing_data_for_release(
    db: &Database,
    release_id: &str,
) -> anyhow::Result<()> {
    db.conn().execute(
        "DELETE FROM release_packing_candidates WHERE release_id = ?1",
        params![release_id],
    )?;
    db.conn().execute(
        "DELETE FROM release_packing_scores WHERE release_id = ?1",
        params![release_id],
    )?;
    Ok(())
}

/// Execute WritePendingAcoustIdSubmissions: write pending submissions.
pub(super) fn execute_write_pending_acoustid_submissions(
    db: &Database,
    rows: &[PendingAcoustIdSubmission],
) -> anyhow::Result<()> {
    let mut stmt = db.conn().prepare(
        "INSERT OR REPLACE INTO pending_acoustid_submissions \
         (fingerprint, recording_id, duration_ms, source) \
         VALUES (?1, ?2, ?3, ?4)",
    )?;
    for row in rows {
        stmt.execute(params![
            row.fingerprint,
            row.recording_id,
            row.duration_ms,
            row.source,
        ])?;
    }
    Ok(())
}
