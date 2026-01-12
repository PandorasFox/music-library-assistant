//! Change session and pending change operations.

use anyhow::{Context, Result};
use rusqlite::params;
use std::collections::HashMap;

use super::Database;
use crate::db::changes::{ChangeSession, ChangeStatus, ChangeType, PendingChange};

impl Database {
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

    pub fn get_pending_change_counts(&self) -> Result<HashMap<String, usize>> {
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
    // Row Conversion Helper
    // ========================================================================

    pub(super) fn row_to_pending_change(row: &rusqlite::Row) -> rusqlite::Result<PendingChange> {
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
}
