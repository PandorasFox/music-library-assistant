//! Deployment logging operations.

use anyhow::{Context, Result};
use rusqlite::params;
use std::collections::HashSet;

use super::Database;
use crate::corpus::db::types::DeploymentStats;

impl Database {
    // ========================================================================
    // Deployment Operations
    // ========================================================================

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
    // Corpus Health Operations (Deployment Stats)
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

        let mut library_inodes = HashSet::new();
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
}
