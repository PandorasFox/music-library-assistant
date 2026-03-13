//! Structured logging with category-based routing to separate log files.
//!
//! ## Architecture
//!
//! An early mpsc channel is created in `main()` before anything else.
//! All log calls send through a global `OnceLock<Sender>`. When the Witch
//! is created, She takes ownership of the receiver and spawns a dedicated
//! logging thread that routes entries to separate files by category.
//!
//! Pre-Witch messages queue in the unbounded channel buffer and are
//! drained when the logging thread starts.
//!
//! ## Log Files
//!
//! All under `~/.local/share/mm/logs/`:
//! - `general.log` - Startup, state transitions, compute, UI, db_thread
//! - `mutations.log` - Mutation execution lifecycle, transaction details
//! - `errors.log` - All errors and warnings (also mirrored to general.log)

// Re-export public logging API from mm-meta
pub use mm_meta::logging::{
    init_log_channel, log_error, log_general, log_mutation, request_shutdown, LogCategory, LogOp,
};

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;

use mm_utils::paths::get_logs_dir;

// ============================================================================
// Logging Thread
// ============================================================================

/// Handle returned by `spawn_log_thread()` for shutdown coordination.
pub(crate) struct LogThreadHandle {
    thread_handle: Option<JoinHandle<()>>,
}

impl crate::witch::types::ManagedThread for LogThreadHandle {
    fn send_shutdown(&self) {
        request_shutdown();
    }

    fn take_handle(&mut self) -> Option<JoinHandle<()>> {
        self.thread_handle.take()
    }
}

/// Spawn the logging thread. Called by Witch::new().
///
/// The thread takes ownership of the receiver and routes entries to files.
/// Pre-Witch messages that queued in the buffer are drained immediately.
pub(crate) fn spawn_log_thread(rx: Receiver<LogOp>) -> LogThreadHandle {
    let handle = std::thread::spawn(move || {
        run_log_thread(rx);
    });
    LogThreadHandle {
        thread_handle: Some(handle),
    }
}

/// Main loop for the logging thread. Opens file handles and routes entries.
fn run_log_thread(rx: Receiver<LogOp>) {
    // Open log directory and files
    let logs_dir = match get_logs_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("FATAL: Cannot create logs directory: {}", e);
            return;
        }
    };

    let mut general_file = open_log_file(&logs_dir, "general.log");
    let mut mutations_file = open_log_file(&logs_dir, "mutations.log");
    let mut errors_file = open_log_file(&logs_dir, "errors.log");

    // Drain the channel until shutdown or channel close
    while let Ok(op) = rx.recv() {
        match op {
            LogOp::Entry(entry) => {
                let line = format!(
                    "[{}] {}\n",
                    entry.timestamp.format("%Y-%m-%d %H:%M:%S"),
                    entry.message
                );

                match entry.category {
                    LogCategory::General => {
                        let _ = general_file.write_all(line.as_bytes());
                    }
                    LogCategory::Mutation => {
                        let _ = mutations_file.write_all(line.as_bytes());
                    }
                    LogCategory::Error => {
                        let _ = errors_file.write_all(line.as_bytes());
                        // Mirror errors to general.log for timeline context
                        let _ = general_file.write_all(line.as_bytes());
                    }
                }
            }
            LogOp::Shutdown => {
                let shutdown_line = format!(
                    "[{}] Log thread shutting down\n",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
                );
                let _ = general_file.write_all(shutdown_line.as_bytes());
                break;
            }
        }
    }
}

/// Open a log file in append mode, creating it if necessary.
fn open_log_file(dir: &std::path::Path, name: &str) -> File {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(name))
        .unwrap_or_else(|e| panic!("Failed to open log file {}: {}", name, e))
}
