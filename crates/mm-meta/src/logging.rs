//! Logging API for mm-meta consumers.
//!
//! Provides `log_general` and `log_error` via a global mpsc channel.
//! The channel is initialized by the mm binary in main() before anything else.
//! The logging thread (owned by the Witch) drains the channel.

use std::sync::mpsc::Sender;
use std::sync::OnceLock;

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
pub struct LogEntry {
    pub category: LogCategory,
    pub message: String,
    pub timestamp: chrono::DateTime<chrono::Local>,
}

/// Channel message type with shutdown sentinel.
pub enum LogOp {
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
pub fn init_log_channel() -> std::sync::mpsc::Receiver<LogOp> {
    let (tx, rx) = std::sync::mpsc::channel();
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
pub fn request_shutdown() {
    if let Some(sender) = LOG_SENDER.get() {
        let _ = sender.send(LogOp::Shutdown);
    }
}
