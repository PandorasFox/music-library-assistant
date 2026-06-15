//! External matching and MusicBrainz cache operations.

use rusqlite::params;

use crate::db::Database;

use super::index_ops::current_unix_secs;

// ============================================================================
// External Matching Operations
// ============================================================================

/// Execute InsertExternalMatch: insert a match from an external API.
pub(super) fn execute_insert_external_match(
    db: &Database,
    inode: i64,
    fingerprint: &[u8],
    source: i64,
    recording_id: &str,
    confidence: f64,
    fetched_at: i64,
) -> anyhow::Result<()> {
    db.conn().execute(
        r#"INSERT OR REPLACE INTO external_matches
           (inode, fingerprint, source, recording_id, confidence, fetched_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
        params![
            inode,
            fingerprint,
            source,
            recording_id,
            confidence,
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
///
/// For `mb_release_cache` writes, additionally parses `url-rels` from the
/// JSON and refreshes `mb_release_discogs_links` rows in the same
/// transaction. This keeps the MB→Discogs linkage in sync as cache rows
/// land, without requiring a separate periodic scan over every cached blob.
pub(super) fn execute_upsert_mb_cache(
    db: &Database,
    table: &str,
    id_col: &str,
    id: &str,
    raw_json: &[u8],
    fetched_at: i64,
) -> anyhow::Result<()> {
    let conn = db.conn();
    let tx = conn.unchecked_transaction()?;

    let sql = format!(
        "INSERT OR REPLACE INTO {table} ({id_col}, raw_json, fetched_at) VALUES (?1, ?2, ?3)"
    );
    tx.execute(&sql, params![id, raw_json, fetched_at])?;

    if table == "mb_release_cache" {
        // Drop and re-insert linkage rows for this MB release. We re-extract
        // every time so a corrected url-rels payload removes stale links.
        tx.execute(
            "DELETE FROM mb_release_discogs_links WHERE mb_release_id = ?1",
            params![id],
        )?;

        if let Ok(release) = mm_meta::external::musicbrainz::parse_release(raw_json) {
            if let Some(discogs_id) =
                mm_meta::external::musicbrainz::discogs_release_id_from_relations(
                    &release.relations,
                )
            {
                tx.execute(
                    "INSERT OR IGNORE INTO mb_release_discogs_links
                        (mb_release_id, discogs_release_id, discovered_at)
                     VALUES (?1, ?2, ?3)",
                    params![id, discogs_id, fetched_at],
                )?;
            }
        }
    }

    tx.commit()?;
    Ok(())
}

/// Execute UpsertDiscogsReleaseCache: persist a fetched Discogs release JSON blob.
pub(super) fn execute_upsert_discogs_release_cache(
    db: &Database,
    release_id: &str,
    raw_json: &[u8],
    fetched_at: i64,
) -> anyhow::Result<()> {
    db.conn().execute(
        "INSERT OR REPLACE INTO discogs_release_cache (release_id, raw_json, fetched_at)
         VALUES (?1, ?2, ?3)",
        params![release_id, raw_json, fetched_at],
    )?;
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

/// Execute UpsertDeezerIsrcCache: insert or replace a Deezer ISRC cache entry.
pub(super) fn execute_upsert_deezer_isrc_cache(
    db: &Database,
    isrc: &str,
    status: &str,
    deezer_album_id: Option<i64>,
    cover_url: Option<&str>,
    response_json: Option<&str>,
    fetched_at: i64,
) -> anyhow::Result<()> {
    db.conn().execute(
        r#"INSERT OR REPLACE INTO deezer_isrc_cache
           (isrc, status, deezer_album_id, cover_url, response_json, fetched_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6)"#,
        params![isrc, status, deezer_album_id, cover_url, response_json, fetched_at],
    )?;
    Ok(())
}
