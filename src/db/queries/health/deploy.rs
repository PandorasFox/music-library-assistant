//! Deploy-related health queries.

use anyhow::Result;
use rusqlite::params;

use super::super::Database;
use crate::db::types::Zone;

impl Database {
    /// Get all deploy-ready files (healthy corpus files not yet in library).
    ///
    /// Returns files with their corpus path and computed deploy path.
    /// Sorted by corpus_path for consistent display.
    pub fn get_deploy_ready_files(&self) -> Result<Vec<crate::meta::views::DeploySignalFile>> {
        use crate::meta::views::DeploySignalFile;

        let mut stmt = self
            .conn
            .prepare("SELECT path, deploy_path, library_name FROM signal_deploy_ready ORDER BY path")?;

        let results = stmt
            .query_map(params![], |row| {
                Ok(DeploySignalFile {
                    corpus_path: row.get(0)?,
                    deploy_path: row.get(1)?,
                    library_name: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all deployed healthy files (corpus files correctly deployed to library).
    ///
    /// Returns files with their corpus path and library path.
    /// Sorted by corpus_path for consistent display.
    pub fn get_deployed_healthy_files(&self) -> Result<Vec<crate::meta::views::DeploySignalFile>> {
        use crate::meta::views::DeploySignalFile;

        let mut stmt = self
            .conn
            .prepare("SELECT path, library_path FROM signal_deployed_healthy ORDER BY path")?;

        let results = stmt
            .query_map(params![], |row| {
                let library_path: String = row.get(1)?;
                // library_path is "{library_name}/relative/path" — extract library_name
                let library_name = library_path.split('/').next().unwrap_or("").to_string();
                Ok(DeploySignalFile {
                    library_name,
                    corpus_path: row.get(0)?,
                    deploy_path: library_path,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all sidecar images ready for deployment.
    ///
    /// Reads precomputed signals emitted by DeriveCorpusDeployStatus.
    pub fn get_sidecar_deploy_ready_signals(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::SidecarDeployReadySignal>> {
        use crate::meta::signals::data::{SidecarDeployReadyData, SidecarDeployReadySignal};

        let mut stmt = self.conn.prepare(
            "SELECT inode, path, deploy_path, library_name, data FROM signal_sidecar_deploy_ready ORDER BY path"
        )?;

        let results = stmt
            .query_map(params![], |row| {
                let blob: Vec<u8> = row.get(4)?;
                let data: SidecarDeployReadyData = bincode::deserialize(&blob).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Blob,
                        Box::new(e),
                    )
                })?;
                Ok(SidecarDeployReadySignal {
                    inode: row.get(0)?,
                    path: row.get(1)?,
                    deploy_path: row.get(2)?,
                    library_name: row.get(3)?,
                    data,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all stale library files (deployed at wrong path due to tag changes).
    ///
    /// Returns files with their current library path and expected path.
    /// Sorted by library_path for consistent display.
    pub fn get_library_stale_files(&self) -> Result<Vec<crate::meta::views::StaleSignalFile>> {
        use crate::meta::views::StaleSignalFile;

        let mut stmt = self.conn.prepare(
            "SELECT library_path, expected_path FROM signal_library_stale ORDER BY library_path",
        )?;

        let results = stmt
            .query_map(params![], |row| {
                let library_path: String = row.get(0)?;
                // library_path is "{library_name}/relative/path" — extract library_name
                let library_name = library_path.split('/').next().unwrap_or("").to_string();
                Ok(StaleSignalFile {
                    library_name,
                    library_path,
                    expected_path: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all leftover library files (no corpus backing).
    ///
    /// Sorted by library_path for consistent display.
    pub fn get_library_leftover_files(
        &self,
    ) -> Result<Vec<crate::meta::views::LeftoverSignalFile>> {
        use crate::meta::views::LeftoverSignalFile;

        // key = "library_leftover:{library_name}:{library_name}/path/..."
        let mut stmt = self
            .conn
            .prepare("SELECT key FROM signal_library_leftover ORDER BY key")?;

        let results = stmt
            .query_map(params![], |row| {
                let key: String = row.get(0)?;
                let (library_name, library_path) =
                    crate::meta::signals::data::LibraryLeftoverSignal::parse_key(&key)
                        .map(|(n, p)| (n.to_string(), p.to_string()))
                        .unwrap_or_else(|| (String::new(), key.clone()));
                Ok(LeftoverSignalFile {
                    library_name,
                    library_path,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all deploy conflict groups (multiple corpus files → same library path).
    ///
    /// Sorted by key for consistent display.
    pub fn get_deploy_conflict_groups(&self) -> Result<Vec<crate::meta::views::ConflictGroup>> {
        use crate::meta::views::ConflictGroup;

        let mut stmt = self
            .conn
            .prepare("SELECT key, data FROM signal_deploy_conflict ORDER BY key")?;

        let mut results = Vec::new();
        let rows = stmt.query_map(params![], |row| {
            let deploy_path: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((deploy_path, blob))
        })?;

        for row in rows {
            let (deploy_path, blob) = row?;
            let inodes: Vec<i64> = bincode::deserialize(&blob).unwrap_or_default();

            let mut conflicting_files = Vec::new();
            for inode in inodes {
                if let Ok(Some(audio_file)) = self.get_audio_file_by_inode(inode, Zone::Corpus) {
                    conflicting_files.push((audio_file.path().to_string(), inode));
                }
            }

            results.push(ConflictGroup {
                deploy_path,
                conflicting_files,
            });
        }

        Ok(results)
    }

    /// Get all sidecar deploy conflict groups (multiple corpus images → same library path).
    ///
    /// Sorted by library_name/deploy_path for consistent display.
    pub fn get_sidecar_conflict_groups(
        &self,
    ) -> Result<Vec<crate::meta::views::SidecarConflictGroup>> {
        use crate::meta::views::SidecarConflictGroup;

        let mut stmt = self.conn.prepare(
            "SELECT deploy_path, library_name, data FROM signal_sidecar_deploy_conflict ORDER BY library_name, deploy_path"
        )?;

        let mut results = Vec::new();
        let rows = stmt.query_map(params![], |row| {
            let deploy_path: String = row.get(0)?;
            let library_name: String = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;
            Ok((deploy_path, library_name, blob))
        })?;

        for row in rows {
            let (deploy_path, library_name, blob) = row?;
            let inodes: Vec<i64> = bincode::deserialize(&blob).unwrap_or_default();

            let mut conflicting_files = Vec::new();
            for inode in inodes {
                if let Ok(Some(path)) = self.get_path_for_inode::<crate::zones::CorpusZone>(inode) {
                    conflicting_files.push((path, inode));
                }
            }

            results.push(SidecarConflictGroup {
                deploy_path,
                library_name,
                conflicting_files,
            });
        }

        Ok(results)
    }

    /// Get deploy status for the Deploy view and titlebar indicator.
    ///
    /// Returns whether there's actionable deploy work and per-library file counts.
    /// `configured_libraries` limits counting to only the configured deployment
    /// directories, ignoring other top-level dirs under the library root.
    pub fn get_deploy_status(
        &self,
        configured_libraries: &[String],
    ) -> Result<crate::meta::views::DeployStatus> {
        let needs_action: bool = self.conn.query_row(
            "SELECT
                EXISTS(SELECT 1 FROM signal_deploy_ready)
                OR EXISTS(SELECT 1 FROM signal_library_stale)
                OR EXISTS(SELECT 1 FROM signal_library_leftover)
                OR EXISTS(SELECT 1 FROM signal_sidecar_deploy_ready)",
            [],
            |row| row.get(0),
        )?;

        // Per-library file counts: only count files in configured library dirs
        let mut library_file_counts = Vec::new();
        for lib_name in configured_libraries {
            let pattern = super::super::dir_like_pattern_str(lib_name);
            let count: usize = self.conn.query_row(
                "SELECT COUNT(*) FROM inode_paths WHERE zone = 'library' AND path LIKE ?1 ESCAPE '\\'",
                params![pattern],
                |row| row.get(0),
            )?;
            library_file_counts.push((lib_name.clone(), count));
        }
        library_file_counts.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(crate::meta::views::DeployStatus {
            needs_action,
            library_file_counts,
        })
    }
}
