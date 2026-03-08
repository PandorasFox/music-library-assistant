//! Library scan queries.
//!
//! Library files are stored in the `files` table with `zone = 'library'`.
//! The path includes the library name as a prefix: `library_name/artist/album/track.flac`.
//!
//! Write operations require a witness for authorized execution.

use anyhow::{Context, Result};
use rusqlite::params;
use std::path::PathBuf;

use super::{dir_like_pattern_str, Database};

/// A library file discovered during scanning.
#[derive(Debug, Clone)]
pub struct LibraryScanEntry {
    pub file_path: PathBuf,
    pub inode: i64,
}

impl Database {
    /// Get all files for a library (for DeriveDeployHealthSignals).
    ///
    /// Returns all files where zone = 'library' and path starts with `library_name/`.
    pub fn get_library_files(&self, library_name: &str) -> Result<Vec<LibraryScanEntry>> {
        let pattern = dir_like_pattern_str(library_name);
        let mut stmt = self.conn.prepare(
            r"SELECT path, inode
             FROM files
             WHERE zone = 'library' AND path LIKE ?1 ESCAPE '\'
             ORDER BY path",
        )?;

        let entries = stmt
            .query_map(params![pattern], |row| {
                Ok(LibraryScanEntry {
                    file_path: PathBuf::from(row.get::<_, String>(0)?),
                    inode: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to query library files")?;

        Ok(entries)
    }

    /// Check if a directory entry in the files table already matches the given metadata.
    ///
    /// Returns true if a directory entry exists with matching zone, inode, mtime_secs,
    /// and mtime_nanos, meaning no write is needed (entry is fresh).
    /// Used by `index_directory()` to skip unconditional INSERT OR REPLACE writes.
    pub fn directory_entry_fresh(
        &self,
        zone: &str,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
    ) -> bool {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM files WHERE inode=?1 AND zone=?2 AND is_dir=1 AND mtime_secs=?3 AND mtime_nanos=?4)",
                params![inode, zone, mtime_secs, mtime_nanos],
                |row| row.get::<_, bool>(0),
            )
            .unwrap_or(false)
    }

    /// Get all library file metadata for reconciliation.
    ///
    /// Returns a map of stored_path → (inode, mtime_secs, mtime_nanos, file_size)
    /// for all library audio files (is_dir=0).
    pub fn get_library_file_metadata(
        &self,
    ) -> Result<std::collections::HashMap<String, (i64, i64, i64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, inode, mtime_secs, mtime_nanos, file_size
             FROM files WHERE zone='library' AND is_dir=0",
        )?;

        let mut map = std::collections::HashMap::new();
        let rows = stmt.query_map(params![], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;

        for row in rows {
            let (path, inode, mtime_secs, mtime_nanos, file_size) = row?;
            map.insert(path, (inode, mtime_secs, mtime_nanos, file_size));
        }

        Ok(map)
    }

    /// Get all library files across all libraries.
    ///
    /// Returns all files where zone = 'library'.
    pub fn get_all_library_files(&self) -> Result<Vec<LibraryScanEntry>> {
        let mut stmt = self
            .conn
            .prepare("SELECT path, inode FROM files WHERE zone = 'library' ORDER BY path")?;

        let entries = stmt
            .query_map(params![], |row| {
                Ok(LibraryScanEntry {
                    file_path: PathBuf::from(row.get::<_, String>(0)?),
                    inode: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to query all library files")?;

        Ok(entries)
    }
}
