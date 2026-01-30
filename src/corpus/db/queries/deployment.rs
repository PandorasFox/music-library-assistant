//! Deployment statistics operations.

use anyhow::{Context, Result};
use rusqlite::params;

use super::Database;
use crate::corpus::db::types::DeploymentStats;

impl Database {
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
