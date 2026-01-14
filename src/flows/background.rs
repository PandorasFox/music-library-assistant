//! Background Task Tracking
//!
//! Simple infrastructure for tracking background tasks and their progress.
//! Replaces the over-engineered "operations" layer with minimal types.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// Progress state for a background task.
#[derive(Debug, Clone)]
pub struct TaskProgress {
    pub completed: usize,
    pub total: usize,
    pub skipped: usize,
    pub errors: usize,
    /// Optional byte-level progress (processed, total)
    pub bytes: Option<(u64, u64)>,
    /// Current item being processed (for display)
    pub current_item: Option<String>,
    /// When the task started
    pub start_time: Instant,
}

impl TaskProgress {
    pub fn new(total: usize) -> Self {
        Self {
            completed: 0,
            total,
            skipped: 0,
            errors: 0,
            bytes: None,
            current_item: None,
            start_time: Instant::now(),
        }
    }

    pub fn with_bytes(total: usize, total_bytes: u64) -> Self {
        Self {
            bytes: Some((0, total_bytes)),
            ..Self::new(total)
        }
    }

    pub fn percentage(&self) -> u8 {
        if self.total == 0 {
            return 0;
        }
        let done = self.completed + self.skipped;
        ((done as f64 / self.total as f64) * 100.0).clamp(0.0, 100.0) as u8
    }

    pub fn items_per_second(&self) -> f64 {
        let elapsed = self.start_time.elapsed().as_secs_f64();
        if elapsed == 0.0 {
            return 0.0;
        }
        self.completed as f64 / elapsed
    }
}

/// Messages sent from background tasks to the UI.
#[derive(Debug, Clone)]
pub enum TaskMessage {
    Progress(TaskProgress),
    Complete(TaskResult),
    Error(String),
    Cancelled,
}

/// Result of a completed background task.
#[derive(Debug, Clone)]
pub struct TaskResult {
    pub succeeded: usize,
    pub skipped: usize,
    pub failed: usize,
    pub duration: Duration,
    pub bytes_processed: Option<u64>,
    pub errors: Vec<String>,
}

impl TaskResult {
    pub fn success(succeeded: usize, skipped: usize, duration: Duration) -> Self {
        Self {
            succeeded,
            skipped,
            failed: 0,
            duration,
            bytes_processed: None,
            errors: Vec::new(),
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
}

/// A tracked background task.
pub struct BackgroundTask {
    pub id: String,
    pub label: String,
    pub receiver: mpsc::Receiver<TaskMessage>,
    pub cancel_flag: Arc<AtomicBool>,
    pub progress: TaskProgress,
}

impl BackgroundTask {
    /// Create a new background task, returning (task, sender, cancel_flag).
    pub fn new(label: impl Into<String>) -> (Self, mpsc::Sender<TaskMessage>, Arc<AtomicBool>) {
        let (tx, rx) = mpsc::channel();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let id = uuid::Uuid::new_v4().to_string()[..8].to_string();

        let task = Self {
            id,
            label: label.into(),
            receiver: rx,
            cancel_flag: cancel_flag.clone(),
            progress: TaskProgress::new(0),
        };

        (task, tx, cancel_flag)
    }

    /// Request cancellation.
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    /// Check if cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for BackgroundTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackgroundTask")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("progress", &self.progress)
            .finish()
    }
}

/// Poll all background tasks, returning completed ones.
///
/// Updates progress on active tasks, removes and returns completed/errored/cancelled tasks.
pub fn poll_tasks(tasks: &mut Vec<BackgroundTask>) -> Vec<(String, String, TaskResult)> {
    let mut completed = Vec::new();
    let mut to_remove = Vec::new();

    for (idx, task) in tasks.iter_mut().enumerate() {
        while let Ok(msg) = task.receiver.try_recv() {
            match msg {
                TaskMessage::Progress(progress) => {
                    task.progress = progress;
                }
                TaskMessage::Complete(result) => {
                    completed.push((task.id.clone(), task.label.clone(), result));
                    to_remove.push(idx);
                    break;
                }
                TaskMessage::Error(err) => {
                    let result = TaskResult {
                        succeeded: 0,
                        skipped: 0,
                        failed: 1,
                        duration: task.progress.start_time.elapsed(),
                        bytes_processed: None,
                        errors: vec![err],
                    };
                    completed.push((task.id.clone(), task.label.clone(), result));
                    to_remove.push(idx);
                    break;
                }
                TaskMessage::Cancelled => {
                    let result = TaskResult {
                        succeeded: task.progress.completed,
                        skipped: task.progress.skipped,
                        failed: 0,
                        duration: task.progress.start_time.elapsed(),
                        bytes_processed: task.progress.bytes.map(|(p, _)| p),
                        errors: vec!["Cancelled by user".to_string()],
                    };
                    completed.push((task.id.clone(), task.label.clone(), result));
                    to_remove.push(idx);
                    break;
                }
            }
        }
    }

    // Remove completed tasks in reverse order to maintain indices
    for idx in to_remove.into_iter().rev() {
        tasks.remove(idx);
    }

    completed
}

/// Log a task completion summary to the log file.
pub fn log_task_summary(label: &str, result: &TaskResult) {
    use std::fs::OpenOptions;
    use std::io::Write;

    let log_path = match crate::config::get_operations_log_path() {
        Ok(p) => p,
        Err(_) => return,
    };

    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let summary = format!(
        "[{}] {} - succeeded: {}, skipped: {}, failed: {}, duration: {:.1}s\n",
        timestamp,
        label,
        result.succeeded,
        result.skipped,
        result.failed,
        result.duration.as_secs_f64()
    );

    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let _ = file.write_all(summary.as_bytes());
        for error in &result.errors {
            let error_line = format!("  ERROR: {}\n", error);
            let _ = file.write_all(error_line.as_bytes());
        }
    }
}

/// Helper for sending throttled progress updates.
pub struct ProgressSender {
    tx: mpsc::Sender<TaskMessage>,
    cancel_flag: Arc<AtomicBool>,
    last_update: Instant,
    update_interval: Duration,
}

impl ProgressSender {
    pub fn new(tx: mpsc::Sender<TaskMessage>, cancel_flag: Arc<AtomicBool>) -> Self {
        Self {
            tx,
            cancel_flag,
            last_update: Instant::now() - Duration::from_secs(1), // Allow immediate first update
            update_interval: Duration::from_millis(100),
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::Relaxed)
    }

    /// Send a throttled progress update. Returns true if sent.
    pub fn update(&mut self, progress: TaskProgress) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_update) >= self.update_interval {
            let _ = self.tx.send(TaskMessage::Progress(progress));
            self.last_update = now;
            true
        } else {
            false
        }
    }

    /// Force send a progress update (bypasses throttling).
    pub fn force_update(&mut self, progress: TaskProgress) {
        let _ = self.tx.send(TaskMessage::Progress(progress));
        self.last_update = Instant::now();
    }

    pub fn complete(&self, result: TaskResult) {
        let _ = self.tx.send(TaskMessage::Complete(result));
    }

    pub fn error(&self, message: String) {
        let _ = self.tx.send(TaskMessage::Error(message));
    }

    pub fn cancelled(&self) {
        let _ = self.tx.send(TaskMessage::Cancelled);
    }
}
