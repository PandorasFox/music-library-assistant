//! Thread-local database connection management for computation workers.

use std::cell::RefCell;

// ============================================================================
// Thread-Local Storage
// ============================================================================

// Thread-local cached READ-ONLY database connection for computation workers.
// Each worker thread opens once and reuses, eliminating connection overhead.
//
// IMPORTANT: This connection has `PRAGMA query_only = ON` set.
// Any write operations (INSERT, UPDATE, DELETE) will SILENTLY FAIL.
// All writes from computations must go through `write_thread::signal_sender()`.
thread_local! {
    static THREAD_READ_ONLY_DB: RefCell<Option<crate::db::Database>> = const { RefCell::new(None) };
}

// ============================================================================
// Public API
// ============================================================================

/// Close the thread-local database connection on the current thread.
///
/// Called via `rayon::broadcast()` during shutdown to ensure all worker
/// thread connections are closed before the db_thread attempts WAL checkpoint.
pub fn close_thread_local_connection() {
    THREAD_READ_ONLY_DB.with(|cell| {
        cell.borrow_mut().take();
    });
}

/// Execute a function with the thread-local READ-ONLY database connection.
/// Opens and caches the connection on first use per thread.
///
/// # IMPORTANT: Read-Only Access
///
/// This connection uses `PRAGMA query_only = ON`. All write operations
/// (INSERT, UPDATE, DELETE) will fail. Computations and mutations that need
/// to persist data must route writes through `write_thread::signal_sender()`.
///
/// The `read_only_db` parameter name in executor functions reflects this.
///
/// # Usage
///
/// This function is used by both computation and mutation worker threads
/// to share a single read-only connection per thread, avoiding the overhead
/// of opening new connections for each task.
pub fn with_read_only_db<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&crate::db::ReadOnlyDb<'_>) -> T,
{
    use crate::config;
    use crate::db::{Database, ReadOnlyDb};

    THREAD_READ_ONLY_DB.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.is_none() {
            let db_path = config::get_db_path().map_err(|e| e.to_string())?;
            let db = Database::open_read_only(&db_path).map_err(|e| e.to_string())?;
            *opt = Some(db);
        }

        let read_only_db = ReadOnlyDb::new(opt.as_ref().unwrap());
        Ok(f(&read_only_db))
    })
}
