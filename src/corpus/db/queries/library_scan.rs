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
use crate::db_thread::SignalWitness;

/// A library file discovered during scanning.
#[derive(Debug, Clone)]
pub struct LibraryScanEntry {
    pub file_path: PathBuf,
    pub inode: i64,
}

impl Database {
    /// Clear all files for a library before re-scanning.
    ///
    /// Deletes all files where zone = 'library' and path starts with `library_name/`.
    /// Called at the start of WalkLibrary to ensure fresh scan results.
    pub fn clear_library_files(&self, library_name: &str, _witness: &impl SignalWitness) -> Result<usize> {
        let pattern = dir_like_pattern_str(library_name);
        let count = self
            .conn
            .execute(
                r"DELETE FROM files WHERE zone = 'library' AND path LIKE ?1 ESCAPE '\'",
                params![pattern],
            )
            .context("Failed to clear library files")?;
        Ok(count)
    }

    /// Record a file discovered during library scanning.
    ///
    /// Stores the file in the `files` table with zone = 'library'.
    /// The path is stored as `library_name/relative_path_within_library`.
    #[allow(clippy::too_many_arguments)]
    pub fn record_library_file(
        &self,
        library_name: &str,
        library_root: &std::path::Path,
        file_path: &std::path::Path,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
        scanned_at: i64,
        _witness: &impl SignalWitness,
    ) -> Result<()> {
        // Compute library-relative path: strip library_root prefix, prepend library_name
        let library_relative = file_path
            .strip_prefix(library_root)
            .unwrap_or(file_path);
        let stored_path = format!("{}/{}", library_name, library_relative.display());

        self.conn
            .execute(
                "INSERT OR REPLACE INTO files
                 (inode, zone, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
                 VALUES (?1, 'library', ?2, 0, ?3, ?4, ?5, ?6)",
                params![
                    inode,
                    stored_path,
                    mtime_secs,
                    mtime_nanos,
                    file_size,
                    scanned_at
                ],
            )
            .context("Failed to record library file")?;
        Ok(())
    }

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

    /// Get all library files across all libraries.
    ///
    /// Returns all files where zone = 'library'.
    pub fn get_all_library_files(&self) -> Result<Vec<LibraryScanEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT path, inode FROM files WHERE zone = 'library' ORDER BY path",
        )?;

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
