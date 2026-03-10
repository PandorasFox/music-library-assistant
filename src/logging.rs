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

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, OnceLock};
use std::thread::JoinHandle;

use mm_utils::paths::get_logs_dir;

// ============================================================================
// Types
// ============================================================================

/// Log category determines which file a message routes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogCategory {
    /// Mutation execution START/END, transaction lifecycle.
    /// Routes to: mutations.log
    Mutation,

    /// All errors, warnings, failures.
    /// Routes to: errors.log (also mirrored to general.log)
    Error,

    /// Everything else: state transitions, compute progress, UI actions, startup.
    /// Routes to: general.log
    General,
}

/// A structured log entry sent through the channel.
pub(crate) struct LogEntry {
    category: LogCategory,
    message: String,
    timestamp: chrono::DateTime<chrono::Local>,
}

/// Channel message type with shutdown sentinel.
pub(crate) enum LogOp {
    Entry(LogEntry),
    Shutdown,
}

// ============================================================================
// Global Channel
// ============================================================================

static LOG_SENDER: OnceLock<Sender<LogOp>> = OnceLock::new();

/// Initialize the global log channel. Called once from main(), before anything else.
///
/// Returns the receiver end, which will be given to the Witch to spawn the logging thread.
/// Messages sent before the logging thread starts queue in the unbounded channel buffer.
pub fn init_log_channel() -> Receiver<LogOp> {
    let (tx, rx) = mpsc::channel();
    let _ = LOG_SENDER.set(tx);
    rx
}

// ============================================================================
// Public Logging API
// ============================================================================

/// Send a log entry with the given category.
fn log(category: LogCategory, message: impl Into<String>) {
    if let Some(sender) = LOG_SENDER.get() {
        let _ = sender.send(LogOp::Entry(LogEntry {
            category,
            message: message.into(),
            timestamp: chrono::Local::now(),
        }));
    }
    // If channel isn't initialized (shouldn't happen — init is first step in main),
    // the message is silently dropped.
}

/// Log to general.log — startup, state transitions, compute, UI, db_thread.
pub fn log_general(message: impl Into<String>) {
    log(LogCategory::General, message);
}

/// Log to mutations.log — execution START/END, transaction lifecycle.
pub fn log_mutation(message: impl Into<String>) {
    log(LogCategory::Mutation, message);
}

/// Log to errors.log (and mirrored to general.log) — all errors and warnings.
pub fn log_error(message: impl Into<String>) {
    log(LogCategory::Error, message);
}

// ============================================================================
// Shutdown
// ============================================================================

/// Signal the logging thread to flush and exit.
///
/// Called by `Witch::drop()`. After this, further log() calls will still send
/// but the thread will have exited so messages queue until process exit.
pub fn request_shutdown() {
    if let Some(sender) = LOG_SENDER.get() {
        let _ = sender.send(LogOp::Shutdown);
    }
}

// ============================================================================
// Logging Thread
// ============================================================================

/// Handle returned by `spawn_log_thread()` for shutdown coordination.
pub(crate) struct LogThreadHandle {
    thread_handle: Option<JoinHandle<()>>,
}

impl LogThreadHandle {
    /// Join the logging thread, blocking until it finishes flushing.
    pub fn join(&mut self) {
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
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
