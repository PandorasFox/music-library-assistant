//! Splash Screen Modal
//!
//! Full-screen loading modal shown during startup while the Eye is Closed.
//! Displays progress during initial eyeballing (corpus health check).
//!
//! ## Lifecycle
//!
//! ```text
//! App starts
//!     │
//!     ├─► SplashScreen::new() - splash active
//!     │
//!     ├─► daemon.start_paranoid_eyeball() - queues work
//!     │
//!     ├─► SplashScreen::tick() each frame
//!     │       │
//!     │       └─► Polls daemon, updates progress
//!     │           Checks daemon.eye_state() == Awake
//!     │
//!     └─► SplashScreen::is_complete() becomes true
//!             │
//!             └─► Caller removes splash, transitions to main UI
//! ```

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::config::Config;
use crate::daemon::{DaemonStatus, EyeState, TaskDaemon, WorkerStats};
use crate::db_thread::DbThreadStats;
use super::app::{EYE_CLOSED, EYE_CLOSING};

// ============================================================================
// Loading Types
// ============================================================================

/// Type of loading operation for the splash screen
#[derive(Debug, Clone)]
pub enum LoadingType {
    /// Initial corpus eyeballing (filesystem observation)
    Eyeballing,
    /// Tag verification phase (paranoid mode)
    TagVerification,
    /// Corpus scan in progress
    CorpusScan,
    /// Legacy library scan
    LegacyScan,
    /// General operation with custom message
    Operation(String),
}

impl LoadingType {
    /// Get the display message for this loading type
    pub fn message(&self) -> &str {
        match self {
            LoadingType::Eyeballing => "Checking corpus health...",
            LoadingType::TagVerification => "Verifying tags...",
            LoadingType::CorpusScan => "Scanning corpus...",
            LoadingType::LegacyScan => "Scanning legacy library...",
            LoadingType::Operation(msg) => msg,
        }
    }
}

// ============================================================================
// Splash Screen State
// ============================================================================

/// Splash screen controller.
///
/// Manages the startup loading screen. Eye wake-up is handled by the daemon;
/// splash screen just displays progress and checks for completion.
#[derive(Debug)]
pub struct SplashScreen {
    /// Type of loading operation
    loading_type: LoadingType,
    /// Optional progress (0.0 to 1.0)
    progress: Option<f32>,
    /// Optional progress detail (e.g., "1234 / 5678 tasks")
    progress_detail: Option<String>,
    /// True once daemon's eye state becomes Awake
    complete: bool,
    /// Current eye state (for rendering appropriate frame)
    eye_state: EyeState,
    /// DB thread stats for optional display
    db_stats: Option<DbThreadStats>,
    /// Worker thread stats for optional display
    worker_stats: Option<WorkerStats>,
    /// Pending DB writes (always tracked, no timing guard)
    db_queue_depth: u64,
}

impl SplashScreen {
    /// Create a new splash screen for startup eyeballing.
    ///
    /// The splash will remain active until daemon.eye_state() becomes Awake,
    /// at which point `is_complete()` returns true.
    pub fn new() -> Self {
        Self {
            loading_type: LoadingType::Eyeballing,
            progress: Some(0.0),
            progress_detail: Some("Starting...".to_string()),
            complete: false,
            eye_state: EyeState::Closed,
            db_stats: None,
            worker_stats: None,
            db_queue_depth: 0,
        }
    }

    /// Create a splash screen for a specific loading type (non-startup).
    pub fn with_loading_type(loading_type: LoadingType) -> Self {
        Self {
            loading_type,
            progress: Some(0.0),
            progress_detail: Some("Starting...".to_string()),
            complete: false,
            eye_state: EyeState::Closed,
            db_stats: None,
            worker_stats: None,
            db_queue_depth: 0,
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

    /// Check if the splash screen is complete and ready to transition.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Get the loading type.
    pub fn loading_type(&self) -> &LoadingType {
        &self.loading_type
    }

    /// Get current progress (0.0 to 1.0).
    pub fn progress(&self) -> Option<f32> {
        self.progress
    }

    /// Get progress detail string.
    pub fn progress_detail(&self) -> Option<&str> {
        self.progress_detail.as_deref()
    }

    /// Get the main status message based on current eye state.
    pub fn status_message(&self) -> &'static str {
        match self.eye_state {
            EyeState::Closed => "Scanning corpus...",
            EyeState::Awakening => "Computing health signals...",
            EyeState::Awake => "Ready!",
        }
    }

    /// Tick the splash screen state.
    ///
    /// Returns `true` if the splash is complete (daemon eye state is Awake).
    /// Call this each frame while the splash is active.
    pub fn tick(&mut self, daemon: &TaskDaemon) -> bool {
        // Update progress from daemon status
        let status = daemon.status();
        self.update_progress(&status);

        // Track eye state for rendering
        self.eye_state = daemon.eye_state();

        // Check if daemon eye has awakened (startup complete)
        if self.eye_state == EyeState::Awake {
            self.complete = true;
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

/// Render the splash screen.
///
/// Shows a centered closed eye with a loading message and optional progress bar.
/// Always shows pending DB writes when queue has backlog.
/// If timing instrumentation is enabled, also shows detailed stats.
pub fn render(f: &mut Frame, area: Rect, splash: &SplashScreen, _config: &Config) {
    // Eye closed art is 16 lines tall
    let eye_height = 16;
    // Message line + spacing + eye + optional progress bar + optional stats
    let progress_height = if splash.progress.is_some() { 3 } else { 0 };

    // Always show queue depth when there's a backlog (no timing guard)
    let show_queue_depth = splash.db_queue_depth > 0;
    let queue_depth_height = if show_queue_depth { 1 } else { 0 };

    // Detailed timing stats (behind timing guard)
    let show_stats = crate::config::is_timing_enabled();
    let show_db_stats = show_stats && splash.db_stats.is_some();
    let show_worker_stats = show_stats && splash.worker_stats.is_some();
    let db_stats_height = if show_db_stats { 1 } else { 0 };
    let worker_stats_height = if show_worker_stats { 2 } else { 0 }; // 2 lines for all worker stats
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
            Constraint::Length(progress_height as u16),      // Progress bar (if any)
            Constraint::Length(queue_depth_height as u16),   // Queue depth (always, if backlog)
            Constraint::Length(db_stats_height as u16),      // DB stats (if timing enabled)
            Constraint::Length(worker_stats_height as u16),  // Worker stats (if timing enabled)
            Constraint::Min(0),                              // Bottom margin
        ])
        .split(area);

    // Render message centered - use eye-state-based message
    let message = Paragraph::new(splash.status_message())
        .style(Style::default().fg(Color::Cyan))
        .alignment(Alignment::Center);
    f.render_widget(message, chunks[1]);

    // Render eye centered horizontally
    // Use half-open frame during Awakening, closed otherwise
    let eye_area = Rect {
        x: area.x + h_margin,
        y: chunks[3].y,
        width: eye_width.min(area.width),
        height: chunks[3].height,
    };

    let eye_art = match splash.eye_state {
        EyeState::Awakening => EYE_CLOSING, // Half-open frame
        _ => EYE_CLOSED,
    };

    let eye_lines: Vec<Line> = eye_art
        .lines()
        .map(|line| Line::from(line))
        .collect();

    let eye_widget = Paragraph::new(eye_lines)
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(eye_widget, eye_area);

    // Render progress bar if present
    if let Some(progress) = splash.progress {
        let progress_area = chunks[4];

        // Create progress bar
        let bar_width = 40u16.min(progress_area.width.saturating_sub(4));
        let bar_x = (progress_area.width.saturating_sub(bar_width)) / 2 + progress_area.x;

        let filled = (progress * bar_width as f32) as u16;
        let empty = bar_width.saturating_sub(filled);

        let bar_text = format!(
            "[{}{}]",
            "=".repeat(filled as usize),
            " ".repeat(empty as usize)
        );

        let detail = splash.progress_detail.as_deref().unwrap_or("");
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
                format!("[{} pending DB writes queued]", splash.db_queue_depth),
                Style::default().fg(label_color),
            ),
        ]);

        let queue_widget = Paragraph::new(queue_line)
            .alignment(Alignment::Center);
        f.render_widget(queue_widget, queue_area);
    }

    // Render detailed DB stats if timing enabled and available
    if show_db_stats {
        if let Some(stats) = &splash.db_stats {
            let stats_area = chunks[6];

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

            let stats_widget = Paragraph::new(stats_line)
                .alignment(Alignment::Center);
            f.render_widget(stats_widget, stats_area);
        }
    }

    // Render worker stats if enabled and available
    if show_worker_stats {
        if let Some(stats) = &splash.worker_stats {
            let stats_area = chunks[7];

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
    }
}
