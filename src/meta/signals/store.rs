//! Signal storage traits and per-signal table schemas.
//!
//! Each signal type implements `CorpusSignalStore` or `AggregateSignalStore`,
//! defining its SQL table schema and CRUD operations. These replace the single
//! `signals` table + `metadata_json` blob with typed, per-signal tables.
//!
//! ## Table Patterns
//!
//! 1. **Simple corpus signal** — all flat columns, no BLOB
//! 2. **Corpus signal with BLOB** — flat columns + bincode `data BLOB`
//! 3. **Simple aggregate signal** — semantic key + flat columns
//! 4. **Aggregate signal with BLOB** — semantic key + flat columns + bincode `data BLOB`

use rusqlite::{Connection, Result};

// ============================================================================
// Traits
// ============================================================================

/// Inode-keyed corpus file signal storage.
///
/// Each implementing struct defines its own SQL table and provides
/// typed insert/clear/exists operations. The inode is always the primary key.
pub trait CorpusSignalStore: Sized {
    /// SQL CREATE TABLE statement for this signal's table.
    const TABLE_SQL: &'static str;

    /// Table name (for generic operations like COUNT queries).
    const TABLE_NAME: &'static str;

    /// Insert or replace this signal into its table.
    fn insert(&self, conn: &Connection) -> Result<()>;

    /// Delete the signal for this inode.
    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()>;

    /// Check if a signal exists for this inode.
    fn exists(conn: &Connection, inode: i64) -> Result<bool>;

    /// Count all signals in this table.
    fn count(conn: &Connection) -> Result<usize> {
        let sql = format!("SELECT COUNT(*) FROM {}", Self::TABLE_NAME);
        conn.query_row(&sql, [], |row| row.get(0))
    }

    /// Delete all signals in this table.
    fn clear_all(conn: &Connection) -> Result<()> {
        let sql = format!("DELETE FROM {}", Self::TABLE_NAME);
        conn.execute(&sql, [])?;
        Ok(())
    }

    /// Query all inodes that have signals in this table.
    fn all_inodes(conn: &Connection) -> Result<Vec<i64>> {
        let sql = format!("SELECT inode FROM {}", Self::TABLE_NAME);
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }
}

/// Semantic-keyed aggregate signal storage.
///
/// Each implementing struct defines its own SQL table and provides
/// typed insert/clear/exists operations. The key is always a TEXT primary key.
pub trait AggregateSignalStore: Sized {
    /// SQL CREATE TABLE statement for this signal's table.
    const TABLE_SQL: &'static str;

    /// Table name (for generic operations like COUNT queries).
    const TABLE_NAME: &'static str;

    /// Insert or replace this signal into its table.
    fn insert(&self, conn: &Connection) -> Result<()>;

    /// Delete the signal for this key.
    fn clear_by_key(conn: &Connection, key: &str) -> Result<()>;

    /// Check if a signal exists for this key.
    fn exists(conn: &Connection, key: &str) -> Result<bool>;

    /// Count all signals in this table.
    fn count(conn: &Connection) -> Result<usize> {
        let sql = format!("SELECT COUNT(*) FROM {}", Self::TABLE_NAME);
        conn.query_row(&sql, [], |row| row.get(0))
    }

    /// Delete all signals of this type.
    fn clear_all(conn: &Connection) -> Result<()> {
        let sql = format!("DELETE FROM {}", Self::TABLE_NAME);
        conn.execute(&sql, [])?;
        Ok(())
    }

    /// Query all keys for this signal type.
    fn query_keys(conn: &Connection) -> Result<Vec<String>> {
        let sql = format!("SELECT key FROM {} ORDER BY key", Self::TABLE_NAME);
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Delete all signals whose key starts with the given prefix.
    fn clear_by_key_prefix(conn: &Connection, prefix: &str) -> Result<()> {
        let sql = format!(
            "DELETE FROM {} WHERE key LIKE ? ESCAPE '\\'",
            Self::TABLE_NAME
        );
        let pattern = format!(
            "{}%",
            prefix
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        conn.execute(&sql, [pattern])?;
        Ok(())
    }
}

// ============================================================================
// Corpus File Signal Implementations
// ============================================================================

use super::data::*;

// --- Simple signals (flat columns only) ---

impl CorpusSignalStore for FileInCorpusSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_file_in_corpus (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_file_in_corpus";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_file_in_corpus (inode, path) VALUES (?1, ?2)",
            rusqlite::params![self.inode, self.path],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_file_in_corpus WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_file_in_corpus WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl CorpusSignalStore for UnindexedFileSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_unindexed_file (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_unindexed_file";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_unindexed_file (inode, path) VALUES (?1, ?2)",
            rusqlite::params![self.inode, self.path],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_unindexed_file WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_unindexed_file WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl UnindexedFileSignal {
    pub fn query_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT inode, path FROM signal_unindexed_file ORDER BY path"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Self { inode: row.get(0)?, path: row.get(1)? })
        })?;
        rows.collect()
    }
}

impl CorpusSignalStore for HealthyFileSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_healthy_file (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_healthy_file";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_healthy_file (inode, path) VALUES (?1, ?2)",
            rusqlite::params![self.inode, self.path],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_healthy_file WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_healthy_file WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl HealthyFileSignal {
    pub fn query_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT inode, path FROM signal_healthy_file ORDER BY path"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Self { inode: row.get(0)?, path: row.get(1)? })
        })?;
        rows.collect()
    }
}

impl CorpusSignalStore for CorruptFileSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_corrupt_file (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_corrupt_file";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_corrupt_file (inode, path) VALUES (?1, ?2)",
            rusqlite::params![self.inode, self.path],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_corrupt_file WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_corrupt_file WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl CorpusSignalStore for MtimeOnlyMismatchSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_mtime_only_mismatch (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_mtime_only_mismatch";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_mtime_only_mismatch (inode, path) VALUES (?1, ?2)",
            rusqlite::params![self.inode, self.path],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_mtime_only_mismatch WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_mtime_only_mismatch WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl CorpusSignalStore for MissingDirectorySignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_missing_directory (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_missing_directory";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_missing_directory (inode, path) VALUES (?1, ?2)",
            rusqlite::params![self.inode, self.path],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_missing_directory WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_missing_directory WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

// --- Signals with extra flat columns ---

impl CorpusSignalStore for MissingFileSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_missing_file (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        replaced_by_inode INTEGER,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_missing_file";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_missing_file (inode, path, replaced_by_inode) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.inode, self.path, self.replaced_by_inode],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_missing_file WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_missing_file WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl CorpusSignalStore for MovedFileSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_moved_file (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        old_path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_moved_file";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_moved_file (inode, path, old_path) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.inode, self.path, self.old_path],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_moved_file WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_moved_file WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl CorpusSignalStore for ShitFormatSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_shit_format (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        file_type TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_shit_format";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_shit_format (inode, path, file_type) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.inode, self.path, self.file_type],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_shit_format WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_shit_format WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl CorpusSignalStore for DeployReadySignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_deploy_ready (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        deploy_path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_deploy_ready";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_deploy_ready (inode, path, deploy_path) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.inode, self.path, self.deploy_path],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_deploy_ready WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_deploy_ready WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl CorpusSignalStore for DeployedHealthySignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_deployed_healthy (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        library_path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_deployed_healthy";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_deployed_healthy (inode, path, library_path) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.inode, self.path, self.library_path],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_deployed_healthy WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_deployed_healthy WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

// --- Signals with bincode BLOB data ---

impl CorpusSignalStore for OutOfBandTagSyncSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_oob_tag_sync (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_oob_tag_sync";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.mismatches)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_oob_tag_sync (inode, path, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.inode, self.path, data],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_oob_tag_sync WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_oob_tag_sync WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl CorpusSignalStore for OutOfBandTagConflictSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_oob_tag_conflict (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_oob_tag_conflict";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.mismatches)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_oob_tag_conflict (inode, path, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.inode, self.path, data],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_oob_tag_conflict WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_oob_tag_conflict WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl CorpusSignalStore for SubparDuplicateSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_subpar_duplicate (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_subpar_duplicate";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.data)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_subpar_duplicate (inode, path, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.inode, self.path, data],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_subpar_duplicate WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_subpar_duplicate WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl CorpusSignalStore for CompoundTagSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_compound_tag (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_compound_tag";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.compounds)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_compound_tag (inode, path, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.inode, self.path, data],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_compound_tag WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_compound_tag WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

impl CompoundTagSignal {
    pub fn query_by_inode(conn: &Connection, inode: i64) -> Result<Option<Self>> {
        use rusqlite::OptionalExtension;
        conn.query_row(
            "SELECT inode, path, data FROM signal_compound_tag WHERE inode = ?1",
            rusqlite::params![inode],
            |row| {
                let blob: Vec<u8> = row.get(2)?;
                let compounds: Vec<CompoundTagEntry> = bincode::deserialize(&blob)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(Self { inode: row.get(0)?, path: row.get(1)?, compounds })
            },
        ).optional()
    }
}

impl CorpusSignalStore for ExpectedMissingTagSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_expected_missing_tag (
        inode INTEGER PRIMARY KEY,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_expected_missing_tag";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_expected_missing_tag (inode) VALUES (?1)",
            rusqlite::params![self.inode],
        )?;
        Ok(())
    }

    fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
        conn.execute("DELETE FROM signal_expected_missing_tag WHERE inode = ?1", [inode])?;
        Ok(())
    }

    fn exists(conn: &Connection, inode: i64) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_expected_missing_tag WHERE inode = ?1)",
            [inode],
            |row| row.get(0),
        )
    }
}

// ============================================================================
// Aggregate Signal Implementations
// ============================================================================

impl AggregateSignalStore for ExpectedOverlapSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_expected_overlap (
        key TEXT PRIMARY KEY,
        source_a TEXT NOT NULL,
        source_b TEXT NOT NULL,
        created_at TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_expected_overlap";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_expected_overlap (key, source_a, source_b, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![self.key, self.source_a, self.source_b, self.created_at],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_expected_overlap WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_expected_overlap WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl AggregateSignalStore for ExpectedDuplicateSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_expected_duplicate (
        key TEXT PRIMARY KEY,
        created_at TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_expected_duplicate";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_expected_duplicate (key, created_at) VALUES (?1, ?2)",
            rusqlite::params![self.key, self.created_at],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_expected_duplicate WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_expected_duplicate WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl AggregateSignalStore for CanonicalTagSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_canonical_tag (
        key TEXT PRIMARY KEY,
        tag_name TEXT NOT NULL,
        canonical_value TEXT NOT NULL,
        created_at TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_canonical_tag";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_canonical_tag (key, tag_name, canonical_value, created_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![self.key, self.tag_name, self.canonical_value, self.created_at],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_canonical_tag WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_canonical_tag WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl AggregateSignalStore for LibraryLeftoverSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_library_leftover (
        key TEXT PRIMARY KEY,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_library_leftover";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_library_leftover (key) VALUES (?1)",
            rusqlite::params![self.key],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_library_leftover WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_library_leftover WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl AggregateSignalStore for LibraryStaleSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_library_stale (
        key TEXT PRIMARY KEY,
        library_path TEXT NOT NULL,
        expected_path TEXT NOT NULL,
        corpus_path TEXT NOT NULL,
        inode INTEGER NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_library_stale";

    fn insert(&self, conn: &Connection) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO signal_library_stale (key, library_path, expected_path, corpus_path, inode) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![self.key, self.library_path, self.expected_path, self.corpus_path, self.inode],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_library_stale WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_library_stale WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

// --- Aggregate signals with bincode BLOB data ---

impl AggregateSignalStore for FingerprintOverlapSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_fingerprint_overlap (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_fingerprint_overlap";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.inodes)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_fingerprint_overlap (key, data) VALUES (?1, ?2)",
            rusqlite::params![self.key, data],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_fingerprint_overlap WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_fingerprint_overlap WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl FingerprintOverlapSignal {
    /// Query all fingerprint overlap signals with deserialized inodes.
    pub fn query_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT key, data FROM signal_fingerprint_overlap ORDER BY key"
        )?;
        let rows = stmt.query_map([], |row| {
            let key: String = row.get(0)?;
            let data: Vec<u8> = row.get(1)?;
            let inodes: Vec<i64> = bincode::deserialize(&data)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(
                    1, rusqlite::types::Type::Blob, Box::new(e)
                ))?;
            Ok(Self { key, inodes })
        })?;
        rows.collect()
    }
}

impl AggregateSignalStore for MetadataDuplicateSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_metadata_duplicate (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_metadata_duplicate";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.data)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_metadata_duplicate (key, data) VALUES (?1, ?2)",
            rusqlite::params![self.key, data],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_metadata_duplicate WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_metadata_duplicate WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl AggregateSignalStore for DuplicateInodeSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_duplicate_inode (
        key TEXT PRIMARY KEY,
        inode INTEGER NOT NULL,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_duplicate_inode";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.inodes)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_duplicate_inode (key, inode, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.key, self.inode, data],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_duplicate_inode WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_duplicate_inode WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl AggregateSignalStore for MissingTagSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_missing_tag (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_missing_tag";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.data)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_missing_tag (key, data) VALUES (?1, ?2)",
            rusqlite::params![self.key, data],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_missing_tag WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_missing_tag WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl MissingTagSignal {
    pub fn query_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT key, data FROM signal_missing_tag ORDER BY key"
        )?;
        let rows = stmt.query_map([], |row| {
            let blob: Vec<u8> = row.get(1)?;
            let data: MissingTagData = bincode::deserialize(&blob)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            Ok(Self {
                key: row.get(0)?,
                data,
            })
        })?;
        rows.collect()
    }
}

impl AggregateSignalStore for MissingAlbumSingleSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_missing_album_single (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_missing_album_single";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.data)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_missing_album_single (key, data) VALUES (?1, ?2)",
            rusqlite::params![self.key, data],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_missing_album_single WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_missing_album_single WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl MissingAlbumSingleSignal {
    pub fn query_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT key, data FROM signal_missing_album_single ORDER BY key"
        )?;
        let rows = stmt.query_map([], |row| {
            let blob: Vec<u8> = row.get(1)?;
            let data: MissingAlbumSingleData = bincode::deserialize(&blob)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            Ok(Self {
                key: row.get(0)?,
                data,
            })
        })?;
        rows.collect()
    }
}

impl AggregateSignalStore for DeployConflictSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_deploy_conflict (
        key TEXT PRIMARY KEY,
        deploy_path TEXT NOT NULL,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_deploy_conflict";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.inodes)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_deploy_conflict (key, deploy_path, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.key, self.deploy_path, data],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_deploy_conflict WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_deploy_conflict WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl AggregateSignalStore for TagCanonicitySignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_tag_canonicity (
        key TEXT PRIMARY KEY,
        tag_name TEXT NOT NULL,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_tag_canonicity";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.data)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_tag_canonicity (key, tag_name, data) VALUES (?1, ?2, ?3)",
            rusqlite::params![self.key, self.tag_name, data],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_tag_canonicity WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_tag_canonicity WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl TagCanonicitySignal {
    pub fn query_by_key(conn: &Connection, key: &str) -> Result<Option<Self>> {
        use rusqlite::OptionalExtension;
        conn.query_row(
            "SELECT key, tag_name, data FROM signal_tag_canonicity WHERE key = ?1",
            rusqlite::params![key],
            |row| {
                let blob: Vec<u8> = row.get(2)?;
                let data: TagCanonicityData = bincode::deserialize(&blob)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(Self { key: row.get(0)?, tag_name: row.get(1)?, data })
            },
        ).optional()
    }
}

impl AggregateSignalStore for InconsistentAlbumArtistSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_inconsistent_album_artist (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_inconsistent_album_artist";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.data)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_inconsistent_album_artist (key, data) VALUES (?1, ?2)",
            rusqlite::params![self.key, data],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_inconsistent_album_artist WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_inconsistent_album_artist WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl InconsistentAlbumArtistSignal {
    pub fn query_by_key(conn: &Connection, key: &str) -> Result<Option<Self>> {
        use rusqlite::OptionalExtension;
        conn.query_row(
            "SELECT key, data FROM signal_inconsistent_album_artist WHERE key = ?1",
            rusqlite::params![key],
            |row| {
                let blob: Vec<u8> = row.get(1)?;
                let data: InconsistentAlbumArtistData = bincode::deserialize(&blob)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(Self { key: row.get(0)?, data })
            },
        ).optional()
    }
}

impl AggregateSignalStore for CrossSourceOverlapSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_cross_source_overlap (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_cross_source_overlap";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.data)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_cross_source_overlap (key, data) VALUES (?1, ?2)",
            rusqlite::params![self.key, data],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_cross_source_overlap WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_cross_source_overlap WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl AggregateSignalStore for RedundantDuplicateSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_redundant_duplicate (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_redundant_duplicate";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.data)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_redundant_duplicate (key, data) VALUES (?1, ?2)",
            rusqlite::params![self.key, data],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_redundant_duplicate WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_redundant_duplicate WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl AggregateSignalStore for EmbeddableAlbumArtSignal {
    const TABLE_SQL: &'static str = "CREATE TABLE IF NOT EXISTS signal_embeddable_album_art (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )";
    const TABLE_NAME: &'static str = "signal_embeddable_album_art";

    fn insert(&self, conn: &Connection) -> Result<()> {
        let data = bincode::serialize(&self.data)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT OR REPLACE INTO signal_embeddable_album_art (key, data) VALUES (?1, ?2)",
            rusqlite::params![self.key, data],
        )?;
        Ok(())
    }

    fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
        conn.execute("DELETE FROM signal_embeddable_album_art WHERE key = ?1", [key])?;
        Ok(())
    }

    fn exists(conn: &Connection, key: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM signal_embeddable_album_art WHERE key = ?1)",
            [key],
            |row| row.get(0),
        )
    }
}

impl EmbeddableAlbumArtSignal {
    pub fn query_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT key, data FROM signal_embeddable_album_art ORDER BY key"
        )?;
        let rows = stmt.query_map([], |row| {
            let blob: Vec<u8> = row.get(1)?;
            let data: EmbeddableAlbumArtData = bincode::deserialize(&blob)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            Ok(Self {
                key: row.get(0)?,
                data,
            })
        })?;
        rows.collect()
    }
}

impl CrossSourceOverlapSignal {
    pub fn query_all(conn: &Connection) -> Result<Vec<Self>> {
        let mut stmt = conn.prepare(
            "SELECT key, data FROM signal_cross_source_overlap ORDER BY key"
        )?;
        let rows = stmt.query_map([], |row| {
            let blob: Vec<u8> = row.get(1)?;
            let data: CrossSourceOverlapData = bincode::deserialize(&blob)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            Ok(Self { key: row.get(0)?, data })
        })?;
        rows.collect()
    }
}

// ============================================================================
// Table Creation Helper
// ============================================================================

/// Create all per-signal tables. Idempotent (uses IF NOT EXISTS).
pub fn create_all_signal_tables(conn: &Connection) -> Result<()> {
    // Corpus file signals
    conn.execute_batch(FileInCorpusSignal::TABLE_SQL)?;
    conn.execute_batch(UnindexedFileSignal::TABLE_SQL)?;
    conn.execute_batch(HealthyFileSignal::TABLE_SQL)?;
    conn.execute_batch(CorruptFileSignal::TABLE_SQL)?;
    conn.execute_batch(MtimeOnlyMismatchSignal::TABLE_SQL)?;
    conn.execute_batch(MissingDirectorySignal::TABLE_SQL)?;
    conn.execute_batch(MissingFileSignal::TABLE_SQL)?;
    conn.execute_batch(MovedFileSignal::TABLE_SQL)?;
    conn.execute_batch(ShitFormatSignal::TABLE_SQL)?;
    conn.execute_batch(DeployReadySignal::TABLE_SQL)?;
    conn.execute_batch(DeployedHealthySignal::TABLE_SQL)?;
    conn.execute_batch(OutOfBandTagSyncSignal::TABLE_SQL)?;
    conn.execute_batch(OutOfBandTagConflictSignal::TABLE_SQL)?;
    conn.execute_batch(SubparDuplicateSignal::TABLE_SQL)?;
    conn.execute_batch(CompoundTagSignal::TABLE_SQL)?;
    conn.execute_batch(ExpectedMissingTagSignal::TABLE_SQL)?;

    // Aggregate signals
    conn.execute_batch(CanonicalTagSignal::TABLE_SQL)?;
    conn.execute_batch(ExpectedOverlapSignal::TABLE_SQL)?;
    conn.execute_batch(ExpectedDuplicateSignal::TABLE_SQL)?;
    conn.execute_batch(LibraryLeftoverSignal::TABLE_SQL)?;
    conn.execute_batch(LibraryStaleSignal::TABLE_SQL)?;
    conn.execute_batch(FingerprintOverlapSignal::TABLE_SQL)?;
    conn.execute_batch(MetadataDuplicateSignal::TABLE_SQL)?;
    conn.execute_batch(DuplicateInodeSignal::TABLE_SQL)?;
    conn.execute_batch(MissingTagSignal::TABLE_SQL)?;
    conn.execute_batch(DeployConflictSignal::TABLE_SQL)?;
    conn.execute_batch(TagCanonicitySignal::TABLE_SQL)?;
    conn.execute_batch(InconsistentAlbumArtistSignal::TABLE_SQL)?;
    conn.execute_batch(CrossSourceOverlapSignal::TABLE_SQL)?;
    conn.execute_batch(RedundantDuplicateSignal::TABLE_SQL)?;
    conn.execute_batch(EmbeddableAlbumArtSignal::TABLE_SQL)?;
    conn.execute_batch(MissingAlbumSingleSignal::TABLE_SQL)?;

    Ok(())
}
