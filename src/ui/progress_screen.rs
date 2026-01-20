//! Unified Progress Screen
//!
//! Full-screen progress modal for all daemon work phases:
//! - Eyeballing (startup corpus scan, eye closed)
//! - ContentAnalysis (metadata analysis, eye awake)
//! - SignalRefresh (post-mutation signal updates, eye awake)
//!
//! ## Usage
//!
//! ```rust,ignore
//! // For startup eyeballing
//! let mut progress = ProgressScreen::new_eyeballing();
//! daemon.start_paranoid_eyeball(...);
//!
//! // Each frame
//! if progress.tick(daemon) {
//!     // Eyeballing complete, transition to next phase
//! }
//!
//! // For content analysis
//! let mut progress = ProgressScreen::new_content_analysis();
//! daemon.queue_content_analysis();
//! ```

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::daemon::{DaemonStatus, EyeState, TaskDaemon, WorkerStats};
use crate::db_thread::DbThreadStats;
use super::app::{EYE_CLOSED, EYE_CLOSING};
use super::wait_state::WaitState;

// ============================================================================
// Progress Phase
// ============================================================================

/// The phase of progress being displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressPhase {
    /// Initial eyeballing - eye closed, scanning corpus.
    Eyeballing,
    /// Post-intake content analysis - eye awake, analyzing metadata.
    ContentAnalysis,
    /// Post-mutation signal refresh - eye awake, updating signals.
    SignalRefresh,
}

impl ProgressPhase {
    /// Get the status message for this phase.
    pub fn status_message(&self, eye_state: EyeState, complete: bool) -> &'static str {
        if complete {
            return match self {
                ProgressPhase::Eyeballing => "Ready!",
                ProgressPhase::ContentAnalysis => "Analysis complete!",
                ProgressPhase::SignalRefresh => "Signals updated!",
            };
        }

        match self {
            ProgressPhase::Eyeballing => match eye_state {
                EyeState::Closed => "Scanning corpus...",
                EyeState::Awakening => "Computing health signals...",
                EyeState::Awake => "Ready!",
            },
            ProgressPhase::ContentAnalysis => "Analyzing metadata...",
            ProgressPhase::SignalRefresh => "Updating signals...",
        }
    }

    /// Whether this phase uses a closed/awakening eye (vs animated awake eye).
    pub fn uses_closed_eye(&self) -> bool {
        matches!(self, ProgressPhase::Eyeballing)
    }
}

// ============================================================================
// Progress Screen State
// ============================================================================

/// Unified progress screen controller.
///
/// Handles all daemon work phases with a consistent interface.
#[derive(Debug)]
pub struct ProgressScreen {
    /// Current progress phase.
    phase: ProgressPhase,
    /// Wait state for tracking completion.
    wait_state: WaitState,
    /// Optional progress (0.0 to 1.0).
    progress: Option<f32>,
    /// Optional progress detail (e.g., "1234 / 5678 tasks").
    progress_detail: Option<String>,
    /// True once work is complete.
    complete: bool,
    /// Current eye state (for Eyeballing phase rendering).
    eye_state: EyeState,
    /// DB thread stats for optional display.
    db_stats: Option<DbThreadStats>,
    /// Worker thread stats for optional display.
    worker_stats: Option<WorkerStats>,
    /// Pending DB writes (always tracked).
    db_queue_depth: u64,
}

impl ProgressScreen {
    /// Create a progress screen for startup eyeballing.
    ///
    /// Completes when daemon.eye_state() becomes Awake.
    pub fn new_eyeballing() -> Self {
        Self {
            phase: ProgressPhase::Eyeballing,
            wait_state: WaitState::new(),
            progress: Some(0.0),
            progress_detail: Some("Starting...".to_string()),
            complete: false,
            eye_state: EyeState::Closed,
            db_stats: None,
            worker_stats: None,
            db_queue_depth: 0,
        }
    }

    /// Create a progress screen for content analysis.
    ///
    /// Completes when daemon work drains after starting.
    pub fn new_content_analysis() -> Self {
        let mut screen = Self {
            phase: ProgressPhase::ContentAnalysis,
            wait_state: WaitState::new(),
            progress: Some(0.0),
            progress_detail: Some("Starting metadata analysis...".to_string()),
            complete: false,
            eye_state: EyeState::Awake,
            db_stats: None,
            worker_stats: None,
            db_queue_depth: 0,
        };
        screen.wait_state.start();
        screen
    }

    /// Create a progress screen for post-mutation signal refresh.
    ///
    /// Completes when daemon work drains after starting.
    pub fn new_signal_refresh() -> Self {
        let mut screen = Self {
            phase: ProgressPhase::SignalRefresh,
            wait_state: WaitState::new(),
            progress: Some(0.0),
            progress_detail: Some("Updating signals...".to_string()),
            complete: false,
            eye_state: EyeState::Awake,
            db_stats: None,
            worker_stats: None,
            db_queue_depth: 0,
        };
        screen.wait_state.start();
        screen
    }

    /// Get the current phase.
    pub fn phase(&self) -> ProgressPhase {
        self.phase
    }

    /// Whether this screen uses a closed/awakening eye (vs animated awake eye).
    pub fn uses_closed_eye(&self) -> bool {
        self.phase.uses_closed_eye()
    }

    /// Check if progress is complete.
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
        self.phase.status_message(self.eye_state, self.complete)
    }

    /// Update DB thread stats for display.
    pub fn set_db_stats(&mut self, stats: Option<DbThreadStats>) {
        if stats.is_some() {
            self.db_stats = stats;
        }
    }

    /// Update worker thread stats for display.
    pub fn set_worker_stats(&mut self, stats: Option<WorkerStats>) {
        if stats.is_some() {
            self.worker_stats = stats;
        }
    }

    /// Update pending DB write queue depth.
    pub fn set_db_queue_depth(&mut self, depth: u64) {
        self.db_queue_depth = depth;
    }

    /// Tick the progress screen state.
    ///
    /// Returns `true` if work is complete.
    pub fn tick(&mut self, daemon: &TaskDaemon) -> bool {
        // Update progress from daemon
        let status = daemon.status();
        self.update_progress(&status);

        // Update stats
        self.set_db_stats(daemon.db_stats());
        self.set_worker_stats(daemon.worker_stats());
        self.set_db_queue_depth(daemon.db_queue_depth());

        // Phase-specific completion detection
        match self.phase {
            ProgressPhase::Eyeballing => {
                // Track eye state for rendering
                self.eye_state = daemon.eye_state();

                // Complete when eye becomes Awake
                if self.eye_state == EyeState::Awake {
                    self.complete = true;
                }
            }
            ProgressPhase::ContentAnalysis | ProgressPhase::SignalRefresh => {
                // Use WaitState for completion detection
                if self.wait_state.tick(daemon) {
                    self.complete = true;
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

    /// Get the eye art for this phase.
    pub fn eye_art(&self) -> &'static str {
        match self.phase {
            ProgressPhase::Eyeballing => match self.eye_state {
                EyeState::Awakening => EYE_CLOSING, // Half-open frame
                _ => EYE_CLOSED,
            },
            ProgressPhase::ContentAnalysis | ProgressPhase::SignalRefresh => {
                // For awake phases, caller provides animated eye frame
                EYE_CLOSED // Fallback, but caller should use render_with_eye_frame
            }
        }
    }
}

// ============================================================================
// Rendering
// ============================================================================

/// Render the progress screen.
///
/// For Eyeballing phase, uses closed/awakening eye automatically.
/// For other phases, uses the provided eye_frame (for animation).
pub fn render(f: &mut Frame, area: Rect, screen: &ProgressScreen, eye_frame: Option<&str>) {
    // Eye art is 16 lines tall
    let eye_height = 16;
    let progress_height = if screen.progress.is_some() { 3 } else { 0 };

    // Always show queue depth when there's a backlog
    let show_queue_depth = screen.db_queue_depth > 0;
    let queue_depth_height = if show_queue_depth { 1 } else { 0 };

    // Detailed timing stats (behind timing guard)
    let show_stats = crate::config::is_timing_enabled();
    let show_db_stats = show_stats && screen.db_stats.is_some();
    let show_worker_stats = show_stats && screen.worker_stats.is_some();
    let db_stats_height = if show_db_stats { 1 } else { 0 };
    let worker_stats_height = if show_worker_stats { 2 } else { 0 };
    let total_height = 2 + eye_height + progress_height + queue_depth_height + db_stats_height + worker_stats_height;

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
            Constraint::Length(progress_height as u16),      // Progress bar
            Constraint::Length(queue_depth_height as u16),   // Queue depth
            Constraint::Length(db_stats_height as u16),      // DB stats
            Constraint::Length(worker_stats_height as u16),  // Worker stats
            Constraint::Min(0),                              // Bottom margin
        ])
        .split(area);

    // Render status message centered
    let message = Paragraph::new(screen.status_message())
        .style(Style::default().fg(Color::Cyan))
        .alignment(Alignment::Center);
    f.render_widget(message, chunks[1]);

    // Render eye centered horizontally
    let eye_area = Rect {
        x: area.x + h_margin,
        y: chunks[3].y,
        width: eye_width.min(area.width),
        height: chunks[3].height,
    };

    // Use provided eye frame or phase-appropriate frame
    let eye_art = eye_frame.unwrap_or_else(|| screen.eye_art());

    let eye_lines: Vec<Line> = eye_art
        .lines()
        .map(|line| Line::from(line))
        .collect();

    let eye_widget = Paragraph::new(eye_lines)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(eye_widget, eye_area);

    // Render progress bar if present
    if let Some(progress) = screen.progress {
        let progress_area = chunks[4];

        let bar_width = 40u16.min(progress_area.width.saturating_sub(4));
        let bar_x = (progress_area.width.saturating_sub(bar_width)) / 2 + progress_area.x;

        let filled = (progress * bar_width as f32) as u16;
        let empty = bar_width.saturating_sub(filled);

        let bar_text = format!(
            "[{}{}]",
            "=".repeat(filled as usize),
            " ".repeat(empty as usize)
        );

        let detail = screen.progress_detail.as_deref().unwrap_or("");
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

    // Render pending DB writes
    if show_queue_depth {
        let queue_area = chunks[5];
        let label_color = Color::Rgb(245, 28, 153); // Magenta

        let queue_line = Line::from(vec![
            Span::styled(
                format!("[{} pending DB writes queued]", screen.db_queue_depth),
                Style::default().fg(label_color),
            ),
        ]);

        let queue_widget = Paragraph::new(queue_line)
            .alignment(Alignment::Center);
        f.render_widget(queue_widget, queue_area);
    }

    // Render DB stats if timing enabled
    if show_db_stats {
        if let Some(stats) = &screen.db_stats {
            let stats_area = chunks[6];

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

            let stats_widget = Paragraph::new(stats_line)
                .alignment(Alignment::Center);
            f.render_widget(stats_widget, stats_area);
        }
    }

    // Render worker stats if timing enabled
    if show_worker_stats {
        if let Some(stats) = &screen.worker_stats {
            let stats_area = chunks[7];
            let label_color = Color::Rgb(245, 28, 153); // Magenta

            let avg_task_color = if stats.avg_task_ms < 100 {
                Color::Green
            } else if stats.avg_task_ms < 500 {
                Color::Yellow
            } else {
                Color::Red
            };

            let queue_wait_color = if stats.queue_wait_avg_ms < 5 {
                Color::Green
            } else if stats.queue_wait_avg_ms < 20 {
                Color::Yellow
            } else {
                Color::Red
            };

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
    }
}
