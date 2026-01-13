//! Duplicate group operations.

use anyhow::Result;
use rusqlite::params;

use super::Database;
use crate::corpus::db::types::Track;

impl Database {
    // ========================================================================
    // Duplicate Group Operations
    // ========================================================================

    pub fn get_unresolved_duplicate_groups(&self) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM duplicate_groups
             WHERE resolution_state = 'pending'
             ORDER BY id",
        )?;

        let group_ids = stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;

        Ok(group_ids)
    }

    pub fn get_duplicate_group_tracks(&self, group_id: i64) -> Result<Vec<Track>> {
        // TODO: This query pattern (17-column SELECT for row_to_track) is duplicated across
        // multiple files. Consider extracting a constant or helper for the column list.
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.path, t.source, t.inode, t.file_size, t.file_type,
                    t.artist, t.album, t.album_artist, t.title, t.track_number, t.genre,
                    t.duration_ms, t.bitrate_kbps, t.sample_rate, t.fingerprint, t.isrc
             FROM tracks t
             INNER JOIN duplicate_group_members dgm ON t.id = dgm.track_id
             WHERE dgm.group_id = ?1
             ORDER BY dgm.id",
        )?;

        let tracks = stmt
            .query_map(params![group_id], Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<Track>>>()?;

        Ok(tracks)
    }

    pub fn mark_duplicate_group_resolved(&self, group_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE duplicate_groups
             SET resolution_state = 'resolved',
                 resolved_at = CURRENT_TIMESTAMP
             WHERE id = ?1",
            params![group_id],
        )?;

        Ok(())
    }

    pub fn clear_pending_duplicate_groups(&self, group_type: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM duplicate_group_members
             WHERE group_id IN (
                 SELECT id FROM duplicate_groups
                 WHERE group_type = ?1 AND resolution_state = 'pending'
             )",
            params![group_type],
        )?;

        self.conn.execute(
            "DELETE FROM duplicate_groups
             WHERE group_type = ?1 AND resolution_state = 'pending'",
            params![group_type],
        )?;

        Ok(())
    }

    pub fn insert_duplicate_group(&self, group_type: &str, group_key: &str) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO duplicate_groups (group_type, group_key, resolution_state)
             VALUES (?1, ?2, 'pending')",
            params![group_type, group_key],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn insert_duplicate_group_member(&self, group_id: i64, track_id: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO duplicate_group_members (group_id, track_id, selected_for_keep)
             VALUES (?1, ?2, 0)",
            params![group_id, track_id],
        )?;

        Ok(())
    }
}
