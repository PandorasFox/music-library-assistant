//! Database operations and queries.
//!
//! All SQLite operations are centralized here.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

use super::changes::{ChangeSession, ChangeStatus, ChangeType, PendingChange};
use super::types::{DeploymentStats, ScanStateEntry, Track};

/// Central database connection wrapper.
pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).context("Failed to open database")?;

        let db = Database { conn };
        db.initialize_schema()?;
        db.migrate_add_fingerprint()?;
        db.migrate_add_isrc()?;
        Ok(db)
    }

    fn initialize_schema(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tracks (
                id INTEGER PRIMARY KEY,
                path TEXT NOT NULL UNIQUE,
                source TEXT NOT NULL,
                inode INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                file_type TEXT NOT NULL,
                artist TEXT,
                album TEXT,
                album_artist TEXT,
                title TEXT,
                track_number INTEGER,
                duration_ms INTEGER,
                bitrate_kbps INTEGER,
                sample_rate INTEGER,
                fingerprint TEXT,
                scanned_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_source ON tracks(source);
            CREATE INDEX IF NOT EXISTS idx_inode ON tracks(inode);
            CREATE INDEX IF NOT EXISTS idx_artist ON tracks(artist);
            CREATE INDEX IF NOT EXISTS idx_album ON tracks(album);
            CREATE INDEX IF NOT EXISTS idx_album_artist ON tracks(album_artist);
            CREATE INDEX IF NOT EXISTS idx_title ON tracks(title);
            CREATE INDEX IF NOT EXISTS idx_duration ON tracks(duration_ms);
            CREATE INDEX IF NOT EXISTS idx_fingerprint ON tracks(fingerprint);

            CREATE TABLE IF NOT EXISTS scan_history (
                id INTEGER PRIMARY KEY,
                source TEXT NOT NULL,
                file_count INTEGER NOT NULL,
                started_at DATETIME NOT NULL,
                completed_at DATETIME NOT NULL
            );

            CREATE TABLE IF NOT EXISTS scan_state (
                id INTEGER PRIMARY KEY,
                source TEXT NOT NULL,
                inode INTEGER NOT NULL,
                path TEXT NOT NULL,
                mtime_secs INTEGER NOT NULL,
                mtime_nanos INTEGER NOT NULL,
                file_size INTEGER NOT NULL,
                scanned_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                UNIQUE(source, inode)
            );

            CREATE INDEX IF NOT EXISTS idx_scan_state_source ON scan_state(source);
            CREATE INDEX IF NOT EXISTS idx_scan_state_inode ON scan_state(inode);
            CREATE INDEX IF NOT EXISTS idx_scan_state_path ON scan_state(path);

            CREATE TABLE IF NOT EXISTS deployment_log (
                id INTEGER PRIMARY KEY,
                library_name TEXT NOT NULL,
                corpus_path TEXT NOT NULL,
                deployed_path TEXT NOT NULL,
                inode INTEGER NOT NULL,
                deployed_at DATETIME DEFAULT CURRENT_TIMESTAMP
            );

            CREATE INDEX IF NOT EXISTS idx_deployment_library ON deployment_log(library_name);
            CREATE INDEX IF NOT EXISTS idx_deployment_inode ON deployment_log(inode);

            CREATE TABLE IF NOT EXISTS corpus_health_stats (
                id INTEGER PRIMARY KEY,
                stat_type TEXT NOT NULL,
                last_updated DATETIME DEFAULT CURRENT_TIMESTAMP,
                data_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_corpus_health_type ON corpus_health_stats(stat_type);

            CREATE TABLE IF NOT EXISTS duplicate_groups (
                id INTEGER PRIMARY KEY,
                group_type TEXT NOT NULL,
                group_key TEXT NOT NULL,
                resolution_state TEXT DEFAULT 'pending',
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                resolved_at DATETIME
            );
            CREATE INDEX IF NOT EXISTS idx_duplicate_group_type ON duplicate_groups(group_type);
            CREATE INDEX IF NOT EXISTS idx_duplicate_resolution ON duplicate_groups(resolution_state);

            CREATE TABLE IF NOT EXISTS duplicate_group_members (
                id INTEGER PRIMARY KEY,
                group_id INTEGER NOT NULL,
                track_id INTEGER NOT NULL,
                selected_for_keep BOOLEAN DEFAULT 0,
                FOREIGN KEY(group_id) REFERENCES duplicate_groups(id),
                FOREIGN KEY(track_id) REFERENCES tracks(id)
            );
            CREATE INDEX IF NOT EXISTS idx_duplicate_members_group ON duplicate_group_members(group_id);
            CREATE INDEX IF NOT EXISTS idx_duplicate_members_track ON duplicate_group_members(track_id);

            CREATE TABLE IF NOT EXISTS tag_edit_history (
                id INTEGER PRIMARY KEY,
                track_id INTEGER NOT NULL,
                field_name TEXT NOT NULL,
                old_value TEXT,
                new_value TEXT,
                edited_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                session_id TEXT,
                FOREIGN KEY(track_id) REFERENCES tracks(id)
            );
            CREATE INDEX IF NOT EXISTS idx_tag_history_track ON tag_edit_history(track_id);
            CREATE INDEX IF NOT EXISTS idx_tag_history_session ON tag_edit_history(session_id);

            -- Algebraic change tracking tables
            CREATE TABLE IF NOT EXISTS pending_changes (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL,
                change_type TEXT NOT NULL,
                source_path TEXT NOT NULL,
                target_path TEXT,
                metadata_changes TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                status TEXT DEFAULT 'pending'
            );
            CREATE INDEX IF NOT EXISTS idx_pending_changes_session ON pending_changes(session_id);
            CREATE INDEX IF NOT EXISTS idx_pending_changes_status ON pending_changes(status);
            CREATE INDEX IF NOT EXISTS idx_pending_changes_source ON pending_changes(source_path);

            CREATE TABLE IF NOT EXISTS change_sessions (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL UNIQUE,
                description TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                committed_at DATETIME,
                status TEXT DEFAULT 'active'
            );
            CREATE INDEX IF NOT EXISTS idx_change_sessions_status ON change_sessions(status);
            "#
        ).context("Failed to initialize database schema")?;

        Ok(())
    }

    /// Migrate existing databases to add fingerprint column
    fn migrate_add_fingerprint(&self) -> Result<()> {
        let has_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tracks') WHERE name='fingerprint'",
                params![],
                |row| {
                    let count: i64 = row.get(0)?;
                    Ok(count > 0)
                },
            )
            .unwrap_or(false);

        if !has_column {
            self.conn
                .execute("ALTER TABLE tracks ADD COLUMN fingerprint TEXT", params![])
                .context("Failed to add fingerprint column")?;

            self.conn
                .execute(
                    "CREATE INDEX IF NOT EXISTS idx_fingerprint ON tracks(fingerprint)",
                    params![],
                )
                .context("Failed to create fingerprint index")?;
        }

        Ok(())
    }

    /// Migrate existing databases to add ISRC column
    fn migrate_add_isrc(&self) -> Result<()> {
        let has_column: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('tracks') WHERE name='isrc'",
                params![],
                |row| {
                    let count: i64 = row.get(0)?;
                    Ok(count > 0)
                },
            )
            .unwrap_or(false);

        if !has_column {
            self.conn
                .execute("ALTER TABLE tracks ADD COLUMN isrc TEXT", params![])
                .context("Failed to add ISRC column")?;

            self.conn
                .execute(
                    "CREATE INDEX IF NOT EXISTS idx_isrc ON tracks(isrc)",
                    params![],
                )
                .context("Failed to create ISRC index")?;
        }

        Ok(())
    }

    // ========================================================================
    // Track Operations
    // ========================================================================

    pub fn insert_track(&self, track: &Track) -> Result<i64> {
        self.conn
            .execute(
                r#"
            INSERT OR REPLACE INTO tracks
            (path, source, inode, file_size, file_type, artist, album, album_artist,
             title, track_number, duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            "#,
                params![
                    &track.path,
                    &track.source,
                    &track.inode,
                    &track.file_size,
                    &track.file_type,
                    &track.artist,
                    &track.album,
                    &track.album_artist,
                    &track.title,
                    &track.track_number,
                    &track.duration_ms,
                    &track.bitrate_kbps,
                    &track.sample_rate,
                    &track.fingerprint,
                    &track.isrc,
                ],
            )
            .context("Failed to insert track")?;

        Ok(self.conn.last_insert_rowid())
    }

    pub fn clear_source(&self, source: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM tracks WHERE source = ?1", params![source])
            .context("Failed to clear source")?;
        Ok(())
    }

    pub fn clear_scan_state(&self, source: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM scan_state WHERE source = ?1", params![source])
            .context("Failed to clear scan state")?;
        Ok(())
    }

    pub fn get_track_count(&self, source: Option<&str>) -> Result<usize> {
        let count: i64 = if let Some(src) = source {
            self.conn.query_row(
                "SELECT COUNT(*) FROM tracks WHERE source = ?1",
                params![src],
                |row| row.get(0),
            )?
        } else {
            self.conn
                .query_row("SELECT COUNT(*) FROM tracks", params![], |row| row.get(0))?
        };
        Ok(count as usize)
    }

    pub fn log_scan(
        &self,
        source: &str,
        file_count: usize,
        started_at: &str,
        completed_at: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO scan_history (source, file_count, started_at, completed_at) VALUES (?1, ?2, ?3, ?4)",
            params![source, file_count as i64, started_at, completed_at],
        ).context("Failed to log scan")?;
        Ok(())
    }

    pub fn get_sources(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT source FROM tracks ORDER BY source")?;
        let sources = stmt
            .query_map(params![], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(sources)
    }

    pub fn get_all_tracks(&self, source: Option<&str>) -> Result<Vec<Track>> {
        let query = if source.is_some() {
            "SELECT id, path, source, inode, file_size, file_type, artist, album, album_artist,
                    title, track_number, duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
             FROM tracks WHERE source = ?1 ORDER BY path"
        } else {
            "SELECT id, path, source, inode, file_size, file_type, artist, album, album_artist,
                    title, track_number, duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
             FROM tracks ORDER BY path"
        };

        let mut stmt = self.conn.prepare(query)?;
        let tracks = if let Some(src) = source {
            stmt.query_map(params![src], Self::row_to_track)?
        } else {
            stmt.query_map(params![], Self::row_to_track)?
        };

        tracks.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    // ========================================================================
    // Scan State Operations
    // ========================================================================

    pub fn get_scan_state_batch(
        &self,
        source: &str,
        inodes: &[i64],
    ) -> Result<std::collections::HashMap<i64, ScanStateEntry>> {
        use std::collections::HashMap;

        if inodes.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders = (0..inodes.len()).map(|_| "?").collect::<Vec<_>>().join(",");

        let query = format!(
            "SELECT source, inode, path, mtime_secs, mtime_nanos, file_size
             FROM scan_state
             WHERE source = ? AND inode IN ({})",
            placeholders
        );

        let mut stmt = self.conn.prepare(&query)?;

        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&source];
        for inode in inodes {
            params.push(inode);
        }

        let entries = stmt.query_map(&params[..], |row| {
            Ok(ScanStateEntry {
                source: row.get(0)?,
                inode: row.get(1)?,
                path: row.get(2)?,
                mtime_secs: row.get(3)?,
                mtime_nanos: row.get(4)?,
                file_size: row.get(5)?,
            })
        })?;

        let mut map = HashMap::new();
        for entry in entries {
            let entry = entry?;
            map.insert(entry.inode, entry);
        }

        Ok(map)
    }

    pub fn upsert_scan_state(&self, entry: &ScanStateEntry) -> Result<()> {
        self.conn
            .execute(
                r#"
            INSERT INTO scan_state (source, inode, path, mtime_secs, mtime_nanos, file_size)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(source, inode) DO UPDATE SET
                path = excluded.path,
                mtime_secs = excluded.mtime_secs,
                mtime_nanos = excluded.mtime_nanos,
                file_size = excluded.file_size,
                scanned_at = CURRENT_TIMESTAMP
            "#,
                params![
                    &entry.source,
                    &entry.inode,
                    &entry.path,
                    &entry.mtime_secs,
                    &entry.mtime_nanos,
                    &entry.file_size,
                ],
            )
            .context("Failed to upsert scan state")?;
        Ok(())
    }

    pub fn cleanup_stale_scan_state(
        &self,
        source: &str,
        current_inodes: &std::collections::HashSet<i64>,
    ) -> Result<usize> {
        use std::collections::HashSet;

        if current_inodes.is_empty() {
            let deleted = self
                .conn
                .execute("DELETE FROM scan_state WHERE source = ?1", params![source])?;
            return Ok(deleted);
        }

        let mut stmt = self
            .conn
            .prepare("SELECT inode FROM scan_state WHERE source = ?1")?;
        let existing_inodes: HashSet<i64> = stmt
            .query_map(params![source], |row| row.get(0))?
            .collect::<Result<HashSet<_>, _>>()?;

        let to_delete: Vec<i64> = existing_inodes
            .difference(current_inodes)
            .copied()
            .collect();

        if to_delete.is_empty() {
            return Ok(0);
        }

        let placeholders = (0..to_delete.len())
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");

        let query = format!(
            "DELETE FROM scan_state WHERE source = ? AND inode IN ({})",
            placeholders
        );

        let mut params: Vec<&dyn rusqlite::ToSql> = vec![&source];
        for inode in &to_delete {
            params.push(inode);
        }

        let deleted = self.conn.execute(&query, &params[..])?;
        Ok(deleted)
    }

    // ========================================================================
    // Deployment Operations
    // ========================================================================

    pub fn get_tracks_by_corpus_path_prefix(&self, path_prefix: &str) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, source, inode, file_size, file_type, artist, album, album_artist,
                    title, track_number, duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
             FROM tracks
             WHERE source = 'corpus' AND path LIKE ?1 || '%'
             ORDER BY path",
        )?;

        let tracks = stmt.query_map(params![path_prefix], Self::row_to_track)?;
        tracks.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_library_tracks_by_source(&self, source: &str) -> Result<Vec<Track>> {
        self.get_all_tracks(Some(source))
    }

    pub fn log_deployment(
        &self,
        library_name: &str,
        corpus_path: &str,
        deployed_path: &str,
        inode: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO deployment_log (library_name, corpus_path, deployed_path, inode)
             VALUES (?1, ?2, ?3, ?4)",
                params![library_name, corpus_path, deployed_path, inode],
            )
            .context("Failed to log deployment")?;
        Ok(())
    }

    // ========================================================================
    // Corpus Health Operations
    // ========================================================================

    pub fn compute_deployment_stats(&self) -> Result<DeploymentStats> {
        let total: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM tracks WHERE source = 'corpus'",
                params![],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let sources = self.get_sources()?;
        let library_sources: Vec<String> = sources
            .iter()
            .filter(|s| *s != "corpus" && *s != "legacy")
            .cloned()
            .collect();

        let mut library_inodes = std::collections::HashSet::new();
        for source in library_sources {
            let tracks = self.get_all_tracks(Some(&source))?;
            for track in tracks {
                library_inodes.insert(track.inode);
            }
        }

        let deployed = if total > 0 {
            let corpus_tracks = self.get_all_tracks(Some("corpus"))?;
            corpus_tracks
                .iter()
                .filter(|t| library_inodes.contains(&t.inode))
                .count()
        } else {
            0
        };

        let percentage = if total > 0 {
            (deployed as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        let last_updated = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

        Ok(DeploymentStats {
            total_corpus_files: total,
            deployed_files: deployed,
            deployment_percentage: percentage,
            last_updated,
        })
    }

    pub fn update_corpus_health_stats(&self) -> Result<()> {
        let stats = self.compute_deployment_stats()?;
        let stats_json =
            serde_json::to_string(&stats).context("Failed to serialize deployment stats")?;

        self.conn.execute(
            "INSERT OR REPLACE INTO corpus_health_stats (id, stat_type, data_json, last_updated)
             VALUES (1, 'deployment', ?1, datetime('now'))",
            params![stats_json],
        ).context("Failed to update corpus health stats")?;

        Ok(())
    }

    pub fn get_deployment_stats(&self) -> Result<Option<DeploymentStats>> {
        let result = self.conn.query_row(
            "SELECT data_json FROM corpus_health_stats WHERE stat_type = 'deployment'",
            params![],
            |row| {
                let json: String = row.get(0)?;
                Ok(json)
            },
        );

        match result {
            Ok(json) => {
                let stats: DeploymentStats = serde_json::from_str(&json)
                    .context("Failed to deserialize deployment stats")?;
                Ok(Some(stats))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // ========================================================================
    // Tag Edit Operations
    // ========================================================================

    pub fn log_tag_edit(
        &self,
        track_id: i64,
        field_name: &str,
        old_value: Option<&str>,
        new_value: Option<&str>,
        session_id: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tag_edit_history (track_id, field_name, old_value, new_value, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![track_id, field_name, old_value, new_value, session_id],
        ).context("Failed to log tag edit")?;
        Ok(())
    }

    pub fn update_track_tag(&self, track_id: i64, field_name: &str, value: &str) -> Result<()> {
        let query = match field_name {
            "artist" => "UPDATE tracks SET artist = ?1 WHERE id = ?2",
            "album" => "UPDATE tracks SET album = ?1 WHERE id = ?2",
            "album_artist" => "UPDATE tracks SET album_artist = ?1 WHERE id = ?2",
            "title" => "UPDATE tracks SET title = ?1 WHERE id = ?2",
            "track_number" => {
                if let Ok(num) = value.parse::<i32>() {
                    self.conn
                        .execute(
                            "UPDATE tracks SET track_number = ?1 WHERE id = ?2",
                            params![num, track_id],
                        )
                        .context("Failed to update track_number")?;
                    return Ok(());
                } else {
                    return Ok(());
                }
            }
            "isrc" => "UPDATE tracks SET isrc = ?1 WHERE id = ?2",
            _ => return Ok(()),
        };

        self.conn
            .execute(query, params![value, track_id])
            .with_context(|| format!("Failed to update {} for track {}", field_name, track_id))?;

        Ok(())
    }

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
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.path, t.source, t.inode, t.file_size, t.file_type,
                    t.artist, t.album, t.album_artist, t.title, t.track_number,
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

    pub fn get_tracks_by_paths(
        &self,
        path_prefixes: &[PathBuf],
        source: &str,
    ) -> Result<Vec<Track>> {
        if path_prefixes.is_empty() {
            return Ok(Vec::new());
        }

        let conditions: Vec<String> = path_prefixes
            .iter()
            .enumerate()
            .map(|(i, _)| format!("path LIKE ?{}", i + 2))
            .collect();
        let where_clause = conditions.join(" OR ");

        let query = format!(
            "SELECT id, path, source, inode, file_size, file_type,
                    artist, album, album_artist, title, track_number,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
             FROM tracks
             WHERE source = ?1 AND fingerprint IS NOT NULL AND ({})
             ORDER BY path",
            where_clause
        );

        let mut stmt = self.conn.prepare(&query)?;

        let mut params: Vec<String> = vec![source.to_string()];
        for prefix in path_prefixes {
            let pattern = format!("{}%", prefix.to_string_lossy());
            params.push(pattern);
        }

        let param_refs: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();

        let tracks = stmt
            .query_map(&param_refs[..], Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    // ========================================================================
    // Change Session Operations
    // ========================================================================

    pub fn create_change_session(&self, description: &str) -> Result<String> {
        let session_id = uuid::Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO change_sessions (session_id, description, status)
             VALUES (?1, ?2, 'active')",
            params![&session_id, description],
        ).context("Failed to create change session")?;
        Ok(session_id)
    }

    pub fn get_active_session(&self) -> Result<Option<ChangeSession>> {
        let result = self.conn.query_row(
            "SELECT session_id, description, created_at, committed_at, status
             FROM change_sessions
             WHERE status = 'active'
             ORDER BY created_at DESC
             LIMIT 1",
            params![],
            |row| {
                Ok(ChangeSession {
                    session_id: row.get(0)?,
                    description: row.get(1)?,
                    created_at: row.get(2)?,
                    committed_at: row.get(3)?,
                    status: row.get(4)?,
                })
            },
        );

        match result {
            Ok(session) => Ok(Some(session)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn get_change_session(&self, session_id: &str) -> Result<Option<ChangeSession>> {
        let result = self.conn.query_row(
            "SELECT session_id, description, created_at, committed_at, status
             FROM change_sessions
             WHERE session_id = ?1",
            params![session_id],
            |row| {
                Ok(ChangeSession {
                    session_id: row.get(0)?,
                    description: row.get(1)?,
                    created_at: row.get(2)?,
                    committed_at: row.get(3)?,
                    status: row.get(4)?,
                })
            },
        );

        match result {
            Ok(session) => Ok(Some(session)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn commit_session(&self, session_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE change_sessions
             SET status = 'committed', committed_at = CURRENT_TIMESTAMP
             WHERE session_id = ?1",
            params![session_id],
        ).context("Failed to commit change session")?;
        Ok(())
    }

    pub fn revert_session(&self, session_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE change_sessions
             SET status = 'reverted'
             WHERE session_id = ?1",
            params![session_id],
        ).context("Failed to revert change session")?;
        Ok(())
    }

    // ========================================================================
    // Pending Change Operations
    // ========================================================================

    pub fn add_pending_change(&self, change: &PendingChange) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO pending_changes (session_id, change_type, source_path, target_path, metadata_changes, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &change.session_id,
                change.change_type.as_str(),
                &change.source_path,
                &change.target_path,
                &change.metadata_changes,
                change.status.as_str(),
            ],
        ).context("Failed to add pending change")?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_pending_changes(&self, session_id: &str) -> Result<Vec<PendingChange>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, change_type, source_path, target_path, metadata_changes, created_at, status
             FROM pending_changes
             WHERE session_id = ?1
             ORDER BY id",
        )?;

        let changes = stmt.query_map(params![session_id], Self::row_to_pending_change)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(changes)
    }

    pub fn get_all_pending_changes(&self) -> Result<Vec<PendingChange>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, change_type, source_path, target_path, metadata_changes, created_at, status
             FROM pending_changes
             WHERE status = 'pending'
             ORDER BY session_id, id",
        )?;

        let changes = stmt.query_map(params![], Self::row_to_pending_change)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(changes)
    }

    pub fn update_change_status(&self, change_id: i64, status: ChangeStatus) -> Result<()> {
        self.conn.execute(
            "UPDATE pending_changes SET status = ?1 WHERE id = ?2",
            params![status.as_str(), change_id],
        ).context("Failed to update change status")?;
        Ok(())
    }

    pub fn clear_pending_changes(&self, session_id: &str) -> Result<usize> {
        let deleted = self.conn.execute(
            "DELETE FROM pending_changes WHERE session_id = ?1 AND status = 'pending'",
            params![session_id],
        ).context("Failed to clear pending changes")?;
        Ok(deleted)
    }

    pub fn get_pending_change_counts(&self) -> Result<std::collections::HashMap<String, usize>> {
        use std::collections::HashMap;

        let mut stmt = self.conn.prepare(
            "SELECT change_type, COUNT(*)
             FROM pending_changes
             WHERE status = 'pending'
             GROUP BY change_type",
        )?;

        let mut counts = HashMap::new();
        let rows = stmt.query_map(params![], |row| {
            let change_type: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((change_type, count as usize))
        })?;

        for row in rows {
            let (k, v) = row?;
            counts.insert(k, v);
        }

        Ok(counts)
    }

    // ========================================================================
    // Row Conversion Helpers
    // ========================================================================

    fn row_to_pending_change(row: &rusqlite::Row) -> rusqlite::Result<PendingChange> {
        let change_type_str: String = row.get(2)?;
        let status_str: String = row.get(7)?;

        Ok(PendingChange {
            id: Some(row.get(0)?),
            session_id: row.get(1)?,
            change_type: ChangeType::from_str(&change_type_str).unwrap_or(ChangeType::Move),
            source_path: row.get(3)?,
            target_path: row.get(4)?,
            metadata_changes: row.get(5)?,
            created_at: row.get(6)?,
            status: ChangeStatus::from_str(&status_str).unwrap_or(ChangeStatus::Pending),
        })
    }

    fn row_to_track(row: &rusqlite::Row) -> rusqlite::Result<Track> {
        Ok(Track {
            id: Some(row.get(0)?),
            path: row.get(1)?,
            source: row.get(2)?,
            inode: row.get(3)?,
            file_size: row.get(4)?,
            file_type: row.get(5)?,
            artist: row.get(6)?,
            album: row.get(7)?,
            album_artist: row.get(8)?,
            title: row.get(9)?,
            track_number: row.get(10)?,
            duration_ms: row.get(11)?,
            bitrate_kbps: row.get(12)?,
            sample_rate: row.get(13)?,
            fingerprint: row.get(14)?,
            isrc: row.get(15)?,
        })
    }
}
