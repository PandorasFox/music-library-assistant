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
use serde::de::DeserializeOwned;
use std::collections::HashMap;
/// Compute a deterministic hash from bincode-serialized bytes.
///
/// Uses FNV-1a (64-bit), a simple non-cryptographic hash with a fixed algorithm.
/// Unlike `DefaultHasher`, FNV-1a is stable across Rust compiler versions —
/// `DefaultHasher`'s algorithm is explicitly not guaranteed stable, so stored
/// hash values could silently become stale after a toolchain update, triggering
/// spurious re-emission of every signal.
fn compute_blob_hash(bytes: &[u8]) -> i64 {
    const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x00000100000001B3;

    let mut hash = FNV_OFFSET_BASIS;
    for &byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash as i64
}

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

    /// Query all inodes that have signals in this table.
    fn all_inodes(conn: &Connection) -> Result<Vec<i64>> {
        let sql = format!("SELECT inode FROM {}", Self::TABLE_NAME);
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Delete all signals from this table.
    fn clear_all(conn: &Connection) -> Result<usize> {
        conn.execute(&format!("DELETE FROM {}", Self::TABLE_NAME), [])
    }

    /// Query inode→data_hash map for BLOB signal types.
    ///
    /// Default returns empty map (scalar-only signal types).
    /// BLOB signal types override this to SELECT inode, data_hash.
    fn query_inode_hashes(conn: &Connection) -> Result<HashMap<i64, i64>> {
        let _ = conn;
        Ok(HashMap::new())
    }

    /// Query the data_hash for a single inode, if the signal exists.
    ///
    /// Default impl uses TABLE_NAME for a point query. BLOB signal types
    /// get this for free from their `data_hash` column.
    fn query_inode_hash(conn: &Connection, inode: i64) -> Result<Option<i64>> {
        use rusqlite::OptionalExtension;
        let sql = format!(
            "SELECT data_hash FROM {} WHERE inode = ?1",
            Self::TABLE_NAME
        );
        conn.query_row(&sql, [inode], |row| row.get(0)).optional()
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

    /// Delete all signals from this table.
    fn clear_all(conn: &Connection) -> Result<usize> {
        conn.execute(&format!("DELETE FROM {}", Self::TABLE_NAME), [])
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

    /// Query key→data_hash map for BLOB signal types.
    ///
    /// Default returns empty map (scalar-only signal types).
    /// BLOB signal types override this to SELECT key, data_hash.
    fn query_key_hashes(conn: &Connection) -> Result<HashMap<String, i64>> {
        let _ = conn;
        Ok(HashMap::new())
    }
}

// ============================================================================
// Declarative macros for signal store implementations
// ============================================================================

/// Helper to convert a field access into its SQL-ready value.
/// Most fields just use `self.field`, but `sql_via` fields call `.as_str()`.
macro_rules! field_value {
    ($self:ident, $field:ident) => { &$self.$field };
    ($self:ident, $field:ident, sql_via) => { $self.$field.as_str() };
}

/// Implement `CorpusSignalStore` for a corpus signal.
///
/// # Flat columns only
/// ```ignore
/// impl_corpus_signal!(StructName, "table_name", TABLE_SQL_EXPR,
///     insert_sql: "INSERT OR REPLACE INTO ...",
///     fields: [field1, field2],
/// );
/// ```
///
/// # With blob
/// ```ignore
/// impl_corpus_signal!(StructName, "table_name", TABLE_SQL_EXPR,
///     insert_sql: "INSERT OR REPLACE INTO ...",
///     fields: [field1, field2],
///     blob: blob_field,
/// );
/// ```
///
/// # With sql_via (enum fields using .as_str())
/// Use `field_name via sql_via` syntax in fields list.
macro_rules! impl_corpus_signal {
    // Flat columns only (no blob)
    ($ty:ty, $table:literal, $table_sql:expr,
     insert_sql: $insert_sql:literal,
     fields: [$($field:ident $(via $via:ident)?), * $(, )?] $(,)?
    ) => {
        impl CorpusSignalStore for $ty {
            const TABLE_SQL: &'static str = $table_sql;
            const TABLE_NAME: &'static str = $table;

            fn insert(&self, conn: &Connection) -> Result<()> {
                conn.execute(
                    $insert_sql,
                    rusqlite::params![$(field_value!(self, $field $(, $via)?)),*],
                )?;
                Ok(())
            }

            fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
                conn.execute(
                    concat!("DELETE FROM ", $table, " WHERE inode = ?1"),
                    [inode],
                )?;
                Ok(())
            }

            fn exists(conn: &Connection, inode: i64) -> Result<bool> {
                conn.query_row(
                    concat!("SELECT EXISTS(SELECT 1 FROM ", $table, " WHERE inode = ?1)"),
                    [inode],
                    |row| row.get(0),
                )
            }
        }
    };
    // With blob field
    ($ty:ty, $table:literal, $table_sql:expr,
     insert_sql: $insert_sql:literal,
     fields: [$($field:ident $(via $via:ident)?), * $(, )?],
     blob: $blob_field:ident $(,)?
    ) => {
        impl CorpusSignalStore for $ty {
            const TABLE_SQL: &'static str = $table_sql;
            const TABLE_NAME: &'static str = $table;

            fn insert(&self, conn: &Connection) -> Result<()> {
                let data = bincode::serialize(&self.$blob_field)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                let hash = compute_blob_hash(&data);
                conn.execute(
                    $insert_sql,
                    rusqlite::params![$(field_value!(self, $field $(, $via)?),)* data, hash],
                )?;
                Ok(())
            }

            fn query_inode_hashes(conn: &Connection) -> Result<HashMap<i64, i64>> {
                let mut stmt = conn.prepare(
                    concat!("SELECT inode, data_hash FROM ", $table)
                )?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect()
            }

            fn clear_by_inode(conn: &Connection, inode: i64) -> Result<()> {
                conn.execute(
                    concat!("DELETE FROM ", $table, " WHERE inode = ?1"),
                    [inode],
                )?;
                Ok(())
            }

            fn exists(conn: &Connection, inode: i64) -> Result<bool> {
                conn.query_row(
                    concat!("SELECT EXISTS(SELECT 1 FROM ", $table, " WHERE inode = ?1)"),
                    [inode],
                    |row| row.get(0),
                )
            }
        }
    };
}

/// Implement `AggregateSignalStore` for an aggregate signal.
///
/// Same field syntax as `impl_corpus_signal!`.
macro_rules! impl_aggregate_signal {
    // Flat columns only (no blob)
    ($ty:ty, $table:literal, $table_sql:expr,
     insert_sql: $insert_sql:literal,
     fields: [$($field:ident $(via $via:ident)?), * $(, )?] $(,)?
    ) => {
        impl AggregateSignalStore for $ty {
            const TABLE_SQL: &'static str = $table_sql;
            const TABLE_NAME: &'static str = $table;

            fn insert(&self, conn: &Connection) -> Result<()> {
                conn.execute(
                    $insert_sql,
                    rusqlite::params![$(field_value!(self, $field $(, $via)?)),*],
                )?;
                Ok(())
            }

            fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
                conn.execute(
                    concat!("DELETE FROM ", $table, " WHERE key = ?1"),
                    [key],
                )?;
                Ok(())
            }

            fn exists(conn: &Connection, key: &str) -> Result<bool> {
                conn.query_row(
                    concat!("SELECT EXISTS(SELECT 1 FROM ", $table, " WHERE key = ?1)"),
                    [key],
                    |row| row.get(0),
                )
            }
        }
    };
    // With blob field
    ($ty:ty, $table:literal, $table_sql:expr,
     insert_sql: $insert_sql:literal,
     fields: [$($field:ident $(via $via:ident)?), * $(, )?],
     blob: $blob_field:ident $(,)?
    ) => {
        impl AggregateSignalStore for $ty {
            const TABLE_SQL: &'static str = $table_sql;
            const TABLE_NAME: &'static str = $table;

            fn insert(&self, conn: &Connection) -> Result<()> {
                let data = bincode::serialize(&self.$blob_field)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                let hash = compute_blob_hash(&data);
                conn.execute(
                    $insert_sql,
                    rusqlite::params![$(field_value!(self, $field $(, $via)?),)* data, hash],
                )?;
                Ok(())
            }

            fn query_key_hashes(conn: &Connection) -> Result<HashMap<String, i64>> {
                let mut stmt = conn.prepare(
                    concat!("SELECT key, data_hash FROM ", $table)
                )?;
                let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
                rows.collect()
            }

            fn clear_by_key(conn: &Connection, key: &str) -> Result<()> {
                conn.execute(
                    concat!("DELETE FROM ", $table, " WHERE key = ?1"),
                    [key],
                )?;
                Ok(())
            }

            fn exists(conn: &Connection, key: &str) -> Result<bool> {
                conn.query_row(
                    concat!("SELECT EXISTS(SELECT 1 FROM ", $table, " WHERE key = ?1)"),
                    [key],
                    |row| row.get(0),
                )
            }
        }
    };
}

/// Deserialize a bincode BLOB from a SQLite row value.
fn deserialize_blob<T: DeserializeOwned>(blob: &[u8]) -> Result<T> {
    bincode::deserialize(blob)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Blob, Box::new(e)))
}

/// Generate typed query methods for signal structs.
///
/// Avoids hand-writing the identical `query_all`, `query_by_key`, and
/// `query_by_inode` boilerplate on each signal type. Field names are used
/// to build both the SELECT column list and the `Self { ... }` constructor.
///
/// # Arms
/// - `all_flat` — `query_all` for flat corpus signals (no BLOB)
/// - `all` — `query_all` for aggregate blob signals (ORDER BY key)
/// - `by_key` — `query_by_key` for aggregate blob signals
/// - `by_inode` — `query_by_inode` for corpus blob signals
macro_rules! impl_signal_query {
    (all_flat, $ty:ty, $table:literal, [$($field:ident),+]) => {
        impl $ty {
            pub fn query_all(conn: &Connection) -> Result<Vec<Self>> {
                let mut stmt = conn.prepare(concat!(
                    "SELECT ", impl_signal_query!(@join $($field),+),
                    " FROM ", $table, " ORDER BY path"
                ))?;
                let rows = stmt.query_map([], |row| {
                    let mut _idx = 0usize;
                    $(let $field = row.get({ let i = _idx; _idx += 1; i })?;)*
                    Ok(Self { $($field),+ })
                })?;
                rows.collect()
            }
        }
    };

    (all, $ty:ty, $table:literal, [$($field:ident),+], $blob_field:ident) => {
        impl $ty {
            pub fn query_all(conn: &Connection) -> Result<Vec<Self>> {
                let mut stmt = conn.prepare(concat!(
                    "SELECT ", impl_signal_query!(@join $($field),+),
                    ", data FROM ", $table, " ORDER BY key"
                ))?;
                let rows = stmt.query_map([], |row| {
                    let mut _idx = 0usize;
                    $(let $field = row.get({ let i = _idx; _idx += 1; i })?;)*
                    let _blob: Vec<u8> = row.get(_idx)?;
                    let $blob_field = deserialize_blob(&_blob)?;
                    Ok(Self { $($field,)+ $blob_field })
                })?;
                rows.collect()
            }
        }
    };

    (by_key, $ty:ty, $table:literal, [$($field:ident),+], $blob_field:ident) => {
        impl $ty {
            pub fn query_by_key(conn: &Connection, key: &str) -> Result<Option<Self>> {
                use rusqlite::OptionalExtension;
                conn.query_row(
                    concat!(
                        "SELECT ", impl_signal_query!(@join $($field),+),
                        ", data FROM ", $table, " WHERE key = ?1"
                    ),
                    rusqlite::params![key],
                    |row| {
                        let mut _idx = 0usize;
                        $(let $field = row.get({ let i = _idx; _idx += 1; i })?;)*
                        let _blob: Vec<u8> = row.get(_idx)?;
                        let $blob_field = deserialize_blob(&_blob)?;
                        Ok(Self { $($field,)+ $blob_field })
                    },
                ).optional()
            }
        }
    };

    (by_inode, $ty:ty, $table:literal, [$($field:ident),+], $blob_field:ident) => {
        impl $ty {
            pub fn query_by_inode(conn: &Connection, inode: i64) -> Result<Option<Self>> {
                use rusqlite::OptionalExtension;
                conn.query_row(
                    concat!(
                        "SELECT ", impl_signal_query!(@join $($field),+),
                        ", data FROM ", $table, " WHERE inode = ?1"
                    ),
                    rusqlite::params![inode],
                    |row| {
                        let mut _idx = 0usize;
                        $(let $field = row.get({ let i = _idx; _idx += 1; i })?;)*
                        let _blob: Vec<u8> = row.get(_idx)?;
                        let $blob_field = deserialize_blob(&_blob)?;
                        Ok(Self { $($field,)+ $blob_field })
                    },
                ).optional()
            }
        }
    };

    (@join $f:ident) => { stringify!($f) };
    (@join $f:ident, $($rest:ident),+) => {
        concat!(stringify!($f), ", ", impl_signal_query!(@join $($rest),+))
    };
}

// ============================================================================
// Corpus File Signal Implementations
// ============================================================================

use super::data::*;

// --- Simple signals (flat columns only) ---

impl_corpus_signal!(FileInCorpusSignal, "signal_file_in_corpus",
    "CREATE TABLE IF NOT EXISTS signal_file_in_corpus (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        generation INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_file_in_corpus (inode, path, generation) VALUES (?1, ?2, ?3)",
    fields: [inode, path, generation],
);

impl_corpus_signal!(UnindexedFileSignal, "signal_unindexed_file",
    "CREATE TABLE IF NOT EXISTS signal_unindexed_file (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_unindexed_file (inode, path) VALUES (?1, ?2)",
    fields: [inode, path],
);

impl_signal_query!(all_flat, UnindexedFileSignal, "signal_unindexed_file", [inode, path]);

impl_corpus_signal!(HealthyFileSignal, "signal_healthy_file",
    "CREATE TABLE IF NOT EXISTS signal_healthy_file (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_healthy_file (inode, path) VALUES (?1, ?2)",
    fields: [inode, path],
);

impl_signal_query!(all_flat, HealthyFileSignal, "signal_healthy_file", [inode, path]);

// ============================================================================
// Inbox file signal stores
// ============================================================================

impl_corpus_signal!(FileInInboxSignal, "signal_file_in_inbox",
    "CREATE TABLE IF NOT EXISTS signal_file_in_inbox (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        generation INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_file_in_inbox (inode, path, generation) VALUES (?1, ?2, ?3)",
    fields: [inode, path, generation],
);

impl_corpus_signal!(InboxUnindexedSignal, "signal_inbox_unindexed",
    "CREATE TABLE IF NOT EXISTS signal_inbox_unindexed (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_inbox_unindexed (inode, path) VALUES (?1, ?2)",
    fields: [inode, path],
);

impl_corpus_signal!(InboxHealthySignal, "signal_inbox_healthy",
    "CREATE TABLE IF NOT EXISTS signal_inbox_healthy (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_inbox_healthy (inode, path) VALUES (?1, ?2)",
    fields: [inode, path],
);

impl_corpus_signal!(InboxCorpusMatchSignal, "signal_inbox_corpus_match",
    "CREATE TABLE IF NOT EXISTS signal_inbox_corpus_match (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        classification TEXT NOT NULL DEFAULT 'equivalent',
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_inbox_corpus_match (inode, path, classification, data, data_hash) VALUES (?1, ?2, ?3, ?4, ?5)",
    fields: [inode, path, classification via sql_via],
    blob: data,
);

// ============================================================================
// Corpus health signal stores
// ============================================================================

impl_corpus_signal!(CorruptFileSignal, "signal_corrupt_file",
    "CREATE TABLE IF NOT EXISTS signal_corrupt_file (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_corrupt_file (inode, path) VALUES (?1, ?2)",
    fields: [inode, path],
);

impl_corpus_signal!(MtimeOnlyMismatchSignal, "signal_mtime_only_mismatch",
    "CREATE TABLE IF NOT EXISTS signal_mtime_only_mismatch (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_mtime_only_mismatch (inode, path) VALUES (?1, ?2)",
    fields: [inode, path],
);

impl_corpus_signal!(MissingDirectorySignal, "signal_missing_directory",
    "CREATE TABLE IF NOT EXISTS signal_missing_directory (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_missing_directory (inode, path) VALUES (?1, ?2)",
    fields: [inode, path],
);

impl_corpus_signal!(ExpectedMissingTagSignal, "signal_expected_missing_tag",
    "CREATE TABLE IF NOT EXISTS signal_expected_missing_tag (
        inode INTEGER PRIMARY KEY,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_expected_missing_tag (inode) VALUES (?1)",
    fields: [inode],
);

impl_corpus_signal!(MissingFileSignal, "signal_missing_file",
    "CREATE TABLE IF NOT EXISTS signal_missing_file (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        replaced_by_inode INTEGER,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_missing_file (inode, path, replaced_by_inode) VALUES (?1, ?2, ?3)",
    fields: [inode, path, replaced_by_inode],
);

impl_corpus_signal!(MovedFileSignal, "signal_moved_file",
    "CREATE TABLE IF NOT EXISTS signal_moved_file (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        old_path TEXT NOT NULL,
        old_zone TEXT NOT NULL DEFAULT 'corpus',
        new_zone TEXT NOT NULL DEFAULT 'corpus',
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_moved_file (inode, path, old_path, old_zone, new_zone) VALUES (?1, ?2, ?3, ?4, ?5)",
    fields: [inode, path, old_path, old_zone, new_zone],
);

impl_corpus_signal!(LosslessRemuxSignal, "signal_lossless_remux",
    "CREATE TABLE IF NOT EXISTS signal_lossless_remux (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        file_type TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_lossless_remux (inode, path, file_type) VALUES (?1, ?2, ?3)",
    fields: [inode, path, file_type],
);

impl_corpus_signal!(DeployReadySignal, "signal_deploy_ready",
    "CREATE TABLE IF NOT EXISTS signal_deploy_ready (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        deploy_path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_deploy_ready (inode, path, deploy_path) VALUES (?1, ?2, ?3)",
    fields: [inode, path, deploy_path],
);

impl_corpus_signal!(DeployedHealthySignal, "signal_deployed_healthy",
    "CREATE TABLE IF NOT EXISTS signal_deployed_healthy (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        library_path TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_deployed_healthy (inode, path, library_path) VALUES (?1, ?2, ?3)",
    fields: [inode, path, library_path],
);

impl_corpus_signal!(SidecarDeployReadySignal, "signal_sidecar_deploy_ready",
    "CREATE TABLE IF NOT EXISTS signal_sidecar_deploy_ready (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        deploy_path TEXT NOT NULL,
        library_name TEXT NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_sidecar_deploy_ready (inode, path, deploy_path, library_name, data, data_hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    fields: [inode, path, deploy_path, library_name],
    blob: data,
);

impl_corpus_signal!(OutOfBandTagSyncSignal, "signal_oob_tag_sync",
    "CREATE TABLE IF NOT EXISTS signal_oob_tag_sync (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_oob_tag_sync (inode, path, data, data_hash) VALUES (?1, ?2, ?3, ?4)",
    fields: [inode, path],
    blob: mismatches,
);

impl_corpus_signal!(OutOfBandTagConflictSignal, "signal_oob_tag_conflict",
    "CREATE TABLE IF NOT EXISTS signal_oob_tag_conflict (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_oob_tag_conflict (inode, path, data, data_hash) VALUES (?1, ?2, ?3, ?4)",
    fields: [inode, path],
    blob: mismatches,
);

impl_corpus_signal!(SubparDuplicateSignal, "signal_subpar_duplicate",
    "CREATE TABLE IF NOT EXISTS signal_subpar_duplicate (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_subpar_duplicate (inode, path, data, data_hash) VALUES (?1, ?2, ?3, ?4)",
    fields: [inode, path],
    blob: data,
);

impl_corpus_signal!(CompoundTagSignal, "signal_compound_tag",
    "CREATE TABLE IF NOT EXISTS signal_compound_tag (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_compound_tag (inode, path, data, data_hash) VALUES (?1, ?2, ?3, ?4)",
    fields: [inode, path],
    blob: compounds,
);

impl_signal_query!(by_inode, CompoundTagSignal, "signal_compound_tag", [inode, path], compounds);

impl_corpus_signal!(PathTagMismatchSignal, "signal_path_tag_mismatch",
    "CREATE TABLE IF NOT EXISTS signal_path_tag_mismatch (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_path_tag_mismatch (inode, path, data, data_hash) VALUES (?1, ?2, ?3, ?4)",
    fields: [inode, path],
    blob: data,
);

impl_corpus_signal!(ExternalMatchSignal, "signal_external_match",
    "CREATE TABLE IF NOT EXISTS signal_external_match (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_external_match (inode, path, data, data_hash) VALUES (?1, ?2, ?3, ?4)",
    fields: [inode, path],
    blob: data,
);

impl_corpus_signal!(ReleasePackingSignal, "signal_release_packing",
    "CREATE TABLE IF NOT EXISTS signal_release_packing (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_release_packing (inode, path, data, data_hash) VALUES (?1, ?2, ?3, ?4)",
    fields: [inode, path],
    blob: data,
);

impl_corpus_signal!(InboxCompoundTagSignal, "signal_inbox_compound_tag",
    "CREATE TABLE IF NOT EXISTS signal_inbox_compound_tag (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_inbox_compound_tag (inode, path, data, data_hash) VALUES (?1, ?2, ?3, ?4)",
    fields: [inode, path],
    blob: compounds,
);

impl_signal_query!(by_inode, InboxCompoundTagSignal, "signal_inbox_compound_tag", [inode, path], compounds);

impl_corpus_signal!(UnmatchedCorpusTrackSignal, "signal_unmatched_corpus_track",
    "CREATE TABLE IF NOT EXISTS signal_unmatched_corpus_track (
        inode INTEGER PRIMARY KEY,
        path TEXT NOT NULL,
        category TEXT NOT NULL DEFAULT 'no_match',
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_unmatched_corpus_track (inode, path, category, data, data_hash) VALUES (?1, ?2, ?3, ?4, ?5)",
    fields: [inode, path, category via sql_via],
    blob: data,
);

// ============================================================================
// Aggregate Signal Implementations
// ============================================================================

// --- Simple aggregate signals (flat columns only) ---

impl_aggregate_signal!(ExpectedOverlapSignal, "signal_expected_overlap",
    "CREATE TABLE IF NOT EXISTS signal_expected_overlap (
        key TEXT PRIMARY KEY,
        source_a TEXT NOT NULL,
        source_b TEXT NOT NULL,
        created_at TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_expected_overlap (key, source_a, source_b, created_at) VALUES (?1, ?2, ?3, ?4)",
    fields: [key, source_a, source_b, created_at],
);

impl_aggregate_signal!(ExpectedDuplicateSignal, "signal_expected_duplicate",
    "CREATE TABLE IF NOT EXISTS signal_expected_duplicate (
        key TEXT PRIMARY KEY,
        created_at TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_expected_duplicate (key, created_at) VALUES (?1, ?2)",
    fields: [key, created_at],
);

impl_aggregate_signal!(CanonicalTagSignal, "signal_canonical_tag",
    "CREATE TABLE IF NOT EXISTS signal_canonical_tag (
        key TEXT PRIMARY KEY,
        tag_name TEXT NOT NULL,
        canonical_value TEXT NOT NULL,
        created_at TEXT NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_canonical_tag (key, tag_name, canonical_value, created_at) VALUES (?1, ?2, ?3, ?4)",
    fields: [key, tag_name, canonical_value, created_at],
);

impl_aggregate_signal!(LibraryLeftoverSignal, "signal_library_leftover",
    "CREATE TABLE IF NOT EXISTS signal_library_leftover (
        key TEXT PRIMARY KEY,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_library_leftover (key) VALUES (?1)",
    fields: [key],
);

impl_aggregate_signal!(LibraryStaleSignal, "signal_library_stale",
    "CREATE TABLE IF NOT EXISTS signal_library_stale (
        key TEXT PRIMARY KEY,
        library_path TEXT NOT NULL,
        expected_path TEXT NOT NULL,
        corpus_path TEXT NOT NULL,
        inode INTEGER NOT NULL,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_library_stale (key, library_path, expected_path, corpus_path, inode) VALUES (?1, ?2, ?3, ?4, ?5)",
    fields: [key, library_path, expected_path, corpus_path, inode],
);

// --- Aggregate signals with bincode BLOB data ---

impl_aggregate_signal!(FingerprintOverlapSignal, "signal_fingerprint_overlap",
    "CREATE TABLE IF NOT EXISTS signal_fingerprint_overlap (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_fingerprint_overlap (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: inodes,
);

impl_signal_query!(all, FingerprintOverlapSignal, "signal_fingerprint_overlap", [key], inodes);

impl_aggregate_signal!(MetadataDuplicateSignal, "signal_metadata_duplicate",
    "CREATE TABLE IF NOT EXISTS signal_metadata_duplicate (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_metadata_duplicate (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_aggregate_signal!(DuplicateInodeSignal, "signal_duplicate_inode",
    "CREATE TABLE IF NOT EXISTS signal_duplicate_inode (
        key TEXT PRIMARY KEY,
        inode INTEGER NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_duplicate_inode (key, inode, data, data_hash) VALUES (?1, ?2, ?3, ?4)",
    fields: [key, inode],
    blob: inodes,
);

impl_aggregate_signal!(MissingTagSignal, "signal_missing_tag",
    "CREATE TABLE IF NOT EXISTS signal_missing_tag (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_missing_tag (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_signal_query!(all, MissingTagSignal, "signal_missing_tag", [key], data);

impl_aggregate_signal!(MissingAlbumSingleSignal, "signal_missing_album_single",
    "CREATE TABLE IF NOT EXISTS signal_missing_album_single (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_missing_album_single (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_signal_query!(all, MissingAlbumSingleSignal, "signal_missing_album_single", [key], data);

impl_aggregate_signal!(DeployConflictSignal, "signal_deploy_conflict",
    "CREATE TABLE IF NOT EXISTS signal_deploy_conflict (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_deploy_conflict (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: inodes,
);

impl_aggregate_signal!(SidecarDeployConflictSignal, "signal_sidecar_deploy_conflict",
    "CREATE TABLE IF NOT EXISTS signal_sidecar_deploy_conflict (
        key TEXT PRIMARY KEY,
        deploy_path TEXT NOT NULL,
        library_name TEXT NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_sidecar_deploy_conflict (key, deploy_path, library_name, data, data_hash) VALUES (?1, ?2, ?3, ?4, ?5)",
    fields: [key, deploy_path, library_name],
    blob: inodes,
);

impl_aggregate_signal!(TagCanonicitySignal, "signal_tag_canonicity",
    "CREATE TABLE IF NOT EXISTS signal_tag_canonicity (
        key TEXT PRIMARY KEY,
        tag_name TEXT NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_tag_canonicity (key, tag_name, data, data_hash) VALUES (?1, ?2, ?3, ?4)",
    fields: [key, tag_name],
    blob: data,
);

impl_signal_query!(by_key, TagCanonicitySignal, "signal_tag_canonicity", [key, tag_name], data);

impl_aggregate_signal!(InconsistentAlbumArtistSignal, "signal_inconsistent_album_artist",
    "CREATE TABLE IF NOT EXISTS signal_inconsistent_album_artist (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_inconsistent_album_artist (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_signal_query!(by_key, InconsistentAlbumArtistSignal, "signal_inconsistent_album_artist", [key], data);

impl_aggregate_signal!(CrossSourceOverlapSignal, "signal_cross_source_overlap",
    "CREATE TABLE IF NOT EXISTS signal_cross_source_overlap (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_cross_source_overlap (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_signal_query!(all, CrossSourceOverlapSignal, "signal_cross_source_overlap", [key], data);

impl_aggregate_signal!(ReleaseOverlapSignal, "signal_release_overlap",
    "CREATE TABLE IF NOT EXISTS signal_release_overlap (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_release_overlap (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_signal_query!(all, ReleaseOverlapSignal, "signal_release_overlap", [key], data);

impl_aggregate_signal!(RedundantDuplicateSignal, "signal_redundant_duplicate",
    "CREATE TABLE IF NOT EXISTS signal_redundant_duplicate (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_redundant_duplicate (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_aggregate_signal!(InboxTagCanonicitySignal, "signal_inbox_tag_canonicity",
    "CREATE TABLE IF NOT EXISTS signal_inbox_tag_canonicity (
        key TEXT PRIMARY KEY,
        tag_name TEXT NOT NULL,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_inbox_tag_canonicity (key, tag_name, data, data_hash) VALUES (?1, ?2, ?3, ?4)",
    fields: [key, tag_name],
    blob: data,
);

impl_signal_query!(by_key, InboxTagCanonicitySignal, "signal_inbox_tag_canonicity", [key, tag_name], data);

impl_aggregate_signal!(InboxMissingTagSignal, "signal_inbox_missing_tag",
    "CREATE TABLE IF NOT EXISTS signal_inbox_missing_tag (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_inbox_missing_tag (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_aggregate_signal!(DiscExtractionSignal, "signal_disc_extraction",
    "CREATE TABLE IF NOT EXISTS signal_disc_extraction (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_disc_extraction (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_signal_query!(all, DiscExtractionSignal, "signal_disc_extraction", [key], data);

// ============================================================================
// Release Packing Gap Analysis Signal Implementations
// ============================================================================

impl_aggregate_signal!(UnfilledReleaseSlotSignal, "signal_unfilled_release_slot",
    "CREATE TABLE IF NOT EXISTS signal_unfilled_release_slot (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_unfilled_release_slot (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_aggregate_signal!(PackedReleaseSignal, "signal_packed_release",
    "CREATE TABLE IF NOT EXISTS signal_packed_release (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_packed_release (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_aggregate_signal!(PackingKnotSignal, "signal_packing_knot",
    "CREATE TABLE IF NOT EXISTS signal_packing_knot (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_packing_knot (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_aggregate_signal!(AlternativeReleasePackingSignal, "signal_alternative_release_packing",
    "CREATE TABLE IF NOT EXISTS signal_alternative_release_packing (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_alternative_release_packing (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_aggregate_signal!(VariousArtistsOverrideSignal, "signal_various_artists_override",
    "CREATE TABLE IF NOT EXISTS signal_various_artists_override (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_various_artists_override (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_aggregate_signal!(PinnedReleaseConflictSignal, "signal_pinned_release_conflict",
    "CREATE TABLE IF NOT EXISTS signal_pinned_release_conflict (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_pinned_release_conflict (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

impl_aggregate_signal!(SameRecordingDifferentReleaseSignal, "signal_same_recording_different_release",
    "CREATE TABLE IF NOT EXISTS signal_same_recording_different_release (
        key TEXT PRIMARY KEY,
        data BLOB NOT NULL,
        data_hash INTEGER NOT NULL DEFAULT 0,
        discovered_at DATETIME DEFAULT CURRENT_TIMESTAMP
    )",
    insert_sql: "INSERT OR REPLACE INTO signal_same_recording_different_release (key, data, data_hash) VALUES (?1, ?2, ?3)",
    fields: [key],
    blob: data,
);

// Table creation is now handled by `db::table_schema::schema_inventory()`.
// Signal TABLE_SQL consts on each type remain as the source of truth,
// referenced by the inventory entries.
