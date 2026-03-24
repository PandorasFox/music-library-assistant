//! External matching and MusicBrainz cache operations.

use rusqlite::params;

use crate::db::Database;

use super::index_ops::current_unix_secs;

// ============================================================================
// External Matching Operations
// ============================================================================

/// Execute InsertExternalMatch: insert a match from an external API.
#[allow(clippy::too_many_arguments)] // args map 1:1 to SQL columns
pub(super) fn execute_insert_external_match(
    db: &Database,
    inode: i64,
    fingerprint: &[u8],
    source: i64,
    recording_id: &str,
    confidence: f64,
    raw_response: Option<&[u8]>,
    fetched_at: i64,
) -> anyhow::Result<()> {

    db.conn().execute(
        r#"INSERT OR REPLACE INTO external_matches
           (inode, fingerprint, source, recording_id, confidence, raw_response, fetched_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
        params![
            inode,
            fingerprint,
            source,
            recording_id,
            confidence,
            raw_response,
            fetched_at
        ],
    )?;

    Ok(())
}

/// Execute InsertExternalNoMatch: record that a fingerprint had no results.
pub(super) fn execute_insert_external_no_match(
    db: &Database,
    fingerprint: &[u8],
    source: i64,
    queried_at: i64,
) -> anyhow::Result<()> {

    db.conn().execute(
        r#"INSERT OR REPLACE INTO external_no_match
           (fingerprint, source, queried_at)
           VALUES (?1, ?2, ?3)"#,
        params![fingerprint, source, queried_at],
    )?;

    Ok(())
}

/// Execute UpsertExternalRetry: record a failed lookup for retry.
pub(super) fn execute_upsert_external_retry(
    db: &Database,
    inode: i64,
    fingerprint: &[u8],
    source: i64,
    error: &str,
) -> anyhow::Result<()> {
    let now = current_unix_secs();

    db.conn().execute(
        r#"INSERT INTO external_retry (inode, fingerprint, source, failed_at, error, retry_count)
           VALUES (?1, ?2, ?3, ?4, ?5, 1)
           ON CONFLICT(inode, source) DO UPDATE SET
               failed_at = excluded.failed_at,
               error = excluded.error,
               retry_count = retry_count + 1"#,
        params![inode, fingerprint, source, now, error],
    )?;

    Ok(())
}

/// Execute DeleteExternalRetry: remove retry entry after successful lookup.
pub(super) fn execute_delete_external_retry(db: &Database, inode: i64, source: i64) -> anyhow::Result<()> {

    db.conn().execute(
        "DELETE FROM external_retry WHERE inode = ?1 AND source = ?2",
        params![inode, source],
    )?;

    Ok(())
}

pub(super) fn execute_drop_external_match(db: &Database, inode: i64) -> anyhow::Result<()> {

    db.conn().execute(
        "DELETE FROM external_matches WHERE inode = ?1",
        params![inode],
    )?;

    Ok(())
}

/// Execute an MB cache upsert for any entity type.
pub(super) fn execute_upsert_mb_cache(
    db: &Database,
    table: &str,
    id_col: &str,
    id: &str,
    raw_json: &[u8],
    fetched_at: i64,
) -> anyhow::Result<()> {

    let sql = format!(
        "INSERT OR REPLACE INTO {table} ({id_col}, raw_json, fetched_at) VALUES (?1, ?2, ?3)"
    );
    db.conn().execute(&sql, params![id, raw_json, fetched_at])?;
    Ok(())
}

/// Execute InsertMbKnownEntity: persist a discovered MB entity for resumable fetching.
pub(super) fn execute_insert_mb_known_entity(
    db: &Database,
    mbid: &str,
    entity_type: &str,
    discovered_from: Option<&str>,
    discovered_at: i64,
) -> anyhow::Result<()> {

    db.conn().execute(
        r#"INSERT OR IGNORE INTO mb_known_entities
           (mbid, entity_type, discovered_from, discovered_at)
           VALUES (?1, ?2, ?3, ?4)"#,
        params![mbid, entity_type, discovered_from, discovered_at],
    )?;

    Ok(())
}

/// Execute UpsertCaaReleaseCache: insert or replace a CAA release cache entry.
pub(super) fn execute_upsert_caa_release_cache(
    db: &Database,
    release_id: &str,
    status: &str,
    response_json: Option<&str>,
    image_count: i64,
    fetched_at: i64,
) -> anyhow::Result<()> {
    db.conn().execute(
        r#"INSERT OR REPLACE INTO caa_release_cache
           (release_id, status, response_json, image_count, fetched_at)
           VALUES (?1, ?2, ?3, ?4, ?5)"#,
        params![release_id, status, response_json, image_count, fetched_at],
    )?;
    Ok(())
}
