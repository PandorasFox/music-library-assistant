//! Standardized Operation System
//!
//! Provides unified progress reporting, cancellation, and queuing for all
//! long-running background operations. All operations use this common infrastructure
//! to ensure consistent UI progress reporting and operation summary logging.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::config;
use crate::progress::{MtimeMismatchStats, ScanMessage, ScanProgress, ScanResult};

// ============================================================================
// Progress Types
// ============================================================================

/// Universal progress tracking for any operation.
///
/// Designed to be generic enough for scanning, deploying, report generation,
/// change execution, etc.
#[derive(Debug, Clone)]
pub struct OperationProgress {
    /// Total items to process (files, changes, tracks, etc.)
    pub total_items: usize,
    /// Items successfully processed so far
    pub completed_items: usize,
    /// Items skipped (unchanged, already processed, etc.)
    pub skipped_items: usize,
    /// Items that failed processing
    pub error_count: usize,

    /// Optional: total bytes to process (for I/O-bound operations)
    pub total_bytes: Option<u64>,
    /// Optional: bytes processed so far
    pub bytes_processed: Option<u64>,

    /// Description of current item being processed
    pub current_item: Option<String>,

    /// When the operation started (for ETA calculation)
    pub start_time: Instant,

    /// Optional: additional context (e.g., mtime stats for scanning)
    pub context: Option<ProgressContext>,
}

impl OperationProgress {
    /// Create a new progress tracker with known item count.
    pub fn new(total_items: usize) -> Self {
        Self {
            total_items,
            completed_items: 0,
            skipped_items: 0,
            error_count: 0,
            total_bytes: None,
            bytes_processed: None,
            current_item: None,
            start_time: Instant::now(),
            context: None,
        }
    }

    /// Create with byte tracking (for I/O operations like scanning).
    pub fn with_bytes(total_items: usize, total_bytes: u64) -> Self {
        Self {
            total_bytes: Some(total_bytes),
            bytes_processed: Some(0),
            ..Self::new(total_items)
        }
    }

    /// Percentage complete (0-100) based on items.
    pub fn percentage(&self) -> u8 {
        if self.total_items == 0 {
            return 0;
        }
        let done = self.completed_items + self.skipped_items;
        ((done as f64 / self.total_items as f64) * 100.0).clamp(0.0, 100.0) as u8
    }

    /// Percentage complete based on bytes (if available).
    pub fn byte_percentage(&self) -> Option<u8> {
        let (processed, total) = (self.bytes_processed?, self.total_bytes?);
        if total == 0 {
            return Some(0);
        }
        Some(((processed as f64 / total as f64) * 100.0).clamp(0.0, 100.0) as u8)
    }

    /// Throughput in items per second.
    pub fn items_per_second(&self) -> f64 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed == 0.0 {
            return 0.0;
        }
        self.completed_items as f64 / elapsed
    }

    /// Throughput in MiB/s (if bytes are tracked).
    pub fn throughput_mib(&self) -> Option<f64> {
        let bytes = self.bytes_processed?;
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed == 0.0 {
            return Some(0.0);
        }
        Some((bytes as f64 / (1024.0 * 1024.0)) / elapsed)
    }

    /// Throughput in MB/s (decimal, for display consistency with existing code).
    pub fn throughput_mbps(&self) -> Option<f64> {
        let bytes = self.bytes_processed?;
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed == 0.0 {
            return Some(0.0);
        }
        Some((bytes as f64 / 1_000_000.0) / elapsed)
    }

    /// Estimated time remaining in seconds.
    pub fn eta_seconds(&self) -> Option<u64> {
        let remaining_items = self
            .total_items
            .saturating_sub(self.completed_items + self.skipped_items);
        if remaining_items == 0 {
            return Some(0);
        }

        let items_per_sec = self.items_per_second();
        if items_per_sec < 0.01 {
            return None;
        }

        Some((remaining_items as f64 / items_per_sec) as u64)
    }

    /// ETA based on bytes (more accurate for variable-size items).
    pub fn eta_seconds_from_bytes(&self) -> Option<u64> {
        let (processed, total) = (self.bytes_processed?, self.total_bytes?);
        let remaining = total.saturating_sub(processed);
        if remaining == 0 {
            return Some(0);
        }

        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed == 0.0 || processed == 0 {
            return None;
        }

        let bytes_per_sec = processed as f64 / elapsed;
        if bytes_per_sec < 1.0 {
            return None;
        }

        Some((remaining as f64 / bytes_per_sec) as u64)
    }
}

/// Additional context attached to progress (operation-specific).
#[derive(Debug, Clone)]
pub enum ProgressContext {
    /// Scanner-specific: mtime mismatch statistics
    Scan { mtime_stats: MtimeMismatchStats },
    /// Change execution: breakdown by change type
    Changes {
        moves: usize,
        deletes: usize,
        deploys: usize,
        tag_edits: usize,
    },
    /// Report generation: which report type
    Report { report_name: String },
    /// Deployment: library being deployed to
    Deploy { library_name: String },
}

// ============================================================================
// Message Types
// ============================================================================

/// Messages sent from background operations to the UI.
#[derive(Debug, Clone)]
pub enum OperationMessage {
    /// Progress update - sent periodically during operation.
    Progress(OperationProgress),

    /// Operation completed successfully.
    Complete(OperationResult),

    /// Operation failed with an error.
    Error(String),

    /// Operation was cancelled by user.
    Cancelled,
}

/// Result of a completed operation.
#[derive(Debug, Clone)]
pub struct OperationResult {
    /// Number of items successfully processed
    pub succeeded: usize,
    /// Number of items skipped
    pub skipped: usize,
    /// Number of items that failed
    pub failed: usize,
    /// Total duration of the operation
    pub duration: Duration,
    /// Total bytes processed (if applicable)
    pub bytes_processed: Option<u64>,
    /// List of error messages (if any failures)
    pub errors: Vec<String>,
    /// Operation-specific result data
    pub data: Option<ResultData>,
}

impl OperationResult {
    pub fn success(succeeded: usize, skipped: usize, duration: Duration) -> Self {
        Self {
            succeeded,
            skipped,
            failed: 0,
            duration,
            bytes_processed: None,
            errors: Vec::new(),
            data: None,
        }
    }

    pub fn with_errors(mut self, failed: usize, errors: Vec<String>) -> Self {
        self.failed = failed;
        self.errors = errors;
        self
    }

    pub fn with_bytes(mut self, bytes: u64) -> Self {
        self.bytes_processed = Some(bytes);
        self
    }

    pub fn with_data(mut self, data: ResultData) -> Self {
        self.data = Some(data);
        self
    }
}

/// Operation-specific result data.
#[derive(Debug, Clone)]
pub enum ResultData {
    /// Scan result details
    Scan {
        files_scanned: usize,
        files_skipped: usize,
        bytes_scanned: u64,
    },
    /// Change execution details
    Changes { session_id: Option<String> },
    /// Report generation
    Report { output_path: String },
    /// Deployment
    Deploy {
        library_name: String,
        lost_files_moved: usize,
    },
    /// Tag flush operation
    TagFlush {
        flushed_paths: Vec<String>,
    },
}

// ============================================================================
// Operation Handle
// ============================================================================

/// Handle for controlling a running operation.
///
/// Returned when spawning an operation - allows cancellation and progress polling.
#[derive(Debug)]
pub struct OperationHandle {
    /// Receiver for progress messages
    pub receiver: mpsc::Receiver<OperationMessage>,
    /// Flag to signal cancellation
    pub cancel_flag: Arc<AtomicBool>,
    /// Type of operation for display purposes
    pub operation_type: OperationType,
}

impl OperationHandle {
    /// Create a new operation handle with channel.
    pub fn new(
        operation_type: OperationType,
    ) -> (Self, mpsc::Sender<OperationMessage>, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let handle = Self {
            receiver: rx,
            cancel_flag: cancel_flag.clone(),
            operation_type,
        };

        (handle, tx, cancel_flag)
    }

    /// Request cancellation of the operation.
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    /// Check if cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }

    /// Non-blocking poll for the next message.
    pub fn try_recv(&self) -> Option<OperationMessage> {
        self.receiver.try_recv().ok()
    }
}

/// Type of operation (for display and routing).
#[derive(Debug, Clone)]
pub enum OperationType {
    Scanning { source_name: String },
    GeneratingReport { report_type: String },
    Deploying { library_name: String, dry_run: bool },
    ExecutingChanges { session_id: String, change_count: usize },
    RebuildingHealth,
    Heartbeat,
}

impl OperationType {
    /// Human-readable description for UI.
    pub fn description(&self) -> String {
        match self {
            Self::Scanning { source_name } => format!("Scanning: {}", source_name),
            Self::GeneratingReport { report_type } => format!("Generating {} report", report_type),
            Self::Deploying {
                library_name,
                dry_run,
            } => {
                if *dry_run {
                    format!("Preview deployment to {}", library_name)
                } else {
                    format!("Deploying to {}", library_name)
                }
            }
            Self::ExecutingChanges { change_count, .. } => {
                format!("Executing {} changes", change_count)
            }
            Self::RebuildingHealth => "Rebuilding health index".to_string(),
            Self::Heartbeat => "Validating corpus".to_string(),
        }
    }

    /// Present participle for status line (e.g., "Scanning corpus")
    pub fn active_description(&self) -> String {
        match self {
            Self::Scanning { source_name } => format!("Scanning {}", source_name),
            Self::GeneratingReport { report_type } => format!("Generating {} report", report_type),
            Self::Deploying { library_name, .. } => format!("Deploying to {}", library_name),
            Self::ExecutingChanges { change_count, .. } => {
                format!("Executing {} changes", change_count)
            }
            Self::RebuildingHealth => "Rebuilding health index".to_string(),
            Self::Heartbeat => "Validating corpus".to_string(),
        }
    }
}

// ============================================================================
// Progress Reporter Helper
// ============================================================================

/// Helper for sending progress updates from within an operation.
///
/// Handles update throttling to avoid flooding the channel.
pub struct ProgressReporter {
    tx: mpsc::Sender<OperationMessage>,
    cancel_flag: Arc<AtomicBool>,
    last_update: Instant,
    update_interval: Duration,
}

impl ProgressReporter {
    /// Create a reporter with default update interval (100ms).
    pub fn new(tx: mpsc::Sender<OperationMessage>, cancel_flag: Arc<AtomicBool>) -> Self {
        Self {
            tx,
            cancel_flag,
            last_update: Instant::now() - Duration::from_secs(1), // Allow immediate first update
            update_interval: Duration::from_millis(100),
        }
    }

    /// Create a reporter with custom update interval.
    pub fn with_interval(
        tx: mpsc::Sender<OperationMessage>,
        cancel_flag: Arc<AtomicBool>,
        interval: Duration,
    ) -> Self {
        Self {
            update_interval: interval,
            ..Self::new(tx, cancel_flag)
        }
    }

    /// Check if the operation should be cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::Relaxed)
    }

    /// Send a progress update (throttled).
    ///
    /// Returns true if the update was sent, false if throttled.
    pub fn update(&mut self, progress: OperationProgress) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_update) >= self.update_interval {
            let _ = self.tx.send(OperationMessage::Progress(progress));
            self.last_update = now;
            true
        } else {
            false
        }
    }

    /// Force send a progress update (bypasses throttling).
    pub fn force_update(&mut self, progress: OperationProgress) {
        let _ = self.tx.send(OperationMessage::Progress(progress));
        self.last_update = Instant::now();
    }

    /// Send completion message.
    pub fn complete(&self, result: OperationResult) {
        let _ = self.tx.send(OperationMessage::Complete(result));
    }

    /// Send error message.
    pub fn error(&self, message: String) {
        let _ = self.tx.send(OperationMessage::Error(message));
    }

    /// Send cancelled message.
    pub fn cancelled(&self) {
        let _ = self.tx.send(OperationMessage::Cancelled);
    }
}

// ============================================================================
// Spawn Helpers
// ============================================================================

/// Spawn a background operation with proper setup.
///
/// Returns a handle for controlling and monitoring the operation.
pub fn spawn_operation<F>(operation_type: OperationType, work: F) -> OperationHandle
where
    F: FnOnce(ProgressReporter) + Send + 'static,
{
    let (handle, tx, cancel_flag) = OperationHandle::new(operation_type);
    let reporter = ProgressReporter::new(tx, cancel_flag);

    std::thread::spawn(move || {
        work(reporter);
    });

    handle
}

// ============================================================================
// Logging
// ============================================================================

/// Log an operation summary to the operations log file.
///
/// Appends a summary line to `~/.local/share/mla/operations-overview.log`.
pub fn log_operation_summary(result: &OperationResult, operation_type: &OperationType) {
    let log_path = match config::get_operations_log_path() {
        Ok(p) => p,
        Err(_) => return, // Silently fail if we can't get the path
    };

    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");

    let summary = format!(
        "[{}] {} - succeeded: {}, skipped: {}, failed: {}, duration: {:.1}s\n",
        timestamp,
        operation_type.description(),
        result.succeeded,
        result.skipped,
        result.failed,
        result.duration.as_secs_f64()
    );

    // Ensure parent directory exists
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Append to log file
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path);

    if let Ok(mut file) = file {
        let _ = file.write_all(summary.as_bytes());

        // If errors, log details
        for error in &result.errors {
            let error_line = format!("  ERROR: {}\n", error);
            let _ = file.write_all(error_line.as_bytes());
        }
    }
}

// ============================================================================
// Conversions from ScanProgress/ScanMessage
// ============================================================================

impl From<&ScanProgress> for OperationProgress {
    fn from(scan: &ScanProgress) -> Self {
        Self {
            total_items: scan.total_files,
            completed_items: scan.files_processed,
            skipped_items: scan.files_skipped,
            error_count: scan.errors,
            total_bytes: Some(scan.total_bytes),
            bytes_processed: Some(scan.bytes_processed),
            current_item: scan.current_file.clone(),
            start_time: scan.start_time,
            context: scan.mtime_stats.as_ref().map(|stats| ProgressContext::Scan {
                mtime_stats: stats.clone(),
            }),
        }
    }
}

impl From<ScanProgress> for OperationProgress {
    fn from(scan: ScanProgress) -> Self {
        (&scan).into()
    }
}

impl From<ScanMessage> for OperationMessage {
    fn from(msg: ScanMessage) -> Self {
        match msg {
            ScanMessage::Progress(p) => OperationMessage::Progress((&p).into()),
            ScanMessage::Complete(r) => OperationMessage::Complete(OperationResult {
                succeeded: r.files_scanned,
                skipped: r.files_skipped,
                failed: r.errors,
                duration: r.duration,
                bytes_processed: Some(r.bytes_scanned),
                errors: Vec::new(),
                data: Some(ResultData::Scan {
                    files_scanned: r.files_scanned,
                    files_skipped: r.files_skipped,
                    bytes_scanned: r.bytes_scanned,
                }),
            }),
            ScanMessage::Error(e) => OperationMessage::Error(e),
        }
    }
}

impl From<&ScanResult> for OperationResult {
    fn from(r: &ScanResult) -> Self {
        OperationResult {
            succeeded: r.files_scanned,
            skipped: r.files_skipped,
            failed: r.errors,
            duration: r.duration,
            bytes_processed: Some(r.bytes_scanned),
            errors: Vec::new(),
            data: Some(ResultData::Scan {
                files_scanned: r.files_scanned,
                files_skipped: r.files_skipped,
                bytes_scanned: r.bytes_scanned,
            }),
        }
    }
}

// ============================================================================
// Multi-Operation Tracking
// ============================================================================

/// Unique identifier for a tracked operation
pub type OperationId = String;

/// Generate a new unique operation ID
pub fn new_operation_id() -> OperationId {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

/// A tracked operation with its receiver and state
pub struct TrackedOperation {
    pub id: OperationId,
    pub operation_type: OperationType,
    pub progress: OperationProgress,
    pub cancel_flag: Arc<AtomicBool>,
    pub receiver: mpsc::Receiver<OperationMessage>,
}

impl TrackedOperation {
    /// Create a new tracked operation, returning (operation, sender)
    pub fn new(operation_type: OperationType) -> (Self, mpsc::Sender<OperationMessage>) {
        let (tx, rx) = mpsc::channel();
        let op = Self {
            id: new_operation_id(),
            operation_type,
            progress: OperationProgress::new(0),
            cancel_flag: Arc::new(AtomicBool::new(false)),
            receiver: rx,
        };
        (op, tx)
    }

    /// Request cancellation
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    /// Check if cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for TrackedOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackedOperation")
            .field("id", &self.id)
            .field("operation_type", &self.operation_type)
            .field("progress", &self.progress)
            .finish()
    }
}

/// Manager for multiple concurrent operations
#[derive(Default)]
pub struct OperationManager {
    operations: Vec<TrackedOperation>,
    /// Legacy scan progress tracking (for backwards compatibility with existing scan code)
    legacy_progress: Option<(OperationType, OperationProgress)>,
}

impl OperationManager {
    pub fn new() -> Self {
        Self {
            operations: Vec::new(),
            legacy_progress: None,
        }
    }

    /// Add a new operation, returning its ID and sender
    pub fn add(&mut self, operation_type: OperationType) -> (OperationId, mpsc::Sender<OperationMessage>, Arc<AtomicBool>) {
        let (op, tx) = TrackedOperation::new(operation_type);
        let id = op.id.clone();
        let cancel_flag = op.cancel_flag.clone();
        self.operations.push(op);
        (id, tx, cancel_flag)
    }

    /// Get number of active operations (including legacy)
    pub fn count(&self) -> usize {
        self.operations.len() + if self.legacy_progress.is_some() { 1 } else { 0 }
    }

    /// Check if any operations are running
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty() && self.legacy_progress.is_none()
    }

    /// Get operation by ID
    pub fn get(&self, id: &str) -> Option<&TrackedOperation> {
        self.operations.iter().find(|op| op.id == id)
    }

    /// Get mutable operation by ID
    pub fn get_mut(&mut self, id: &str) -> Option<&mut TrackedOperation> {
        self.operations.iter_mut().find(|op| op.id == id)
    }

    /// Iterate over all operations
    pub fn iter(&self) -> impl Iterator<Item = &TrackedOperation> {
        self.operations.iter()
    }

    /// Update legacy scan progress (used by legacy scan receiver)
    pub fn update_legacy_progress(&mut self, progress: OperationProgress) {
        if let Some((_, ref mut p)) = self.legacy_progress {
            *p = progress;
        }
    }

    /// Set the legacy operation type for display
    pub fn set_legacy_operation(&mut self, op_type: OperationType) {
        self.legacy_progress = Some((op_type, OperationProgress::new(0)));
    }

    /// Clear legacy progress tracking
    pub fn clear_legacy_progress(&mut self) {
        self.legacy_progress = None;
    }

    /// Get legacy progress if any
    pub fn legacy_progress(&self) -> Option<&(OperationType, OperationProgress)> {
        self.legacy_progress.as_ref()
    }

    /// Get all active operations for display (including legacy if present)
    pub fn all_for_display(&self) -> Vec<(&OperationType, &OperationProgress)> {
        let mut result: Vec<_> = self.operations
            .iter()
            .map(|op| (&op.operation_type, &op.progress))
            .collect();

        if let Some((ref op_type, ref progress)) = self.legacy_progress {
            result.push((op_type, progress));
        }

        result
    }

    /// Poll all operations for progress, returning completed (id, result, op_type) tuples
    pub fn poll_all(&mut self) -> Vec<(OperationId, OperationResult, OperationType)> {
        let mut completed = Vec::new();
        let mut to_remove = Vec::new();

        for (idx, op) in self.operations.iter_mut().enumerate() {
            while let Ok(msg) = op.receiver.try_recv() {
                match msg {
                    OperationMessage::Progress(progress) => {
                        op.progress = progress;
                    }
                    OperationMessage::Complete(result) => {
                        completed.push((op.id.clone(), result, op.operation_type.clone()));
                        to_remove.push(idx);
                        break;
                    }
                    OperationMessage::Error(err) => {
                        let result = OperationResult {
                            succeeded: 0,
                            skipped: 0,
                            failed: 1,
                            duration: op.progress.start_time.elapsed(),
                            bytes_processed: None,
                            errors: vec![err],
                            data: None,
                        };
                        completed.push((op.id.clone(), result, op.operation_type.clone()));
                        to_remove.push(idx);
                        break;
                    }
                    OperationMessage::Cancelled => {
                        let result = OperationResult {
                            succeeded: op.progress.completed_items,
                            skipped: op.progress.skipped_items,
                            failed: 0,
                            duration: op.progress.start_time.elapsed(),
                            bytes_processed: op.progress.bytes_processed,
                            errors: vec!["Cancelled by user".to_string()],
                            data: None,
                        };
                        completed.push((op.id.clone(), result, op.operation_type.clone()));
                        to_remove.push(idx);
                        break;
                    }
                }
            }
        }

        // Remove completed operations (in reverse order to maintain indices)
        for idx in to_remove.into_iter().rev() {
            self.operations.remove(idx);
        }

        completed
    }

    /// Cancel all operations
    pub fn cancel_all(&self) {
        for op in &self.operations {
            op.cancel();
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_progress_percentage() {
        let mut progress = OperationProgress::new(100);
        assert_eq!(progress.percentage(), 0);

        progress.completed_items = 50;
        assert_eq!(progress.percentage(), 50);

        progress.skipped_items = 25;
        assert_eq!(progress.percentage(), 75);

        progress.completed_items = 75;
        progress.skipped_items = 25;
        assert_eq!(progress.percentage(), 100);
    }

    #[test]
    fn test_operation_progress_with_bytes() {
        let progress = OperationProgress::with_bytes(100, 1000);
        assert_eq!(progress.total_bytes, Some(1000));
        assert_eq!(progress.bytes_processed, Some(0));
    }

    #[test]
    fn test_operation_result_builder() {
        let result = OperationResult::success(10, 5, Duration::from_secs(1))
            .with_errors(2, vec!["error1".to_string()])
            .with_bytes(1000);

        assert_eq!(result.succeeded, 10);
        assert_eq!(result.skipped, 5);
        assert_eq!(result.failed, 2);
        assert_eq!(result.bytes_processed, Some(1000));
        assert_eq!(result.errors.len(), 1);
    }

    #[test]
    fn test_operation_type_description() {
        let op = OperationType::Scanning {
            source_name: "corpus".to_string(),
        };
        assert_eq!(op.description(), "Scanning: corpus");

        let op = OperationType::ExecutingChanges {
            session_id: "abc".to_string(),
            change_count: 42,
        };
        assert_eq!(op.description(), "Executing 42 changes");
    }

    #[test]
    fn test_scan_progress_conversion() {
        let scan = ScanProgress {
            total_bytes: 1000,
            bytes_processed: 500,
            files_processed: 10,
            total_files: 20,
            files_skipped: 5,
            current_file: Some("test.flac".to_string()),
            errors: 1,
            start_time: Instant::now(),
            mtime_stats: None,
        };

        let op: OperationProgress = (&scan).into();
        assert_eq!(op.total_items, 20);
        assert_eq!(op.completed_items, 10);
        assert_eq!(op.skipped_items, 5);
        assert_eq!(op.error_count, 1);
        assert_eq!(op.total_bytes, Some(1000));
        assert_eq!(op.bytes_processed, Some(500));
    }
}
