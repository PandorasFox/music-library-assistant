//! Content Analysis Progress Screen
//!
//! Full-screen progress modal shown during content analysis phase.
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
    text::Line,
    widgets::Paragraph,
    Frame,
};

use crate::daemon::{DaemonStateSnapshot, DaemonStatus, TaskDaemon};

// ============================================================================
// Content Analysis Progress State
// ============================================================================

/// Content analysis progress screen controller.
///
/// Manages the progress display during content analysis phase.
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
}

impl ContentAnalysisProgress {
    /// Create a new content analysis progress screen.
    ///
    /// The screen will remain active until daemon completes all content analysis
    /// computations, at which point `is_complete()` returns true.
    pub fn new() -> Self {
        Self {
            progress: Some(0.0),
            progress_detail: Some("Starting content analysis...".to_string()),
            complete: false,
            seen_working: false,
        }
    }

    /// Check if content analysis is complete and ready to transition.
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
            "Analyzing content..."
        }
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

/// Render the content analysis progress screen.
///
/// Shows a centered eye (provided by caller, can be animated) with a progress message.
/// The eye_frame parameter should come from the app's Eye animation state.
pub fn render(f: &mut Frame, area: Rect, progress: &ContentAnalysisProgress, eye_frame: &str) {
    // Eye art is 16 lines tall
    let eye_height = 16;
    // Message line + spacing + eye + optional progress bar
    let progress_height = if progress.progress.is_some() { 3 } else { 0 };
    let total_height = 2 + eye_height + progress_height; // message + gap + eye + progress

    // Calculate vertical centering
    let v_margin = area.height.saturating_sub(total_height as u16) / 2;

    // Calculate horizontal centering for the eye (eye is ~60 chars wide)
    let eye_width = 60u16;
    let h_margin = area.width.saturating_sub(eye_width) / 2;

    // Layout vertically
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(v_margin),           // Top margin
            Constraint::Length(1),                  // Message
            Constraint::Length(1),                  // Spacing
            Constraint::Length(eye_height as u16), // Eye
            Constraint::Length(progress_height as u16), // Progress bar (if any)
            Constraint::Min(0),                     // Bottom margin
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
}
