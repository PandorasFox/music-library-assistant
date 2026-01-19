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

use crate::daemon::{DaemonStatus, EyeState, TaskDaemon};
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
}
