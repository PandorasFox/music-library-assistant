//! Metadata Analysis Progress Screen
//!
//! Full-screen progress modal shown during metadata analysis phase.
//! Unlike the startup splash screen, this runs with the Eye awake (blinking).
//!
//! ## Lifecycle
//!
//! ```text
//! Intake complete (or skipped)
//!     │
//!     ├─► daemon.queue_content_analysis() - queues work
//!     │
//!     ├─► ContentAnalysisProgress::new() - screen created
//!     │
//!     ├─► ContentAnalysisProgress::tick() each frame
//!     │       │
//!     │       └─► Polls daemon, updates progress
//!     │           Checks daemon.state() for completion
//!     │
//!     └─► ContentAnalysisProgress::is_complete() becomes true
//!             │
//!             └─► Caller transitions to Insights view
//! ```

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::daemon::{DaemonStateSnapshot, DaemonStatus, TaskDaemon, WorkerStats};
use crate::db_thread::DbThreadStats;

// ============================================================================
// Content Analysis Progress State
// ============================================================================

/// Metadata analysis progress screen controller.
///
/// Manages the progress display during metadata analysis phase.
/// Eye is awake and can blink during this phase.
#[derive(Debug)]
pub struct ContentAnalysisProgress {
    /// Optional progress (0.0 to 1.0)
    progress: Option<f32>,
    /// Optional progress detail (e.g., "1234 / 5678 tasks")
    progress_detail: Option<String>,
    /// True once content analysis computations complete
    complete: bool,
    /// Whether we've seen daemon start working (to distinguish initial state from completion)
    seen_working: bool,
    /// DB thread stats for optional display
    db_stats: Option<DbThreadStats>,
    /// Worker thread stats for optional display
    worker_stats: Option<WorkerStats>,
    /// Pending DB writes (always tracked, no timing guard)
    db_queue_depth: u64,
}

impl ContentAnalysisProgress {
    /// Create a new metadata analysis progress screen.
    ///
    /// The screen will remain active until daemon completes all metadata analysis
    /// computations, at which point `is_complete()` returns true.
    pub fn new() -> Self {
        Self {
            progress: Some(0.0),
            progress_detail: Some("Starting metadata analysis...".to_string()),
            complete: false,
            seen_working: false,
            db_stats: None,
            worker_stats: None,
            db_queue_depth: 0,
        }
    }

    /// Check if metadata analysis is complete and ready to transition.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Get current progress (0.0 to 1.0).
    pub fn progress(&self) -> Option<f32> {
        self.progress
    }

    /// Get progress detail string.
    pub fn progress_detail(&self) -> Option<&str> {
        self.progress_detail.as_deref()
    }

    /// Get the status message.
    pub fn status_message(&self) -> &'static str {
        if self.complete {
            "Analysis complete!"
        } else {
            "Analyzing metadata..."
        }
    }

    /// Update DB thread stats for display.
    /// Only updates if stats are provided (timing instrumentation enabled).
    pub fn set_db_stats(&mut self, stats: Option<DbThreadStats>) {
        if stats.is_some() {
            self.db_stats = stats;
        }
    }

    /// Update worker thread stats for display.
    /// Only updates if stats are provided (timing instrumentation enabled).
    pub fn set_worker_stats(&mut self, stats: Option<WorkerStats>) {
        if stats.is_some() {
            self.worker_stats = stats;
        }
    }

    /// Update pending DB write queue depth (always available).
    pub fn set_db_queue_depth(&mut self, depth: u64) {
        self.db_queue_depth = depth;
    }

    /// Tick the progress screen state.
    ///
    /// Returns `true` if content analysis is complete.
    /// Call this each frame while the screen is active.
    pub fn tick(&mut self, daemon: &TaskDaemon) -> bool {
        // Update progress from daemon status
        let status = daemon.status();
        self.update_progress(&status);

        // Track when daemon starts working
        if status.state == DaemonStateSnapshot::Working {
            self.seen_working = true;
        }

        // Check for completion:
        // - Daemon must have been working at some point (seen_working)
        // - Daemon state is now Idle or Completed
        // - No pending tasks
        if self.seen_working && status.pending == 0 {
            match status.state {
                DaemonStateSnapshot::Idle | DaemonStateSnapshot::Completed => {
                    self.complete = true;
                }
                DaemonStateSnapshot::Working => {
                    // Still working, not complete
                }
            }
        }

        self.complete
    }

    /// Update progress from daemon status.
    fn update_progress(&mut self, status: &DaemonStatus) {
        let completed = status.total_processed;
        let total = status.session_queued;
        if total > 0 {
            self.progress = Some(completed as f32 / total as f32);
            self.progress_detail = Some(format!("{} / {}", completed, total));
        }
    }
}

// ============================================================================
// Rendering
// ============================================================================

/// Render the metadata analysis progress screen.
///
/// Shows a centered eye (provided by caller, can be animated) with a progress message.
/// The eye_frame parameter should come from the app's Eye animation state.
pub fn render(f: &mut Frame, area: Rect, progress: &ContentAnalysisProgress, eye_frame: &str) {
    // Eye art is 16 lines tall
    let eye_height = 16;
    // Message line + spacing + eye + optional progress bar + optional stats
    let progress_height = if progress.progress.is_some() { 3 } else { 0 };
    // Always show queue depth when there's a backlog (no timing guard)
    let show_queue_depth = progress.db_queue_depth > 0;
    let queue_depth_height = if show_queue_depth { 1 } else { 0 };

    let worker_stats_height = if progress.worker_stats.is_some() { 2 } else { 0 };
    let db_stats_height = if progress.db_stats.is_some() { 1 } else { 0 };
    let total_height = 2 + eye_height + progress_height + queue_depth_height + worker_stats_height + db_stats_height;

    // Calculate vertical centering
    let v_margin = area.height.saturating_sub(total_height as u16) / 2;

    // Calculate horizontal centering for the eye (eye is ~60 chars wide)
    let eye_width = 60u16;
    let h_margin = area.width.saturating_sub(eye_width) / 2;

    // Layout vertically
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(v_margin),                    // Top margin
            Constraint::Length(1),                           // Message
            Constraint::Length(1),                           // Spacing
            Constraint::Length(eye_height as u16),           // Eye
            Constraint::Length(progress_height as u16),      // Progress bar (if any)
            Constraint::Length(queue_depth_height as u16),   // Queue depth (always, if backlog)
            Constraint::Length(worker_stats_height as u16),  // Worker stats (if timing enabled)
            Constraint::Length(db_stats_height as u16),      // DB stats (if timing enabled)
            Constraint::Min(0),                              // Bottom margin
        ])
        .split(area);

    // Render message centered
    let message = Paragraph::new(progress.status_message())
        .style(Style::default().fg(Color::Cyan))
        .alignment(Alignment::Center);
    f.render_widget(message, chunks[1]);

    // Render eye centered horizontally (uses provided frame - can be animated)
    let eye_area = Rect {
        x: area.x + h_margin,
        y: chunks[3].y,
        width: eye_width.min(area.width),
        height: chunks[3].height,
    };

    let eye_lines: Vec<Line> = eye_frame
        .lines()
        .map(|line| Line::from(line))
        .collect();

    let eye_widget = Paragraph::new(eye_lines)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(eye_widget, eye_area);

    // Render progress bar if present
    if let Some(prog) = progress.progress {
        let progress_area = chunks[4];

        // Create progress bar
        let bar_width = 40u16.min(progress_area.width.saturating_sub(4));
        let bar_x = (progress_area.width.saturating_sub(bar_width)) / 2 + progress_area.x;

        let filled = (prog * bar_width as f32) as u16;
        let empty = bar_width.saturating_sub(filled);

        let bar_text = format!(
            "[{}{}]",
            "=".repeat(filled as usize),
            " ".repeat(empty as usize)
        );

        let detail = progress.progress_detail.as_deref().unwrap_or("");
        let progress_text = format!("{}\n{}", bar_text, detail);

        let progress_widget = Paragraph::new(progress_text)
            .style(Style::default().fg(Color::Yellow))
            .alignment(Alignment::Center);

        let centered_progress = Rect {
            x: bar_x,
            y: progress_area.y,
            width: bar_width + 4,
            height: progress_area.height,
        };

        f.render_widget(progress_widget, centered_progress);
    }

    // Render pending DB writes (always, when there's a backlog)
    if show_queue_depth {
        let queue_area = chunks[5];

        // Magenta color (#f51c99)
        let label_color = Color::Rgb(245, 28, 153);

        let queue_line = Line::from(vec![
            Span::styled(
                format!("[{} pending DB writes queued]", progress.db_queue_depth),
                Style::default().fg(label_color),
            ),
        ]);

        let queue_widget = Paragraph::new(queue_line)
            .alignment(Alignment::Center);
        f.render_widget(queue_widget, queue_area);
    }

    // Render worker stats if available
    if let Some(ref stats) = progress.worker_stats {
        let stats_area = chunks[6];

        // Magenta color for labels (#f51c99)
        let label_color = Color::Rgb(245, 28, 153);

        // Color coding for avg task time
        let avg_task_color = if stats.avg_task_ms < 100 {
            Color::Green
        } else if stats.avg_task_ms < 500 {
            Color::Yellow
        } else {
            Color::Red
        };

        // Color coding for queue wait
        let queue_wait_color = if stats.queue_wait_avg_ms < 5 {
            Color::Green
        } else if stats.queue_wait_avg_ms < 20 {
            Color::Yellow
        } else {
            Color::Red
        };

        // Line 1: Tasks: N | Avg: Nms | Max: Nms (label) | Threads: N
        let line1 = Line::from(vec![
            Span::styled("Tasks: ", Style::default().fg(label_color)),
            Span::styled(format!("{}", stats.tasks_completed), Style::default().fg(Color::Cyan)),
            Span::styled(" | ", Style::default().fg(Color::DarkGray)),
            Span::styled("Avg: ", Style::default().fg(label_color)),
            Span::styled(format!("{}ms", stats.avg_task_ms), Style::default().fg(avg_task_color)),
            Span::styled(" | ", Style::default().fg(Color::DarkGray)),
            Span::styled("Max: ", Style::default().fg(label_color)),
            Span::styled(format!("{}ms", stats.max_task_ms), Style::default().fg(Color::White)),
            if !stats.max_task_label.is_empty() {
                Span::styled(format!(" ({})", stats.max_task_label), Style::default().fg(Color::DarkGray))
            } else {
                Span::raw("")
            },
            Span::styled(" | ", Style::default().fg(Color::DarkGray)),
            Span::styled("Threads: ", Style::default().fg(label_color)),
            Span::styled(format!("{}", stats.active_threads), Style::default().fg(Color::White)),
        ]);

        // Line 2: Q Wait: Navg/Nmax ms | Reads: avg/med/max µs (N)
        let line2 = Line::from(vec![
            Span::styled("Q Wait: ", Style::default().fg(label_color)),
            Span::styled(format!("{}avg", stats.queue_wait_avg_ms), Style::default().fg(queue_wait_color)),
            Span::styled("/", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}max ms", stats.queue_wait_max_ms), Style::default().fg(Color::White)),
            Span::styled(" | ", Style::default().fg(Color::DarkGray)),
            Span::styled("Reads: ", Style::default().fg(label_color)),
            Span::styled(format!("{}", stats.avg_db_read_us), Style::default().fg(Color::Green)),
            Span::styled("/", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", stats.median_db_read_us), Style::default().fg(Color::Cyan)),
            Span::styled("/", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}µs", stats.max_db_read_us), Style::default().fg(Color::White)),
            Span::styled(format!(" ({})", stats.total_db_reads), Style::default().fg(Color::DarkGray)),
        ]);

        let worker_stats_widget = Paragraph::new(vec![line1, line2])
            .alignment(Alignment::Center);
        f.render_widget(worker_stats_widget, stats_area);
    }

    // Render DB stats if available
    if let Some(ref stats) = progress.db_stats {
        let stats_area = chunks[7];

        // Format: "Writes: 456 | 12.3/s | Q: 3"
        let queue_color = if stats.queue_depth > 100 {
            Color::Red
        } else if stats.queue_depth > 0 {
            Color::Yellow
        } else {
            Color::Green
        };

        let stats_line = Line::from(vec![
            Span::styled("Writes: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", stats.total_writes), Style::default().fg(Color::Cyan)),
            Span::styled(" | ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{:.1}/s", stats.writes_per_sec), Style::default().fg(Color::Green)),
            Span::styled(" | Q: ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", stats.queue_depth), Style::default().fg(queue_color)),
        ]);

        let db_stats_widget = Paragraph::new(stats_line)
            .alignment(Alignment::Center);
        f.render_widget(db_stats_widget, stats_area);
    }
}
