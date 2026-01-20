//! Library scan state queries.
//!
//! This module handles persistence of library file scan results between
//! computation phases:
//! - **Awakening**: ScanLibraryDirectory writes discovered files here
//! - **Awake**: DeriveDeployHealthSignals reads and processes this data

use anyhow::{Context, Result};
use rusqlite::params;
use std::path::PathBuf;

use super::Database;

/// A library file discovered during scanning.
#[derive(Debug, Clone)]
pub struct LibraryScanEntry {
    pub library_name: String,
    pub library_root: PathBuf,
    pub file_path: PathBuf,
    pub inode: i64,
    pub scanned_at: i64,
}

impl Database {
    /// Clear all scan state for a library before re-scanning.
    ///
    /// Called at the start of WalkLibrary to ensure fresh scan results.
    pub fn clear_library_scan_state(&self, library_name: &str) -> Result<usize> {
        let count = self
            .conn
            .execute(
                "DELETE FROM library_scan_state WHERE library_name = ?1",
                params![library_name],
            )
            .context("Failed to clear library scan state")?;
        Ok(count)
    }

    /// Record a file discovered during library scanning.
    ///
    /// Called by ScanLibraryDirectory (Awakening phase) for each audio file found.
    pub fn record_library_file(
        &self,
        library_name: &str,
        library_root: &std::path::Path,
        file_path: &std::path::Path,
        inode: i64,
        scanned_at: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO library_scan_state
                 (library_name, library_root, file_path, inode, scanned_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    library_name,
                    library_root.to_string_lossy().as_ref(),
                    file_path.to_string_lossy().as_ref(),
                    inode,
                    scanned_at
                ],
            )
            .context("Failed to record library file")?;
        Ok(())
    }

    /// Get all files for a library (for DeriveDeployHealthSignals).
    ///
    /// Returns all files discovered during the most recent scan of the library.
    pub fn get_library_scan_files(&self, library_name: &str) -> Result<Vec<LibraryScanEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT library_name, library_root, file_path, inode, scanned_at
             FROM library_scan_state
             WHERE library_name = ?1
             ORDER BY file_path",
        )?;

        let entries = stmt
            .query_map(params![library_name], |row| {
                Ok(LibraryScanEntry {
                    library_name: row.get(0)?,
                    library_root: PathBuf::from(row.get::<_, String>(1)?),
                    file_path: PathBuf::from(row.get::<_, String>(2)?),
                    inode: row.get(3)?,
                    scanned_at: row.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed to query library scan files")?;

        Ok(entries)
    }

    /// Get unique library names that have scan data.
    ///
    /// Used by ScheduleContentAnalysis to spawn DeriveDeployHealthSignals
    /// for each library that was scanned during Awakening.
    pub fn get_scanned_library_names(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT library_name FROM library_scan_state ORDER BY library_name",
        )?;

        let names = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()
            .context("Failed to query scanned library names")?;

        Ok(names)
    }

    /// Get library metadata (root path and corpus prefixes) for a scanned library.
    ///
    /// Returns (library_root, corpus_path_prefixes) if the library has scan data.
    /// The corpus_path_prefixes need to be looked up from config, so this just
    /// returns the library_root which can be used to find the config entry.
    pub fn get_library_root(&self, library_name: &str) -> Result<Option<PathBuf>> {
        let root: Option<String> = self
            .conn
            .query_row(
                "SELECT DISTINCT library_root FROM library_scan_state WHERE library_name = ?1 LIMIT 1",
                params![library_name],
                |row| row.get(0),
            )
            .ok();

        Ok(root.map(PathBuf::from))
    }
}
