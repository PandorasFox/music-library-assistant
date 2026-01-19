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
    text::Line,
    widgets::Paragraph,
    Frame,
};

use std::collections::HashMap;

use crate::daemon::{DaemonStatus, EyeState, TaskDaemon};
use super::app::EYE_CLOSED;

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
        }
    }

    /// Create a splash screen for a specific loading type (non-startup).
    pub fn with_loading_type(loading_type: LoadingType) -> Self {
        Self {
            loading_type,
            progress: Some(0.0),
            progress_detail: Some("Starting...".to_string()),
            complete: false,
        }
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

    /// Tick the splash screen state.
    ///
    /// Returns `true` if the splash is complete (daemon eye state is Awake).
    /// Call this each frame while the splash is active.
    pub fn tick(&mut self, daemon: &TaskDaemon) -> bool {
        // Update progress from daemon status
        let status = daemon.status();
        self.update_progress(&status);

        // Check if daemon eye has awakened (eyeballing complete)
        if daemon.eye_state() == EyeState::Awake {
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

            // Show current task type if available
            let task_desc = Self::describe_current_task(&status.task_counts, total, completed);
            self.progress_detail = Some(format!(
                "{} ({}/{})",
                task_desc, completed, total
            ));
        }
    }

    /// Describe the current task phase based on task_counts.
    fn describe_current_task(task_counts: &HashMap<String, usize>, total: usize, completed: usize) -> &'static str {
        // During eyeballing, the phases happen in order:
        // 1. WalkCorpus (Eyeballing/Eyeballing (paranoid))
        // 2. CompareInodes
        // 3. VerifyMtime
        // 4. VerifyTags (paranoid mode)
        // 5. Create signals (MissingFromIndex, MissingFromDisk)

        // Check which phase has incomplete work
        let eyeball_count = task_counts.get("Eyeballing").copied().unwrap_or(0)
            + task_counts.get("Eyeballing (paranoid)").copied().unwrap_or(0);
        let compare_count = task_counts.get("Comparing inodes").copied().unwrap_or(0);
        let mtime_count = task_counts.get("Verifying mtime").copied().unwrap_or(0);
        let tag_count = task_counts.get("Tag verification").copied().unwrap_or(0);
        let index_count = task_counts.get("Indexing tracks").copied().unwrap_or(0);

        // Determine current phase (in-progress or about to start)
        if eyeball_count > 0 && completed < eyeball_count {
            "Walking filesystem..."
        } else if compare_count > 0 && completed < (eyeball_count + compare_count) {
            "Comparing inodes..."
        } else if mtime_count > 0 && completed < (eyeball_count + compare_count + mtime_count) {
            "Checking timestamps..."
        } else if tag_count > 0 && completed < (eyeball_count + compare_count + mtime_count + tag_count) {
            "Verifying tags..."
        } else if index_count > 0 {
            "Indexing files..."
        } else if completed >= total {
            "Finishing up..."
        } else {
            "Processing..."
        }
    }
}

// ============================================================================
// Rendering
// ============================================================================

/// Render the splash screen.
///
/// Shows a centered closed eye with a loading message and optional progress bar.
pub fn render(f: &mut Frame, area: Rect, splash: &SplashScreen) {
    // Eye closed art is 16 lines tall
    let eye_height = 16;
    // Message line + spacing + eye + optional progress bar
    let progress_height = if splash.progress.is_some() { 3 } else { 0 };
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
    let message = Paragraph::new(splash.loading_type.message())
        .style(Style::default().fg(Color::Cyan))
        .alignment(Alignment::Center);
    f.render_widget(message, chunks[1]);

    // Render eye (closed) centered horizontally
    let eye_area = Rect {
        x: area.x + h_margin,
        y: chunks[3].y,
        width: eye_width.min(area.width),
        height: chunks[3].height,
    };

    let eye_lines: Vec<Line> = EYE_CLOSED
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
}
